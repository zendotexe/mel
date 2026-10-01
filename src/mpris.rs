use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::cover::{self, Cover};

const FIELDS: [&str; 10] = [
    "status",
    "title",
    "artist",
    "album",
    "mpris:artUrl",
    "mpris:length",
    "position",
    "volume",
    "shuffle",
    "loop",
];
const POSITION: usize = 6;

#[derive(Clone)]
pub struct State {
    pub status: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub art_url: String,
    pub length_us: f64,
    pub position_us: f64,
    pub volume: f64,
    pub shuffle: bool,
    pub loop_status: String,
    pub polled: Instant,
}

impl State {
    pub fn playing(&self) -> bool {
        self.status == "Playing"
    }

    pub fn position_now(&self) -> f64 {
        let mut pos = self.position_us;
        if self.playing() && self.length_us > 0.0 {
            pos = (pos + self.polled.elapsed().as_secs_f64() * 1e6).min(self.length_us);
        }
        pos
    }

    fn set_position(&mut self, us: f64) {
        self.position_us = us.clamp(
            0.0,
            if self.length_us > 0.0 {
                self.length_us
            } else {
                f64::MAX
            },
        );
        self.polled = Instant::now();
    }
}

#[derive(Default)]
pub struct Shared {
    pub state: Option<State>,
    pub cover: Option<Arc<Cover>>,
    gen: u64,
    hold_position_until: Option<Instant>,
}

const SEEK_HOLD: Duration = Duration::from_millis(1500);

pub fn player() -> String {
    std::env::var("MEL_PLAYER").unwrap_or_else(|_| "spotify".into())
}

pub fn playerctl(args: &[&str]) -> Option<String> {
    let out = Command::new("playerctl")
        .arg("-p")
        .arg(player())
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn format_string(fields: &[&str]) -> String {
    fields
        .iter()
        .map(|f| format!("{{{{{f}}}}}"))
        .collect::<Vec<_>>()
        .join("\x1f")
}

fn parse(values: &[&str], prev: Option<&State>) -> State {
    let get = |i: usize| values.get(i).map(|s| s.trim()).unwrap_or("");
    let num = |i: usize| get(i).parse::<f64>().ok();
    let prev_str = |f: fn(&State) -> &String| prev.map(|p| f(p).clone()).unwrap_or_default();
    let text = |i: usize, f: fn(&State) -> &String| {
        if get(i).is_empty() {
            prev_str(f)
        } else {
            get(i).to_string()
        }
    };
    State {
        status: text(0, |s| &s.status),
        title: get(1).to_string(),
        artist: get(2).to_string(),
        album: get(3).to_string(),
        art_url: get(4).to_string(),
        length_us: num(5).unwrap_or(0.0),
        position_us: num(POSITION).unwrap_or_else(|| prev.map_or(0.0, |p| p.position_now())),
        volume: num(7).or(prev.map(|p| p.volume)).unwrap_or(0.0),
        shuffle: match get(8) {
            "" => prev.is_some_and(|p| p.shuffle),
            v => matches!(v, "true" | "On" | "1"),
        },
        loop_status: match get(9) {
            "" => prev.map_or("None".into(), |p| p.loop_status.clone()),
            v => v.to_string(),
        },
        polled: Instant::now(),
    }
}

fn fetch_state() -> Option<State> {
    let raw = playerctl(&["metadata", "--format", &format_string(&FIELDS)])?;
    let mut values: Vec<String> = raw.split('\x1f').map(String::from).collect();
    values.resize(FIELDS.len(), String::new());
    for (i, cmd) in [(0, "status"), (7, "volume"), (8, "shuffle"), (9, "loop")] {
        if values[i].is_empty() {
            values[i] = playerctl(&[cmd]).unwrap_or_default();
        }
    }
    let refs: Vec<&str> = values.iter().map(String::as_str).collect();
    Some(parse(&refs, None))
}

fn follow_loop(shared: Arc<Mutex<Shared>>) {
    let fields: Vec<&str> = FIELDS
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != POSITION)
        .map(|(_, f)| *f)
        .collect();
    loop {
        let child = crate::die_with_mel(&mut Command::new("playerctl"))
            .args([
                "-p",
                &player(),
                "--follow",
                "metadata",
                "--format",
                &format_string(&fields),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        let Ok(mut child) = child else { return };
        let Some(stdout) = child.stdout.take() else {
            return;
        };
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            let mut g = shared.lock().unwrap();
            if line.trim().is_empty() {
                g.state = None;
                continue;
            }
            let mut values: Vec<&str> = line.split('\x1f').collect();
            values.insert(POSITION, "");
            let prev = g.state.clone();
            let mut state = parse(&values, prev.as_ref());
            if prev
                .as_ref()
                .is_some_and(|p| p.title != state.title || p.art_url != state.art_url)
            {
                state.position_us = 0.0;
            }
            g.state = Some(state);
        }
        let _ = child.kill();
        let _ = child.wait();
        thread::sleep(Duration::from_secs(1));
    }
}

fn cover_loop(shared: Arc<Mutex<Shared>>) {
    let mut loaded = String::new();
    loop {
        let url = shared
            .lock()
            .unwrap()
            .state
            .as_ref()
            .map(|s| s.art_url.clone())
            .unwrap_or_default();
        if url != loaded {
            loaded = url.clone();
            let cover = if url.is_empty() {
                None
            } else {
                Some(Arc::new(cover::load(&url)))
            };
            let mut g = shared.lock().unwrap();
            if g.state.as_ref().is_some_and(|s| s.art_url == url) || cover.is_none() {
                g.cover = cover;
            }
        }
        thread::sleep(Duration::from_millis(30));
    }
}

fn poll_loop(shared: Arc<Mutex<Shared>>) {
    loop {
        let gen = shared.lock().unwrap().gen;
        let state = fetch_state();
        {
            let mut g = shared.lock().unwrap();
            if g.gen == gen {
                let mut state = state;
                let holding = g.hold_position_until.is_some_and(|t| Instant::now() < t);
                if let (true, Some(new), Some(old)) = (holding, state.as_mut(), g.state.as_ref()) {
                    if new.title == old.title {
                        new.position_us = old.position_us;
                        new.polled = old.polled;
                    }
                }
                g.state = state;
            }
        }
        thread::sleep(Duration::from_secs(1));
    }
}

fn command_loop(commands: Receiver<Vec<String>>) {
    let is_seek = |c: &Vec<String>| c.first().is_some_and(|a| a == "position");
    while let Ok(first) = commands.recv() {
        let mut batch = vec![first];
        batch.extend(commands.try_iter());
        let last_seek = batch.iter().rposition(is_seek);
        for (i, args) in batch.iter().enumerate() {
            if is_seek(args) && Some(i) != last_seek {
                continue;
            }
            if args.first().is_some_and(|a| a == "@show") {
                crate::spotify::toggle_window();
                continue;
            }
            let refs: Vec<&str> = args.iter().map(String::as_str).collect();
            playerctl(&refs);
        }
    }
}

pub struct Player {
    pub shared: Arc<Mutex<Shared>>,
    commands: Sender<Vec<String>>,
}

impl Player {
    pub fn start() -> Self {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let (cmd_tx, cmd_rx) = mpsc::channel();
        for f in [follow_loop, cover_loop] {
            let s = shared.clone();
            thread::spawn(move || f(s));
        }
        {
            let s = shared.clone();
            thread::spawn(move || poll_loop(s));
        }
        thread::spawn(move || command_loop(cmd_rx));
        Player {
            shared,
            commands: cmd_tx,
        }
    }

    pub fn snapshot(&self) -> (Option<State>, Option<Arc<Cover>>) {
        let g = self.shared.lock().unwrap();
        (g.state.clone(), g.cover.clone())
    }

    pub fn command(&self, args: &[&str], update: impl FnOnce(&mut State)) {
        {
            let mut g = self.shared.lock().unwrap();
            if let Some(s) = g.state.as_mut() {
                update(s);
            }
            g.gen += 1;
        }
        let _ = self
            .commands
            .send(args.iter().map(|s| s.to_string()).collect());
    }

    pub fn seek_by(&self, delta_us: f64) {
        let target = {
            let mut g = self.shared.lock().unwrap();
            let Some(s) = g.state.as_mut() else { return };
            let target = (s.position_now() + delta_us).max(0.0);
            if s.length_us > 0.0 && target >= s.length_us - 1e6 {
                None
            } else {
                s.set_position(target);
                g.gen += 1;
                g.hold_position_until = Some(Instant::now() + SEEK_HOLD);
                Some(target)
            }
        };
        match target {
            Some(t) => {
                let _ = self
                    .commands
                    .send(vec!["position".into(), format!("{:.3}", t / 1e6)]);
            }
            None => self.command(&["next"], restart_track),
        }
    }

    pub fn toggle_spotify(&self) {
        let _ = self.commands.send(vec!["@show".into()]);
    }
}

pub fn toggle_play(s: &mut State) {
    let pos = s.position_now();
    s.status = if s.playing() {
        "Paused".into()
    } else {
        "Playing".into()
    };
    s.set_position(pos);
}

pub fn restart_track(s: &mut State) {
    s.set_position(0.0);
}

pub fn change_volume(delta: f64) -> impl FnOnce(&mut State) {
    move |s| s.volume = (s.volume + delta).clamp(0.0, 1.0)
}

pub fn toggle_shuffle(s: &mut State) {
    s.shuffle = !s.shuffle;
}

pub fn set_loop(next: &'static str) -> impl FnOnce(&mut State) {
    move |s| s.loop_status = next.into()
}

pub fn stop(s: &mut State) {
    s.status = "Stopped".into();
    s.set_position(0.0);
}
