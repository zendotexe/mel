use std::time::Duration;

use crate::auth;

pub struct Playlist {
    pub name: String,
    pub uri: String,
}

fn get_json(url: &str, token: &str) -> Option<serde_json::Value> {
    ureq::get(url)
        .set("Authorization", &format!("Bearer {token}"))
        .call()
        .ok()?
        .into_json()
        .ok()
}

fn list_playlists(token: &str) -> Vec<Playlist> {
    let mut out = Vec::new();
    let mut url = "https://api.spotify.com/v1/me/playlists?limit=50".to_string();
    while let Some(json) = get_json(&url, token) {
        if let Some(items) = json["items"].as_array() {
            for item in items {
                let name = item["name"].as_str().unwrap_or_default().to_string();
                let uri = item["uri"].as_str().unwrap_or_default().to_string();
                if !name.is_empty() && !uri.is_empty() {
                    out.push(Playlist { name, uri });
                }
            }
        }
        match json["next"].as_str() {
            Some(next) => url = next.to_string(),
            None => break,
        }
    }
    out
}

fn is_subsequence(query: &[char], haystack: &str) -> bool {
    let mut qi = 0;
    for c in haystack.chars() {
        if qi < query.len() && c == query[qi] {
            qi += 1;
        }
    }
    qi == query.len()
}

fn fuzzy_find(query: &str, playlists: &[Playlist]) -> Option<usize> {
    let q_lower = query.to_lowercase();
    let q_chars: Vec<char> = q_lower.chars().collect();
    playlists
        .iter()
        .position(|p| p.name.to_lowercase().contains(&q_lower))
        .or_else(|| {
            playlists
                .iter()
                .position(|p| is_subsequence(&q_chars, &p.name.to_lowercase()))
        })
}

fn devices(token: &str) -> Vec<(String, bool)> {
    let Some(json) = get_json("https://api.spotify.com/v1/me/player/devices", token) else {
        return Vec::new();
    };
    json["devices"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| {
            let id = d["id"].as_str()?.to_string();
            let active = d["is_active"].as_bool().unwrap_or(false);
            Some((id, active))
        })
        .collect()
}

fn wait_for_device(token: &str) -> Option<String> {
    for _ in 0..16 {
        let devs = devices(token);
        if let Some((id, _)) = devs.iter().find(|(_, active)| *active) {
            return Some(id.clone());
        }
        if let Some((id, _)) = devs.first() {
            return Some(id.clone());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    None
}

fn play_context(token: &str, device_id: Option<&str>, context_uri: &str) -> bool {
    let mut url = "https://api.spotify.com/v1/me/player/play".to_string();
    if let Some(id) = device_id {
        url.push_str("?device_id=");
        url.push_str(id);
    }
    ureq::put(&url)
        .set("Authorization", &format!("Bearer {token}"))
        .send_json(serde_json::json!({ "context_uri": context_uri }))
        .is_ok()
}

pub fn play_by_name(query: &str) -> Result<String, String> {
    let token = auth::access_token().ok_or("could not get a Spotify access token")?;
    let playlists = list_playlists(&token);
    if playlists.is_empty() {
        return Err("no playlists found (or Spotify API request failed)".into());
    }
    let idx =
        fuzzy_find(query, &playlists).ok_or_else(|| format!("no playlist matching \"{query}\""))?;
    let playlist = &playlists[idx];
    let device_id = wait_for_device(&token);
    if !play_context(&token, device_id.as_deref(), &playlist.uri) {
        return Err(format!("failed to start playback for \"{}\"", playlist.name));
    }
    Ok(playlist.name.clone())
}
