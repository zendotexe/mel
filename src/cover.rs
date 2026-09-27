use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{Cursor, Read};
use std::path::PathBuf;
use std::time::Duration;

use base64::Engine;
use image::imageops::FilterType;
use image::RgbImage;

pub type Rgb = (u8, u8, u8);

pub const DEFAULT_ACCENT: Rgb = (180, 190, 254);

pub struct Cover {
    pub url: String,
    pub img: Option<RgbImage>,
    pub accent: Rgb,
}

fn cache_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".cache"));
    base.join("mel")
}

fn fetch(url: &str) -> Option<Vec<u8>> {
    if let Some(path) = url.strip_prefix("file://") {
        return fs::read(path).ok();
    }
    let mut h = DefaultHasher::new();
    url.hash(&mut h);
    let path = cache_dir().join(format!("{:016x}", h.finish()));
    if let Ok(bytes) = fs::read(&path) {
        return Some(bytes);
    }
    let resp = ureq::get(url).timeout(Duration::from_secs(5)).call().ok()?;
    let mut bytes = Vec::new();
    resp.into_reader()
        .take(20 << 20)
        .read_to_end(&mut bytes)
        .ok()?;
    let _ = fs::create_dir_all(cache_dir());
    let _ = fs::write(&path, &bytes);
    Some(bytes)
}

pub fn load(url: &str) -> Cover {
    let img = fetch(url)
        .and_then(|b| image::load_from_memory(&b).ok())
        .map(|i| i.to_rgb8());
    let accent = img.as_ref().map_or(DEFAULT_ACCENT, accent_from);
    Cover {
        url: url.to_string(),
        img,
        accent,
    }
}

fn accent_from(img: &RgbImage) -> Rgb {
    let small = image::imageops::resize(img, 32, 32, FilterType::Triangle);
    let mut buckets: HashMap<u16, (u32, [u32; 3])> = HashMap::new();
    for p in small.pixels() {
        let [r, g, b] = p.0;
        let key = ((r as u16 >> 4) << 8) | ((g as u16 >> 4) << 4) | (b as u16 >> 4);
        let e = buckets.entry(key).or_insert((0, [0; 3]));
        e.0 += 1;
        e.1[0] += r as u32;
        e.1[1] += g as u32;
        e.1[2] += b as u32;
    }
    let mut top: Vec<_> = buckets.into_values().collect();
    top.sort_by(|a, b| b.0.cmp(&a.0));
    top.truncate(8);

    let mut best = DEFAULT_ACCENT;
    let mut best_score = -1.0;
    for (count, sum) in top {
        let (r, g, b) = (
            (sum[0] / count) as u8,
            (sum[1] / count) as u8,
            (sum[2] / count) as u8,
        );
        let mx = r.max(g).max(b) as f32;
        let mn = r.min(g).min(b) as f32;
        let sat = if mx > 0.0 { (mx - mn) / mx } else { 0.0 };
        let score = sat * 2.0 + count as f32 / 1024.0;
        if score > best_score {
            best = (r, g, b);
            best_score = score;
        }
    }
    let mx = best.0.max(best.1).max(best.2).max(1) as f32;
    if mx < 170.0 {
        let f = 170.0 / mx;
        let lift = |c: u8| (c as f32 * f).min(255.0) as u8;
        best = (lift(best.0), lift(best.1), lift(best.2));
    }
    best
}

pub fn render_halfblock(img: &RgbImage, cols: u16, rows: u16) -> Vec<String> {
    if cols < 2 || rows < 1 {
        return Vec::new();
    }
    let px = image::imageops::resize(img, cols as u32, rows as u32 * 2, FilterType::Lanczos3);
    (0..rows as u32)
        .map(|y| {
            let mut line = String::new();
            for x in 0..cols as u32 {
                let t = px.get_pixel(x, y * 2).0;
                let b = px.get_pixel(x, y * 2 + 1).0;
                line.push_str(&format!(
                    "\x1b[38;2;{};{};{}m\x1b[48;2;{};{};{}m▀",
                    t[0], t[1], t[2], b[0], b[1], b[2]
                ));
            }
            line.push_str("\x1b[0m");
            line
        })
        .collect()
}

pub fn kitty_png_b64(img: &RgbImage, px: u32) -> String {
    let resized = image::imageops::resize(img, px, px, FilterType::Lanczos3);
    let mut png = Vec::new();
    let _ = resized.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png);
    base64::engine::general_purpose::STANDARD.encode(png)
}

pub fn kitty_place(b64: &str, cols: u16, rows: u16) -> String {
    let chunks: Vec<&[u8]> = b64.as_bytes().chunks(4096).collect();
    let mut out = String::new();
    for (i, chunk) in chunks.iter().enumerate() {
        let more = (i + 1 < chunks.len()) as u8;
        let data = std::str::from_utf8(chunk).unwrap_or("");
        if i == 0 {
            out.push_str(&format!(
                "\x1b_Gf=100,a=T,i=1,p=1,c={cols},r={rows},C=1,q=2,m={more};{data}\x1b\\"
            ));
        } else {
            out.push_str(&format!("\x1b_Gm={more};{data}\x1b\\"));
        }
    }
    out
}

pub const KITTY_CLEAR: &str = "\x1b_Ga=d,d=A,q=2\x1b\\";
