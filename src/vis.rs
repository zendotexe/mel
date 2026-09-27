use std::collections::HashSet;
use std::collections::VecDeque;
use std::os::fd::AsRawFd;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};

use crate::cover::Rgb;

const RATE: u32 = 22050;
const N: usize = 1024;
const LOW: f32 = 50.0;
const HIGH: f32 = 10000.0;
const RANGE_DB: f32 = 45.0;
const MIN_GAIN_DB: f32 = 80.0;
const RECHECK: Duration = Duration::from_secs(2);
const STALE: Duration = Duration::from_millis(400);

pub const HEIGHT: u16 = 6;
pub const BAR_WIDTH: u16 = 2;

pub struct Visualizer {
    samples: Arc<Mutex<VecDeque<f32>>>,
    last_audio: Arc<AtomicU64>,
    epoch: Instant,
    alive: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    levels: Vec<f32>,
    gain: f32,
}

impl Visualizer {
    pub fn new() -> Self {
        let samples = Arc::new(Mutex::new(VecDeque::from(vec![0.0; N])));
        let alive = Arc::new(AtomicBool::new(true));
        let child = Arc::new(Mutex::new(None));
        let last_audio = Arc::new(AtomicU64::new(0));
        let epoch = Instant::now();
        {
            let capture = Capture {
                samples: samples.clone(),
                last_audio: last_audio.clone(),
                epoch,
                alive: alive.clone(),
                child: child.clone(),
            };
            thread::spawn(move || capture.run());
        }
        let window = (0..N)
            .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (N - 1) as f32).cos())
            .collect();
        Visualizer {
            samples,
            last_audio,
            epoch,
            alive,
            child,
            fft: FftPlanner::new().plan_fft_forward(N),
            window,
            levels: Vec::new(),
            gain: 0.0,
        }
    }

    pub fn close(&self) {
        self.alive.store(false, Ordering::SeqCst);
        if let Some(mut c) = self.child.lock().unwrap().take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    pub fn update(&mut self, nbars: usize, active: bool) -> &[f32] {
        if self.levels.len() != nbars {
            self.levels = vec![0.0; nbars];
        }
        let fresh = self.epoch.elapsed().as_millis() as u64
            <= self.last_audio.load(Ordering::Relaxed) + STALE.as_millis() as u64;
        if !active || !fresh || nbars == 0 {
            self.levels.iter_mut().for_each(|l| *l *= 0.8);
            return &self.levels;
        }
        let mut buf: Vec<Complex<f32>> = {
            let s = self.samples.lock().unwrap();
            s.iter()
                .zip(&self.window)
                .map(|(v, w)| Complex::new(v * w, 0.0))
                .collect()
        };
        self.fft.process(&mut buf);
        let mags: Vec<f32> = buf[..N / 2].iter().map(|c| c.norm()).collect();

        let bin_hz = RATE as f32 / N as f32;
        let ratio = (HIGH / LOW).powf(1.0 / nbars as f32);
        let mut raw = Vec::with_capacity(nbars);
        let mut prev_hi = 1usize;
        for b in 0..nbars {
            let lo = prev_hi
                .max((LOW * ratio.powi(b as i32) / bin_hz) as usize)
                .min(N / 2 - 1);
            let hi = (lo + 1)
                .max((LOW * ratio.powi(b as i32 + 1) / bin_hz) as usize)
                .min(N / 2);
            prev_hi = hi;
            let peak = mags[lo..hi].iter().cloned().fold(0.0f32, f32::max);
            let centre = ((lo * hi) as f32).sqrt() * bin_hz;
            raw.push(20.0 * (peak + 1e-9).log10() + 3.0 * (centre / LOW).log2());
        }
        let peak = raw.iter().cloned().fold(f32::MIN, f32::max);
        self.gain = peak.max(self.gain - 0.05).max(MIN_GAIN_DB);
        for (level, v) in self.levels.iter_mut().zip(raw) {
            let target = ((v - (self.gain - RANGE_DB)) / RANGE_DB)
                .clamp(0.0, 1.0)
                .powf(1.6);
            *level = if target > *level {
                target
            } else {
                *level * 0.82 + target * 0.18
            };
        }
        &self.levels
    }
}

impl Drop for Visualizer {
    fn drop(&mut self) {
        self.close();
    }
}

struct Node {
    serial: String,
    running: bool,
    linked: bool,
}

fn spotify_nodes() -> Option<Vec<Node>> {
    let out = Command::new("pw-dump")
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let dump: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    let objects = dump.as_array()?;
    let linked: HashSet<u64> = objects
        .iter()
        .filter(|o| o["type"].as_str().is_some_and(|t| t.ends_with("Link")))
        .filter_map(|o| o["info"]["output-node-id"].as_u64())
        .collect();
    let mut nodes = Vec::new();
    for obj in objects {
        let info = &obj["info"];
        let props = &info["props"];
        if props["media.class"] != "Stream/Output/Audio" {
            continue;
        }
        let name = format!(
            "{}{}",
            props["application.name"].as_str().unwrap_or(""),
            props["application.process.binary"].as_str().unwrap_or("")
        )
        .to_lowercase();
        if !name.contains("spotify") {
            continue;
        }
        let serial = match &props["object.serial"] {
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::String(s) => s.clone(),
            _ => continue,
        };
        nodes.push(Node {
            serial,
            running: info["state"] == "running",
            linked: obj["id"].as_u64().is_some_and(|id| linked.contains(&id)),
        });
    }
    Some(nodes)
}

fn best_node(nodes: &[Node]) -> Option<&str> {
    nodes
        .iter()
        .find(|n| n.running)
        .or_else(|| nodes.iter().find(|n| n.linked))
        .map(|n| n.serial.as_str())
}

struct Capture {
    samples: Arc<Mutex<VecDeque<f32>>>,
    last_audio: Arc<AtomicU64>,
    epoch: Instant,
    alive: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
}

impl Capture {
    fn alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    fn run(self) {
        while self.alive() {
            let target = spotify_nodes().and_then(|nodes| best_node(&nodes).map(String::from));
            match target {
                Some(node) => self.capture(&node),
                None => thread::sleep(Duration::from_secs(1)),
            }
            if let Some(mut c) = self.child.lock().unwrap().take() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
    }

    fn capture(&self, node: &str) {
        let spawned = crate::die_with_mel(&mut Command::new("pw-record"))
            .args([
                "--target",
                node,
                "--rate",
                &RATE.to_string(),
                "--channels",
                "1",
                "--format",
                "s16",
            ])
            .args(["-P", "{ node.dont-reconnect = true }", "--raw", "-"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        let Ok(mut child) = spawned else {
            thread::sleep(Duration::from_secs(1));
            return;
        };
        let Some(stdout) = child.stdout.take() else {
            return;
        };
        *self.child.lock().unwrap() = Some(child);
        let fd = stdout.as_raw_fd();

        let mut buf = [0u8; 4096];
        let mut carry: Option<u8> = None;
        let mut last_check = Instant::now();
        while self.alive() {
            let mut pfd = libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            };
            let ready = unsafe { libc::poll(&mut pfd, 1, 200) };
            if ready > 0 {
                let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
                if n <= 0 {
                    return;
                }
                let mut bytes: Vec<u8> = carry.take().into_iter().collect();
                bytes.extend_from_slice(&buf[..n as usize]);
                if bytes.len() % 2 == 1 {
                    carry = bytes.pop();
                }
                {
                    let mut s = self.samples.lock().unwrap();
                    s.extend(
                        bytes
                            .chunks_exact(2)
                            .map(|p| i16::from_le_bytes([p[0], p[1]]) as f32),
                    );
                    let excess = s.len().saturating_sub(N);
                    s.drain(..excess);
                }
                self.last_audio
                    .store(self.epoch.elapsed().as_millis() as u64, Ordering::Relaxed);
            } else if ready < 0
                && std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR)
            {
                return;
            }
            if last_check.elapsed() >= RECHECK {
                last_check = Instant::now();
                match spotify_nodes() {
                    Some(nodes) => match best_node(&nodes) {
                        Some(best) if best != node => return,
                        None if !nodes.iter().any(|n| n.serial == node) => return,
                        _ => {}
                    },
                    None => {}
                }
            }
        }
    }
}

const BLOCKS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

pub fn render(levels: &[f32], accent: Rgb, height: u16) -> Vec<String> {
    let dim = (
        (accent.0 as f32 * 0.55) as u8,
        (accent.1 as f32 * 0.55) as u8,
        (accent.2 as f32 * 0.55) as u8,
    );
    let h = height as i32;
    (0..h)
        .map(|r| {
            let t = (h - 1 - r) as f32 / (h - 1).max(1) as f32;
            let mix = |d: u8, a: u8| (d as f32 + (a as f32 - d as f32) * (1.0 - t)) as u8;
            let mut line = format!(
                "\x1b[38;2;{};{};{}m",
                mix(dim.0, accent.0),
                mix(dim.1, accent.1),
                mix(dim.2, accent.2)
            );
            for (i, level) in levels.iter().enumerate() {
                let eighths = (level * h as f32 * 8.0) as i32;
                let fill = (eighths - (h - 1 - r) * 8).clamp(0, 8) as usize;
                let ch = if fill == 0 && r == h - 1 {
                    '▁'
                } else {
                    BLOCKS[fill]
                };
                for _ in 0..BAR_WIDTH {
                    line.push(ch);
                }
                if i + 1 < levels.len() {
                    line.push(' ');
                }
            }
            line.push_str("\x1b[0m");
            line
        })
        .collect()
}
