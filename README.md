# mel

A [kew](https://github.com/ravachol/kew)-style terminal remote for the Spotify desktop app.

Plays playlists and songs from Spotify with album-colored text, a band visualizer, a progress bar and custom keybindings

- **Hidden Spotify** – if Spotify isn't running, mel starts it on a hidden Hyprland workspace (Niri support coming soon tm) and closes it again when mel exits
- **Cover art** – full resolution via the kitty graphics protocol, truecolor blocks in other terminals 
  The accent colour is taken from the cover.
- **Visualizer** – captured directly from Spotify's 
- **Instant controls** – sends commands to spotify to update
## Playlists

`mel "some playlist"` fuzzy-finds a playlist by name (substring match first,
then subsequence) and starts playing the first one that matches 
This needs a Spotify Web API, you can follow this guide:

1. Create an app at https://developer.spotify.com/dashboard (any name).
2. Add `http://127.0.0.1:8942/callback` as a Redirect URI in the app's settings.
3. Paste the Client ID when mel prompts for it (or set `MEL_SPOTIFY_CLIENT_ID`).

mel then opens your browser for a one-time login and makes a refresh token in `~/.config/mel/`.

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
| `o` | show/hide the Spotify window |
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
