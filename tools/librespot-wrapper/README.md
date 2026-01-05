librespot-wrapper (WIP)

This tool aims to provide a small helper that can be used by the bot to play a Spotify URI and optionally stream the resulting audio to stdout.

Current behavior (v0.1.0):
- Exchanges `SPOTIFY_REFRESH_TOKEN` + `SPOTIFY_CLIENT_ID`/`SPOTIFY_CLIENT_SECRET` for an access token
- Finds a device with name configured via `--name` (default: `Librespot-Wrapper`) using the Spotify Web API
- Requests playback of the provided `--uri` on that device
- (WIP) streaming of PCM/WAV to stdout is a planned feature — right now the helper will only request playback on the device

How to use (manual steps):
1) Ensure `SPOTIFY_CLIENT_ID`, `SPOTIFY_CLIENT_SECRET`, and `SPOTIFY_REFRESH_TOKEN` are set in your environment.  The Authorization Code flow is required to obtain a refresh token — see Spotify docs.
2) Start a librespot device with a known name (e.g., run your built librespot binary with `--name Librespot-Wrapper` and any needed credentials).
3) Run the helper:
   ./librespot-wrapper --uri spotify:track:<ID> --stdout

Next work (to implement):
- Capture librespot playback output (via a pipe backend, in-process audio sink or other), transcode to WAV and write to stdout
- Add an interactive `login` or `auth` command to guide the user through getting a refresh token
- Build prebuilt release artifacts and add CI to publish them
