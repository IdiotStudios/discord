# Discord Bot

Note: This bot is more of for private use not for a whole community to be using unless your not worried about anyone being able to use the commands. A permission system will be added in the future

A Discord bot with written in Rust using Serenity. Features:

- Interactive music control panel with live updates (status, volume, remaining time, title/artist, thumbnail)
- Play music from search, YouTube, and (experimental) direct Spotify streaming
- Automatic helper setup for Spotify streaming (`.bin/librespot-wrapper` or compiled `librespot`)
- A Start Command to start any of your services easily via discord.

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
- Control panel message shows playback status and buttons for Pause/Resume/Stop/Vol+/Vol-

### Start command

- `start <service> [args]` — sends a POST to a configured service and reports the response.
   - Configuration file: `config.jsonc` at the project root (auto-created with defaults on first run).
   - Example (JSONC):

```json
{
   // Start command configuration
   "start": {
      "services": {
         "mc": {
            "url": "http://localhost:8080/start",
            "method": "POST",
            "headers": { "Content-Type": "application/json" },
            "body": { "action": "start" },
            "args_field": "args",
            "timeout_secs": 10
         }
      }
   }
}
```

- Usage in Discord: `!is start mc` or `!is start mc server-1` (the extra text is placed in the JSON body under the `args_field`, default `args`).

- NOTE: May have to rename this to something else and make multiple commands, an example would be !is put/post/get mc

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
