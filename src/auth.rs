use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use sha2::{Digest, Sha256};

const REDIRECT_PORT: u16 = 8942;
const REDIRECT_URI: &str = "http://127.0.0.1:8942/callback";
const SCOPES: &str =
    "playlist-read-private playlist-read-collaborative user-modify-playback-state user-read-playback-state";
const TOKEN_URL: &str = "https://accounts.spotify.com/api/token";
const AUTHORIZE_URL: &str = "https://accounts.spotify.com/authorize";

pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: SystemTime,
}

fn config_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config"));
    base.join("mel")
}

fn client_id_path() -> PathBuf {
    config_dir().join("client_id")
}

fn token_path() -> PathBuf {
    config_dir().join("token.json")
}

fn read_client_id() -> Option<String> {
    if let Ok(id) = std::env::var("MEL_SPOTIFY_CLIENT_ID") {
        if !id.trim().is_empty() {
            return Some(id.trim().to_string());
        }
    }
    fs::read_to_string(client_id_path())
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn prompt_client_id() -> Option<String> {
    println!("mel needs a Spotify API client id to look up playlists (one-time setup).");
    println!("  1. Open https://developer.spotify.com/dashboard and create an app.");
    println!("  2. In its settings, add this exact Redirect URI:");
    println!("       {REDIRECT_URI}");
    println!("  3. Copy the Client ID shown on the app page and paste it here.");
    print!("Client ID: ");
    std::io::stdout().flush().ok()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).ok()?;
    let id = line.trim().to_string();
    if id.is_empty() {
        return None;
    }
    let _ = fs::create_dir_all(config_dir());
    let _ = fs::write(client_id_path(), &id);
    Some(id)
}

fn random_bytes(n: usize) -> Vec<u8> {
    let mut buf = vec![0u8; n];
    if let Ok(mut f) = fs::File::open("/dev/urandom") {
        let _ = f.read_exact(&mut buf);
    }
    buf
}

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn code_verifier() -> String {
    b64url(&random_bytes(64))
}

fn code_challenge(verifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    b64url(&hasher.finalize())
}

fn enc(s: &str) -> String {
    utf8_percent_encode(s, NON_ALPHANUMERIC).to_string()
}

fn authorize_url(client_id: &str, challenge: &str, state: &str) -> String {
    format!(
        "{AUTHORIZE_URL}?client_id={}&response_type=code&redirect_uri={}&code_challenge_method=S256&code_challenge={}&scope={}&state={}",
        enc(client_id),
        enc(REDIRECT_URI),
        enc(challenge),
        enc(SCOPES),
        enc(state),
    )
}

fn open_browser(url: &str) {
    let _ = Command::new("xdg-open")
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

fn query_param(qs: &str, key: &str) -> Option<String> {
    qs.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then(|| {
            percent_encoding::percent_decode_str(v)
                .decode_utf8_lossy()
                .into_owned()
        })
    })
}

fn listen_for_code(expected_state: &str) -> Option<String> {
    let listener = TcpListener::bind(("127.0.0.1", REDIRECT_PORT)).ok()?;
    listener.set_nonblocking(true).ok()?;
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        if Instant::now() > deadline {
            return None;
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                let _ = stream.set_nonblocking(false);
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let first_line = req.lines().next().unwrap_or("");
                let path = first_line.split_whitespace().nth(1).unwrap_or("");
                if let Some(qs) = path.strip_prefix("/callback?") {
                    let body = "<html><body>mel: you can close this tab.</body></html>";
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                    let state = query_param(qs, "state");
                    return state
                        .filter(|s| s == expected_state)
                        .and_then(|_| query_param(qs, "code"));
                }
                let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nConnection: close\r\n\r\n");
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(100));
            }
            Err(_) => return None,
        }
    }
}

fn tokens_from_json(json: &serde_json::Value, fallback_refresh: Option<&str>) -> Option<Tokens> {
    let access_token = json["access_token"].as_str()?.to_string();
    let refresh_token = json["refresh_token"]
        .as_str()
        .map(str::to_string)
        .or_else(|| fallback_refresh.map(str::to_string))?;
    let expires_in = json["expires_in"].as_u64().unwrap_or(3600);
    let expires_at = SystemTime::now() + Duration::from_secs(expires_in);
    Some(Tokens {
        access_token,
        refresh_token,
        expires_at,
    })
}

fn request_tokens(form: &[(&str, &str)], fallback_refresh: Option<&str>) -> Option<Tokens> {
    let resp = ureq::post(TOKEN_URL)
        .set("Content-Type", "application/x-www-form-urlencoded")
        .send_form(form)
        .ok()?;
    let json: serde_json::Value = resp.into_json().ok()?;
    tokens_from_json(&json, fallback_refresh)
}

fn exchange_code(client_id: &str, code: &str, verifier: &str) -> Option<Tokens> {
    request_tokens(
        &[
            ("client_id", client_id),
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", REDIRECT_URI),
            ("code_verifier", verifier),
        ],
        None,
    )
}

fn refresh_tokens(client_id: &str, refresh_token: &str) -> Option<Tokens> {
    request_tokens(
        &[
            ("client_id", client_id),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ],
        Some(refresh_token),
    )
}

fn read_cached_tokens() -> Option<Tokens> {
    let raw = fs::read_to_string(token_path()).ok()?;
    let json: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let access_token = json["access_token"].as_str()?.to_string();
    let refresh_token = json["refresh_token"].as_str()?.to_string();
    let expires_at = UNIX_EPOCH + Duration::from_secs(json["expires_at"].as_u64().unwrap_or(0));
    Some(Tokens {
        access_token,
        refresh_token,
        expires_at,
    })
}

fn save_tokens(t: &Tokens) {
    let _ = fs::create_dir_all(config_dir());
    let expires_at = t
        .expires_at
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let json = serde_json::json!({
        "access_token": t.access_token,
        "refresh_token": t.refresh_token,
        "expires_at": expires_at,
    });
    if fs::write(token_path(), json.to_string()).is_ok() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(token_path(), fs::Permissions::from_mode(0o600));
        }
    }
}

fn login_flow(client_id: &str) -> Option<Tokens> {
    let verifier = code_verifier();
    let challenge = code_challenge(&verifier);
    let state = b64url(&random_bytes(16));
    let url = authorize_url(client_id, &challenge, &state);

    println!("mel: opening your browser to log in to Spotify...");
    println!("If it doesn't open, visit this URL:");
    println!("  {url}");
    open_browser(&url);

    println!("Waiting for you to finish logging in (up to 3 minutes)...");
    let code = listen_for_code(&state)?;
    exchange_code(client_id, &code, &verifier)
}

pub fn access_token() -> Option<String> {
    let client_id = read_client_id().or_else(prompt_client_id)?;

    if let Some(t) = read_cached_tokens() {
        if t.expires_at > SystemTime::now() + Duration::from_secs(60) {
            return Some(t.access_token);
        }
        if let Some(refreshed) = refresh_tokens(&client_id, &t.refresh_token) {
            save_tokens(&refreshed);
            return Some(refreshed.access_token);
        }
    }

    let tokens = login_flow(&client_id)?;
    save_tokens(&tokens);
    Some(tokens.access_token)
}
