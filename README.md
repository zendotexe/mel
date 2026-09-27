# mel

A [kew](https://github.com/ravachol/kew)-style terminal remote for the Spotify desktop app.

Audio plays in Spotify, mel pulls it over MPRIS and shows the current track
with album-colored cover art, a band visualizer, a progress bar and custom keybindings

- **Hidden Spotify** – if Spotify isn't running, mel starts it on a hidden
  Hyprland special workspace (`special:mel`) and closes it again when mel exits.
  A Spotify that was already open is left alone.
- **Cover art** – full resolution via the kitty graphics protocol, truecolor
  half-blocks in other terminals. The accent colour is taken from the cover.
- **Visualizer** – captured from Spotify's own PipeWire stream, so other apps'
  audio doesn't move the bars.
- **Instant controls** – key presses update the screen immediately; changes
  made in Spotify itself are picked up from MPRIS signals.

## Keys

| Key | Action |
|---|---|
| `Space` / `p` | play / pause |
| `h` / `l` (or ← / →) | previous / next track |
| `a` / `d` | seek back / forward 5s |
| `+` `=` / `-` | volume up / down |
| `s` | shuffle |
| `r` | repeat: off → all → one |
| `S` | stop |
| `o` | show the Spotify window |
| `q` | quit |

## Requirements

- Linux with the Spotify desktop app
- `playerctl`
- PipeWire (`pw-record`, `pw-dump`) for the visualizer
- Hyprland (0.53+, Lua config) for the hidden-window behaviour; elsewhere
  Spotify is simply started normally
- `dbus-send` (for a fast, clean Spotify quit)

## Build

```sh
cargo build --release
install -m755 target/release/mel ~/.local/bin/mel
```

Set `MEL_PLAYER` to control a different MPRIS player (default: `spotify`).
