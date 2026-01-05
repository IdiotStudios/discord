# Discord Music Bot

A Discord music bot with Spotify and YouTube support, written in Rust using Serenity and Songbird. Features:

- Interactive control panel with live updates (status, volume, remaining time, title/artist, thumbnail)
- Play from search, YouTube, and (experimental) direct Spotify streaming
- Automatic helper setup for Spotify streaming (`.bin/librespot-wrapper` or compiled `librespot`)
- `streamtest` command to record and inspect helper/ffmpeg output for diagnostics

---

## Quick start

Prerequisites:
- Rust toolchain (for building in-repo helper) or prebuilt binaries
- `ffmpeg` and `yt-dlp` available on PATH
- A Spotify account (Premium required for Connect playback) Youtube will fallback if you dont have

1. Copy `.env.example` to `.env` and fill in values:
   - `DISCORD_TOKEN` (required)
   - `SPOTIFY_CLIENT_ID`, `SPOTIFY_CLIENT_SECRET` (for metadata and token exchange)
   - `SPOTIFY_REFRESH_TOKEN` (use `scripts/get_spotify_refresh_token.sh` to obtain)
   - `SPOTIFY_STREAM_CMD` (optional override command template; use `{uri}` placeholder)
   - `SPOTIFY_PREFER_YOUTUBE=1` (optional: force YouTube fallback for Spotify links)

2. Run the setup script to fetch or build helper binaries:

```bash
./scripts/setup.sh
```

3. Run the bot:

```bash
cargo run
```

## Auth helper

To obtain a Spotify refresh token, run:

```bash
./scripts/get_spotify_refresh_token.sh
```

Follow the browser prompts and paste the returned refresh token into `.env` as `SPOTIFY_REFRESH_TOKEN`.

## Commands

- `music play <query|url>` — play a track or search query.
- `music streamtest <spotify-url>` — record 10s sample from the configured Spotify helper and run `ffprobe` for diagnostics.
- Control panel message shows playback status and buttons for Pause/Resume/Stop/Vol+/Vol-

## Troubleshooting

- Invalid refresh token: re-run the auth helper and update `.env`.
- `PREMIUM_REQUIRED`: Spotify Connect playback requires a Spotify Premium account.
- `no suitable format reader`: helper may not output a probeable container; enable `SPOTIFY_PREFER_YOUTUBE=1`.
- Check `MUSIC_VERBOSE=1` and generated logs in the working directory for ffmpeg/helper stderr output.

## Contributing

Open pull requests with changes and tests. For helper builds and OS packaging, see `scripts/setup.sh` for build steps.

---

## License

This project is licensed under the MIT License - see the `LICENSE` file for details.
