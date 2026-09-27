use std::fmt::Write as _;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::cover::{self, Cover, Rgb, DEFAULT_ACCENT};
use crate::mpris::State;
use crate::vis::{self, Visualizer};

const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";

pub const HELP: &str =
    "Space/p play · h/l prev/next · a/d seek · +/- vol · s shuffle · r repeat · o Spotify · q quit";

fn fg(c: Rgb) -> String {
    format!("\x1b[38;2;{};{};{}m", c.0, c.1, c.2)
}

fn goto(out: &mut String, row: u16, col: u16) {
    let _ = write!(out, "\x1b[{row};{col}H");
}

fn clip(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_string();
    }
    let mut out = String::new();
    let mut w = 0;
    for ch in text.chars() {
        let cw = ch.width().unwrap_or(0);
        if w + cw + 1 > width {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out.push('…');
    out
}

fn fmt_time(us: f64) -> String {
    let s = (us / 1_000_000.0).max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

fn wrap_help(width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for item in HELP.split(" · ") {
        let cand = if cur.is_empty() {
            item.to_string()
        } else {
            format!("{cur} · {item}")
        };
        if cand.width() <= width || cur.is_empty() {
            cur = cand;
        } else {
            lines.push(std::mem::replace(&mut cur, item.to_string()));
        }
    }
    lines.push(cur);
    lines.truncate(2);
    lines
}

#[derive(PartialEq, Clone)]
struct Placement {
    url: String,
    row: u16,
    col: u16,
    cols: u16,
    rows: u16,
    px: u32,
}

enum Row {
    Text(String),
    Image { left: u16, right: u16 },
}

pub struct Ui {
    kitty: bool,
    placed: Option<Placement>,
    kitty_cache: Option<(String, u32, String)>,
    half_cache: Option<(String, u16, u16, Vec<String>)>,
}

fn pad(col: u16) -> String {
    " ".repeat(col.saturating_sub(1) as usize)
}

impl Ui {
    pub fn new() -> Self {
        let kitty = std::env::var_os("KITTY_WINDOW_ID").is_some()
            || std::env::var("TERM").is_ok_and(|t| t == "xterm-kitty");
        Ui {
            kitty,
            placed: None,
            kitty_cache: None,
            half_cache: None,
        }
    }

    pub fn kitty(&self) -> bool {
        self.kitty
    }

    fn cell_size() -> (f32, f32) {
        match crossterm::terminal::window_size() {
            Ok(ws) if ws.width > 0 && ws.height > 0 && ws.columns > 0 && ws.rows > 0 => (
                ws.width as f32 / ws.columns as f32,
                ws.height as f32 / ws.rows as f32,
            ),
            _ => (8.0, 16.0),
        }
    }

    pub fn draw(
        &mut self,
        state: Option<&State>,
        cover: Option<&Cover>,
        message: Option<&str>,
        vis: &mut Visualizer,
        starting: bool,
    ) -> String {
        let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
        let mut frame: Vec<Row> = (0..rows).map(|_| Row::Text(String::new())).collect();
        let mut out = String::from("\x1b[?2026h");
        let mut placement: Option<Placement> = None;

        match state {
            None => {
                vis.update(0, false);
                let lines = [
                    (format!("{BOLD}{}mel{RESET}", fg(DEFAULT_ACCENT)), 3),
                    (String::new(), 0),
                    (
                        if starting {
                            "starting Spotify…"
                        } else {
                            "Spotify isn't running."
                        }
                        .to_string(),
                        if starting { 17 } else { 22 },
                    ),
                    (
                        format!(
                            "{DIM}{}{RESET}",
                            if starting {
                                "q to quit"
                            } else {
                                "press o to open it, q to quit"
                            }
                        ),
                        if starting { 9 } else { 29 },
                    ),
                ];
                let top = rows.saturating_sub(lines.len() as u16) / 2;
                for (i, (line, w)) in lines.iter().enumerate() {
                    if let Some(r) = frame.get_mut((top as usize) + i) {
                        *r = Row::Text(format!("{}{line}", pad(cols.saturating_sub(*w) / 2 + 1)));
                    }
                }
            }
            Some(state) => {
                placement = self.layout_player(&mut frame, state, cover, message, vis, cols, rows)
            }
        }

        if placement != self.placed {
            match &placement {
                Some(p) => {
                    let img = cover
                        .and_then(|c| c.img.as_ref())
                        .expect("placement implies an image");
                    let cached =
                        matches!(&self.kitty_cache, Some((u, px, _)) if *u == p.url && *px == p.px);
                    if !cached {
                        self.kitty_cache =
                            Some((p.url.clone(), p.px, cover::kitty_png_b64(img, p.px)));
                    }
                    goto(&mut out, p.row, p.col);
                    out.push_str(&cover::kitty_place(
                        &self.kitty_cache.as_ref().unwrap().2,
                        p.cols,
                        p.rows,
                    ));
                }
                None => out.push_str(cover::KITTY_CLEAR),
            }
            self.placed = placement;
        }

        for (i, row) in frame.iter().enumerate() {
            let r = i as u16 + 1;
            match row {
                Row::Text(text) => {
                    goto(&mut out, r, 1);
                    let _ = write!(out, "{text}{RESET}\x1b[K");
                }
                Row::Image { left, right } => {
                    goto(&mut out, r, 1);
                    out.push_str(&pad(*left));
                    if *right <= cols {
                        goto(&mut out, r, *right);
                        out.push_str("\x1b[K");
                    }
                }
            }
        }
        out.push_str("\x1b[?2026l");
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn layout_player(
        &mut self,
        frame: &mut [Row],
        state: &State,
        cover: Option<&Cover>,
        message: Option<&str>,
        vis: &mut Visualizer,
        cols: u16,
        rows: u16,
    ) -> Option<Placement> {
        let accent = cover.map_or(DEFAULT_ACCENT, |c| c.accent);
        let img = cover.and_then(|c| c.img.as_ref());

        let text_rows = 1 + 3 + 1 + vis::HEIGHT + 1 + 1 + 1;
        let avail = rows.saturating_sub(text_rows + 4);
        let mut cover_rows = ((avail as f32 * 0.6) as u16).min(cols.saturating_sub(4) / 2);
        let mut cover_cols = cover_rows * 2;
        if self.kitty {
            let (cw, ch) = Self::cell_size();
            cover_cols = (cover_rows as f32 * ch / cw).round() as u16;
            if cover_cols > cols.saturating_sub(4) {
                cover_cols = cols.saturating_sub(4);
                cover_rows = (cover_cols as f32 * cw / ch).round() as u16;
            }
        }
        let width = cols.saturating_sub(4).min(cover_cols.max(50)).max(10);
        let left = cols.saturating_sub(width) / 2 + 1;
        let top = rows.saturating_sub(1 + cover_rows + text_rows) / 2 + 1;
        let cover_left = cols.saturating_sub(cover_cols) / 2 + 1;

        let mut placement = None;
        if let (Some(img), Some(c)) = (img, cover) {
            if cover_rows > 0 {
                if self.kitty {
                    let px = ((cover_rows as f32 * Self::cell_size().1) as u32)
                        .min(img.width().max(img.height()));
                    placement = Some(Placement {
                        url: c.url.clone(),
                        row: top,
                        col: cover_left,
                        cols: cover_cols,
                        rows: cover_rows,
                        px,
                    });
                    for r in top..top + cover_rows {
                        if let Some(slot) = frame.get_mut(r as usize - 1) {
                            *slot = Row::Image {
                                left: cover_left,
                                right: cover_left + cover_cols,
                            };
                        }
                    }
                } else {
                    let cached = matches!(&self.half_cache, Some((u, w, h, _)) if *u == c.url && *w == cover_cols && *h == cover_rows);
                    if !cached {
                        self.half_cache = Some((
                            c.url.clone(),
                            cover_cols,
                            cover_rows,
                            cover::render_halfblock(img, cover_cols, cover_rows),
                        ));
                    }
                    for (i, line) in self.half_cache.as_ref().unwrap().3.iter().enumerate() {
                        if let Some(slot) = frame.get_mut((top as usize - 1) + i) {
                            *slot = Row::Text(format!("{}{line}", pad(cover_left)));
                        }
                    }
                }
            }
        }
        let mut set = |row: u16, text: String| {
            if let Some(r) = frame.get_mut(row as usize - 1) {
                *r = Row::Text(text);
            }
        };
        let mut row = top + cover_rows + 1;

        let w = width as usize;
        let lp = pad(left);
        let title = if state.title.is_empty() {
            "Nothing playing"
        } else {
            &state.title
        };
        set(
            row,
            format!("{lp}{BOLD}{}{}{RESET}", fg(accent), clip(title, w)),
        );
        set(row + 1, format!("{lp}{}", clip(&state.artist, w)));
        set(
            row + 2,
            format!("{lp}{DIM}{}{RESET}", clip(&state.album, w)),
        );
        row += 4;

        let nbars = ((width + 1) / (vis::BAR_WIDTH + 1)) as usize;
        let levels = vis.update(nbars, state.playing());
        let bars_width = (nbars as u16 * (vis::BAR_WIDTH + 1)).saturating_sub(1);
        let vis_pad = pad(left + width.saturating_sub(bars_width) / 2);
        for (i, line) in vis::render(levels, accent, vis::HEIGHT)
            .into_iter()
            .enumerate()
        {
            set(row + i as u16, format!("{vis_pad}{line}"));
        }
        row += vis::HEIGHT;

        let length = state.length_us;
        let pos = state.position_now();
        let times = format!("{} / {}", fmt_time(pos), fmt_time(length));
        let bar_w = (w.saturating_sub(times.len() + 1)).max(5);
        let done = if length > 0.0 {
            ((bar_w as f64 * pos / length) as usize).min(bar_w)
        } else {
            0
        };
        set(
            row,
            format!(
                "{lp}{}{}{RESET}{DIM}{}{RESET} {times}",
                fg(accent),
                "━".repeat(done),
                "━".repeat(bar_w - done)
            ),
        );
        row += 2;

        let icon = match state.status.as_str() {
            "Playing" => "",
            "Paused" => "",
            "Stopped" => "",
            _ => "",
        };
        let repeat = match state.loop_status.as_str() {
            "Track" => "repeat one",
            "Playlist" => "repeat all",
            _ => "repeat off",
        };
        let lit = |on: bool, text: &str| {
            if on {
                format!("{}{text}{RESET}", fg(accent))
            } else {
                format!("{DIM}{text}{RESET}")
            }
        };
        let parts = [
            format!("{}{icon} {}{RESET}", fg(accent), state.status),
            format!("vol {}%", (state.volume * 100.0).round() as i64),
            lit(
                state.shuffle,
                if state.shuffle {
                    "shuffle on"
                } else {
                    "shuffle off"
                },
            ),
            lit(state.loop_status != "None", repeat),
        ];
        set(row, format!("{lp}{}", parts.join("   ")));

        let footer: Vec<String> = match message {
            Some(m) => vec![m.to_string()],
            None => wrap_help(cols.saturating_sub(2) as usize),
        };
        for (i, line) in footer.iter().enumerate() {
            let r = rows + 1 + i as u16 - footer.len() as u16;
            let line = clip(line, cols.saturating_sub(2) as usize);
            set(
                r,
                format!(
                    "{}{DIM}{line}{RESET}",
                    pad((cols as usize).saturating_sub(line.width()) as u16 / 2 + 1)
                ),
            );
        }
        placement
    }
}
