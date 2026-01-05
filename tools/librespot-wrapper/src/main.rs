use anyhow::{Context, Result};
use clap::Parser;
use reqwest::Client;
use serde::Deserialize;
use std::env;

#[derive(Parser, Debug)]
#[command(author, version, about = "librespot-wrapper: convenience helper to play a Spotify URI and stream audio to stdout (WIP)")]
struct Args {
    /// Spotify URI to play (e.g., spotify:track:... or open.spotify.com link)
    #[arg(long)]
    uri: Option<String>,

    /// Write raw WAV to stdout (when implemented)
    #[arg(long)]
    stdout: bool,

    /// Device name to register as (defaults to 'Librespot-Wrapper')
    #[arg(long, default_value = "Librespot-Wrapper")]
    name: String,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    token_type: String,
    expires_in: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Load config from env
    let client_id = env::var("SPOTIFY_CLIENT_ID").ok();
    let client_secret = env::var("SPOTIFY_CLIENT_SECRET").ok();
    let refresh_token = env::var("SPOTIFY_REFRESH_TOKEN").ok();

    if refresh_token.is_none() || client_id.is_none() || client_secret.is_none() {
        eprintln!("Missing SPOTIFY_CLIENT_ID, SPOTIFY_CLIENT_SECRET, or SPOTIFY_REFRESH_TOKEN in env.");
        eprintln!("This tool will attempt to control playback on a librespot device via the Web API.");
        eprintln!("See tools/librespot-wrapper/README.md for instructions to obtain a refresh token.");
        anyhow::bail!("missing Spotify credentials");
    }

    let client = Client::new();

    // Ensure URI present
    let uri_owned = args.uri.as_ref().ok_or_else(|| anyhow::anyhow!("You must pass --uri <spotify:track:... or open.spotify.com/track/..."))?;

    // Exchange refresh token for access token using the client credentials
    let token = refresh_access_token(&client, &client_id.unwrap(), &client_secret.unwrap(), &refresh_token.unwrap())
        .await
        .context("failed to refresh access token")?;

    // If stdout mode requested, set up a FIFO and spawn librespot in pipe backend so we can capture audio
    let mut librespot_child = None;
    let mut fifo_path_opt = None;

    if args.stdout {
        // Prepare a FIFO in the temp dir
        let tmpdir = std::env::temp_dir();
        let uniq = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
        let fifo_path = tmpdir.join(format!("librespot-pipe-{}", uniq));

        // Create FIFO using mkfifo
        let mk = std::process::Command::new("mkfifo").arg(&fifo_path).status();
        match mk {
            Ok(s) if s.success() => {
                eprintln!("Created FIFO at {}", fifo_path.display());
            }
            Ok(s) => {
                eprintln!("mkfifo returned non-zero: {:?}", s);
                anyhow::bail!("failed to create fifo");
            }
            Err(e) => {
                eprintln!("mkfifo error: {e:?}");
                anyhow::bail!("mkfifo failed");
            }
        }

        // Find librespot binary (prefer our built pipe-enabled binary, then the wrapper, then 'librespot')
        let librespot_bin = if std::path::Path::new(".bin/librespot-pipe").is_file() {
            ".bin/librespot-pipe".to_string()
        } else if std::path::Path::new(".bin/librespot-wrapper").is_file() {
            ".bin/librespot-wrapper".to_string()
        } else {
            "librespot".to_string()
        };

        // Build librespot args: use '--device' to point to FIFO and pass the access token if available
        let mut ls_args: Vec<String> = vec!["--name".into(), args.name.clone(), "--backend".into(), "pipe".into(), "--device".into(), fifo_path.to_string_lossy().to_string(), "--format".into(), "S16".into()];

        // Prefer passing an OAuth access token rather than username/password
        ls_args.push("--access-token".into());
        ls_args.push(token.access_token.clone());

        eprintln!("Spawning librespot: {} {:?}", librespot_bin, ls_args);
        let mut cmd = tokio::process::Command::new(&librespot_bin);
        for a in ls_args.iter() { cmd.arg(a); }
        cmd.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::piped());

        match cmd.spawn() {
            Ok(child) => {
                eprintln!("librespot started (pid {:?}). Waiting for device to appear...", child.id());
                librespot_child = Some(child);
                fifo_path_opt = Some(fifo_path.clone());
            }
            Err(e) => {
                eprintln!("Failed to start librespot: {e:?}");
                anyhow::bail!("failed to start librespot");
            }
        }

        // Wait for device to appear (poll)
        let mut dev_id = None;
        for _ in 0..20 {
            if let Ok(Some(did)) = find_device_by_name(&client, &token.access_token, &args.name).await {
                dev_id = Some(did); break;
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }

        if dev_id.is_none() {
            eprintln!("Device didn't appear in time");
            anyhow::bail!("device not ready");
        }

        let dev = dev_id.unwrap();

        // Request playback on that device
        let test_uri = args.uri.as_deref().unwrap_or("");
        // Sanity check dev type
        let _: &String = &dev;
        let url = format!("https://api.spotify.com/v1/me/player/play?device_id={}", dev);
        let body = serde_json::json!({ "uris": [ test_uri ] });
        let _ = client
            .put(&url)
            .bearer_auth(&token.access_token)
            .json(&body)
            .send()
            .await?
            .error_for_status()?;

        // Spawn ffmpeg to read from FIFO and write WAV to stdout
        let ff_cmd = format!("ffmpeg -hide_banner -loglevel error -f s16le -ar 48000 -ac 2 -i {} -f wav -", fifo_path.to_string_lossy());
        eprintln!("Spawning ffmpeg: {}", ff_cmd);
        let mut ff = tokio::process::Command::new("sh");
        ff.arg("-c").arg(&ff_cmd);
        ff.stdout(std::process::Stdio::inherit()); // write to our stdout
        ff.stderr(std::process::Stdio::piped());

        let mut ff_child = ff.spawn().context("failed to spawn ffmpeg")?;

        // Wait for ffmpeg to exit (or return immediately if ffmpeg runs until killed)
        let status = ff_child.wait().await.context("ffmpeg wait failed")?;
        eprintln!("ffmpeg exited with: {:?}", status);

        // Clean up fifo
        if let Some(fp) = fifo_path_opt {
            let _ = std::fs::remove_file(&fp);
        }

        // If we reach here, streaming ended
        println!("Streaming finished");
        return Ok(());
    }

    // Otherwise: non-stdout mode -> find a device and start playback normally
    let device_id = find_device_by_name(&client, &token.access_token, &args.name).await?;

    if device_id.is_none() {
        eprintln!("No device named '{}' found for the Spotify account. Start a librespot device with that name and try again.", args.name);
        anyhow::bail!("device not found");
    }

    let dev = device_id.unwrap();

    // Request playback on that device
    let url = format!("https://api.spotify.com/v1/me/player/play?device_id={}", dev);
    let body = serde_json::json!({ "uris": [ args.uri.as_deref().unwrap_or("") ] });
    let _ = client
        .put(&url)
        .bearer_auth(&token.access_token)
        .json(&body)
        .send()
        .await?
        .error_for_status()?;

    println!("Requested playback of {} on device {}", args.uri.as_deref().unwrap_or(""), dev);

    Ok(())
}

async fn refresh_access_token(client: &Client, client_id: &str, client_secret: &str, refresh_token: &str) -> Result<TokenResponse> {
    let body = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
    ];

    let res = client
        .post("https://accounts.spotify.com/api/token")
        .basic_auth(client_id, Some(client_secret))
        .form(&body)
        .send()
        .await?
        .error_for_status()?;

    let tr: TokenResponse = res.json().await?;
    Ok(tr)
}

async fn find_device_by_name(client: &Client, access_token: &str, name: &str) -> Result<Option<String>> {
    // GET https://api.spotify.com/v1/me/player/devices
    #[derive(Deserialize)]
    struct Devices { devices: Vec<Device> }
    #[derive(Deserialize)]
    struct Device { id: String, name: String }

    let res = client
        .get("https://api.spotify.com/v1/me/player/devices")
        .bearer_auth(access_token)
        .send()
        .await?
        .error_for_status()?;

    let devs: Devices = res.json().await?;
    for d in devs.devices.into_iter() {
        if d.name == name {
            return Ok(Some(d.id));
        }
    }
    Ok(None)
}

async fn start_playback(client: &Client, access_token: &str, device_id: &str, uri: &str) -> Result<()> {
    // PUT https://api.spotify.com/v1/me/player/play?device_id={device_id}
    let url = format!("https://api.spotify.com/v1/me/player/play?device_id={}", device_id);
    let body = serde_json::json!({ "uris": [ uri ] });

    let _ = client
        .put(&url)
        .bearer_auth(access_token)
        .json(&body)
        .send()
        .await?
        .error_for_status()?;

    Ok(())
}
