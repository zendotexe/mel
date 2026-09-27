mod cover;
mod input;
mod mpris;
mod spotify;
mod ui;
mod vis;

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::terminal;

use input::{Input, Key};
use mpris::Player;

const FRAME: Duration = Duration::from_millis(1000 / 30);

pub fn die_with_mel(cmd: &mut std::process::Command) -> &mut std::process::Command {
    use std::os::unix::process::CommandExt;
    unsafe {
        cmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        })
    }
}

enum Action {
    Quit,
    Message(String),
    Done,
}

fn handle(key: Key, player: &Player) -> Action {
    match key {
        Key::CtrlC | Key::Esc | Key::Char('q') | Key::Char('Q') => return Action::Quit,
        Key::Char(' ') | Key::Char('p') => player.command(&["play-pause"], mpris::toggle_play),
        Key::Char('l') | Key::Right => player.command(&["next"], mpris::restart_track),
        Key::Char('h') | Key::Left => player.command(&["previous"], mpris::restart_track),
        Key::Char('d') => player.seek_by(5e6),
        Key::Char('a') => player.seek_by(-5e6),
        Key::Char('+') | Key::Char('=') => {
            player.command(&["volume", "0.05+"], mpris::change_volume(0.05))
        }
        Key::Char('-') => player.command(&["volume", "0.05-"], mpris::change_volume(-0.05)),
        Key::Char('s') => player.command(&["shuffle", "Toggle"], mpris::toggle_shuffle),
        Key::Char('r') => {
            let next = match player.snapshot().0.as_ref().map(|s| s.loop_status.as_str()) {
                Some("None") | None => "Playlist",
                Some("Playlist") => "Track",
                _ => "None",
            };
            player.command(&["loop", next], mpris::set_loop(next))
        }
        Key::Char('S') => player.command(&["stop"], mpris::stop),
        Key::Char('o') => {
            player.show_spotify();
            return Action::Message("showing Spotify".into());
        }
        _ => {}
    }
    Action::Done
}

fn restore_terminal(kitty: bool) {
    let mut out = io::stdout();
    let _ = write!(
        out,
        "{}\x1b[?25h\x1b[?1049l",
        if kitty { cover::KITTY_CLEAR } else { "" }
    );
    let _ = out.flush();
    let _ = terminal::disable_raw_mode();
}

fn main() -> io::Result<()> {
    if std::process::Command::new("playerctl")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("mel needs playerctl");
        std::process::exit(1);
    }

    let term = Arc::new(AtomicBool::new(false));
    for sig in [signal_hook::consts::SIGHUP, signal_hook::consts::SIGTERM] {
        signal_hook::flag::register(sig, term.clone())?;
    }

    spotify::launch_hidden();

    let mut ui = ui::Ui::new();
    let kitty = ui.kitty();
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal(kitty);
        spotify::quit_if_owned();
        default_hook(info);
    }));

    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    write!(stdout, "\x1b[?1049h\x1b[?25l\x1b]0;mel\x07")?;
    stdout.flush()?;

    let player = Player::start();
    let mut vis = vis::Visualizer::new();
    let mut message: Option<(String, Instant)> = None;

    while !term.load(Ordering::SeqCst) {
        let frame_start = Instant::now();
        if message
            .as_ref()
            .is_some_and(|(_, until)| frame_start > *until)
        {
            message = None;
        }
        let (state, cover) = player.snapshot();
        let screen = ui.draw(
            state.as_ref(),
            cover.as_deref(),
            message.as_ref().map(|(m, _)| m.as_str()),
            &mut vis,
            spotify::owned(),
        );
        if stdout
            .write_all(screen.as_bytes())
            .and_then(|_| stdout.flush())
            .is_err()
        {
            break;
        }

        match input::wait(FRAME.saturating_sub(frame_start.elapsed())) {
            Input::Closed => break,
            Input::Timeout => {}
            Input::Keys(keys) => {
                let mut quit = false;
                for key in keys {
                    match handle(key, &player) {
                        Action::Quit => quit = true,
                        Action::Message(m) => {
                            message = Some((m, Instant::now() + Duration::from_secs(3)))
                        }
                        Action::Done => {}
                    }
                }
                if quit {
                    break;
                }
            }
        }
    }

    vis.close();
    restore_terminal(kitty);
    spotify::quit_if_owned();
    Ok(())
}
