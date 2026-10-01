use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

static OWNED: AtomicBool = AtomicBool::new(false);

const HIDDEN_WS: &str = "special:mel";

pub fn owned() -> bool {
    OWNED.load(Ordering::SeqCst)
}

fn hyprland() -> bool {
    std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some()
}

fn hypr_dispatch(lua: &str) -> bool {
    Command::new("hyprctl")
        .args(["dispatch", lua])
        .output()
        .is_ok_and(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "ok")
}

fn hypr_json(what: &str) -> Option<serde_json::Value> {
    let out = Command::new("hyprctl").args([what, "-j"]).output().ok()?;
    serde_json::from_slice(&out.stdout).ok()
}

fn spawn_detached() {
    let _ = Command::new("spotify")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn();
}

pub fn running() -> bool {
    Command::new("pgrep")
        .args(["-x", "spotify"])
        .output()
        .is_ok_and(|o| o.status.success())
}

pub fn launch_hidden() {
    if running() {
        return;
    }
    let lua = format!(r#"hl.dsp.exec_cmd("spotify", {{ workspace = "{HIDDEN_WS} silent" }})"#);
    if !(hyprland() && hypr_dispatch(&lua)) {
        spawn_detached();
    }
    OWNED.store(true, Ordering::SeqCst);
}

pub fn quit_if_owned() {
    if !owned() {
        return;
    }
    let _ = Command::new("playerctl")
        .args(["-p", "spotify", "pause"])
        .status();
    if hyprland() {
        if let Some(w) = spotify_window() {
            if !w["workspace"]["name"]
                .as_str()
                .is_some_and(|n| n.starts_with("special:"))
            {
                let addr = w["address"].as_str().unwrap_or_default();
                hypr_dispatch(&format!(
                    r#"hl.dsp.window.move({{ workspace = "{HIDDEN_WS}", window = "address:{addr}", follow = false }})"#
                ));
            }
        }
    }
    let asked = Command::new("dbus-send")
        .args([
            "--session",
            "--type=method_call",
            "--dest=org.mpris.MediaPlayer2.spotify",
            "/org/mpris/MediaPlayer2",
            "org.mpris.MediaPlayer2.Quit",
        ])
        .status()
        .is_ok_and(|s| s.success());
    if !asked {
        let _ = Command::new("pkill").args(["-x", "spotify"]).status();
    }
    for _ in 0..40 {
        if !running() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let _ = Command::new("pkill")
        .args(["-KILL", "-x", "spotify"])
        .status();
}

fn spotify_window() -> Option<serde_json::Value> {
    hypr_json("clients").and_then(|clients| {
        clients
            .as_array()?
            .iter()
            .find(|c| {
                c["class"]
                    .as_str()
                    .is_some_and(|s| s.eq_ignore_ascii_case("spotify"))
            })
            .cloned()
    })
}

pub fn toggle_window() {
    let was_running = running();
    if !was_running {
        OWNED.store(true, Ordering::SeqCst);
    }
    if !hyprland() {
        spawn_detached();
        return;
    }
    let Some(w) = spotify_window() else {
        spawn_detached();
        return;
    };
    let addr = w["address"].as_str().unwrap_or_default();
    let hidden = w["workspace"]["name"]
        .as_str()
        .is_some_and(|n| n.starts_with("special:"));
    if hidden {
        let active = hypr_json("activeworkspace")
            .and_then(|ws| ws["id"].as_i64())
            .unwrap_or(1);
        hypr_dispatch(&format!(
            r#"hl.dsp.window.move({{ workspace = "{active}", window = "address:{addr}" }})"#
        ));
        hypr_dispatch(&format!(r#"hl.dsp.focus({{ window = "address:{addr}" }})"#));
    } else {
        hypr_dispatch(&format!(
            r#"hl.dsp.window.move({{ workspace = "{HIDDEN_WS}", window = "address:{addr}", follow = false }})"#
        ));
    }
}
