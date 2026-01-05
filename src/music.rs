use base64::engine::general_purpose::STANDARD as B64_ENGINE;
use base64::Engine;
use reqwest::Client;
use serde::Deserialize;
use serenity::{
    builder::{CreateEmbed, CreateMessage},
    model::prelude::*,
    prelude::*,
};
use std::env;
use tokio::fs;
use std::path::PathBuf;
use serenity::async_trait;

type MusicResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

async fn store_handle(ctx: &Context, guild_id: GuildId, handle: songbird::tracks::TrackHandle) -> Result<(), ()> {
    let maybe_store = ctx.data.read().await.get::<crate::TrackStore>().cloned();
    if let Some(store) = maybe_store {
        let mut map = store.lock().await;
        map.insert(guild_id, handle);
        Ok(())
    } else {
        Err(())
    }
}

#[derive(Deserialize)]
struct SpotifyToken {
    access_token: String,
}

#[derive(Deserialize)]
struct SpotifySearch {
    tracks: SpotifyTracks,
}

#[derive(Deserialize)]
struct SpotifyTracks {
    items: Vec<SpotifyTrack>,
}

#[derive(Deserialize)]
struct SpotifyTrack {
    name: String,
    artists: Vec<SpotifyArtist>,
}

#[derive(Deserialize)]
struct SpotifyArtist {
    name: String,
}

pub async fn handle_music(
    ctx: &Context,
    channel: ChannelId,
    user_voice: Option<ChannelId>,
    user_id: UserId,
    guild_id: Option<GuildId>,
    args: &str,
    embed_color: u32,
) -> serenity::Result<()> {
    let mut parts = args.split_whitespace();
    let sub = parts.next().unwrap_or("");
    let remainder = parts.collect::<Vec<_>>().join(" ");

    let result: MusicResult<()> = match sub {
        "join" => join(ctx, channel, user_voice, user_id, guild_id, &remainder, embed_color).await,
        "leave" => leave(ctx, channel, user_id, guild_id, embed_color).await,
        "play" => play(ctx, channel, user_id, guild_id, &remainder, embed_color).await,
        "streamtest" => streamtest(ctx, channel, guild_id, &remainder, embed_color).await,
        "control" => {
            if let Some(gid) = guild_id {
                if let Err(e) = send_control_panel(ctx, channel, user_id, gid, embed_color).await {
                    eprintln!("Failed to send control panel: {e:?}");
                }
                Ok(())
            } else {
                send_info(ctx, channel, embed_color, "Music", "Controls only available in a guild").await
            }
        }
        _ => send_info(ctx, channel, embed_color, "Music", "Subcommands: join, play <song>, leave, control").await,
    };

    if let Err(err) = result {
        eprintln!("Music command error: {err:?}");
        let _ = send_info(ctx, channel, embed_color, "Music Error", &format!("{err}"),).await;
    }

    Ok(())
}

pub async fn ensure_media_tools() -> MusicResult<()> {
    const BIN_DIR: &str = ".bin";
    const YTDLP_BIN: &str = "yt-dlp";
    const YTDLP_URL: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp";

    let ytdlp_path = PathBuf::from(BIN_DIR).join(YTDLP_BIN);

    if fs::metadata(&ytdlp_path).await.is_err() {
        fs::create_dir_all(BIN_DIR).await?;
        let bytes = Client::new()
            .get(YTDLP_URL)
            .send()
            .await?
            .error_for_status()?;
        let content = bytes.bytes().await?;
        fs::write(&ytdlp_path, &content).await?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&ytdlp_path).await?.permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&ytdlp_path, perms).await?;
        }
    }

    // Verify ffmpeg is available on PATH — log a warning if not
    match tokio::process::Command::new("ffmpeg").arg("-version").output().await {
        Ok(o) if o.status.success() => {
            println!("ffmpeg found");
        }
        Ok(o) => {
            eprintln!("ffmpeg exists but failed to run: {}", String::from_utf8_lossy(&o.stderr));
        }
        Err(_) => {
            eprintln!("Warning: ffmpeg not found on PATH. Playback may fail.");
        }
    }

    prepend_path(BIN_DIR)?;
    Ok(())
}

/// Ensure an optional Spotify stream helper binary is present in `.bin/librespot-wrapper`.
/// The downloader will attempt to fetch the URL from `SPOTIFY_WRAPPER_URL` if set.
pub async fn ensure_spotify_helper() -> MusicResult<()> {
    const BIN_DIR: &str = ".bin";
    const WRAPPER_BIN: &str = "librespot-wrapper";

    let wrapper_path = PathBuf::from(BIN_DIR).join(WRAPPER_BIN);

    // If the wrapper already exists, nothing to do
    if fs::metadata(&wrapper_path).await.is_ok() {
        return Ok(());
    }

    // Check for SPOTIFY_WRAPPER_URL env var to download a prebuilt helper
    if let Ok(url) = std::env::var("SPOTIFY_WRAPPER_URL") {
        fs::create_dir_all(BIN_DIR).await?;
        eprintln!("Downloading Spotify helper from {}", url);
        let bytes = Client::new().get(&url).send().await?.error_for_status()?;
        let content = bytes.bytes().await?;
        fs::write(&wrapper_path, &content).await?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&wrapper_path).await?.permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&wrapper_path, perms).await?;
        }

        prepend_path(BIN_DIR)?;
        println!("Downloaded Spotify helper to {}", wrapper_path.display());
        Ok(())
    } else {
        // No auto-download URL provided — leave an example wrapper behind so users can configure one
        let example_path = PathBuf::from(BIN_DIR).join(format!("{}.example", WRAPPER_BIN));
        if fs::metadata(&example_path).await.is_err() {
            let example_script = include_str!("../.bin/librespot-wrapper.example");
            fs::create_dir_all(BIN_DIR).await?;
            fs::write(&example_path, example_script).await?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = fs::metadata(&example_path).await?.permissions();
                perms.set_mode(0o644);
                fs::set_permissions(&example_path, perms).await?;
            }
            eprintln!("Wrote example Spotify helper to {}. To enable auto-download, set SPOTIFY_WRAPPER_URL to a prebuilt binary URL.", example_path.display());
        }
        Ok(())
    }
}

async fn join(ctx: &Context, channel: ChannelId, user_voice: Option<ChannelId>, user_id: UserId, guild_id: Option<GuildId>, args: &str, color: u32) -> MusicResult<()> {
    let guild_id = guild_id.ok_or("This command only works in a guild")?;

    // Allow optional channel id argument: "music join <channel>". Priority: explicit arg -> provided user_voice
    let mut channel_id = args
        .split_whitespace()
        .next()
        .and_then(|s| s.trim().trim_start_matches("<#").trim_end_matches('>').parse::<u64>().ok())
        .map(ChannelId::from);

    if let Some(guild) = ctx.cache.guild(guild_id) {
      eprintln!("Voice states:");
      for (uid, vs) in &guild.voice_states {
        eprintln!("user={} channel={:?}", uid.get(), vs.channel_id);
      }
    } else {
      eprintln!("Guild not in cache");
    }


    // If no explicit arg, try to detect user's voice channel from cache first
    if channel_id.is_none() {
        if let Some(v) = voice_channel_for_user_id(ctx, guild_id, user_id) {
            channel_id = Some(v);
            eprintln!("Detected user voice channel from cache: {:?}", v);
        } else {
            // fallback to the precomputed user_voice (from message handler)
            channel_id = user_voice;
        }
    }

    // Inform the user which voice channel we will join (ephemeral-like): auto-delete after a few seconds
    if let Some(cid) = channel_id {
        let notice = format!("Joining <#{}> (requested by <@{}>)", cid.get(), user_id);
        let _ = send_temp_info(ctx.clone(), channel, &notice).await;
    }

    let channel_id = match channel_id {
        Some(cid) => cid,
        None => {
            // Provide a simple diagnostic without needing cache access
            let _ = send_info(
                ctx,
                channel,
                color,
                "Music",
                "Couldn't determine your voice channel. Join a voice channel or provide channel id: is; music join <channel>",
            )
            .await;

            return Err("Couldn't determine voice channel".into());
        }
    };

    let manager = songbird::get(ctx)
        .await
        .ok_or("Songbird Voice client placed in at initialisation.")?
        .clone();

    let _handler = manager.join(guild_id, channel_id).await?;

    send_info(
        ctx,
        channel,
        color,
        "Music",
        &format!("Joined <#{}>", channel_id.get()),
    )
    .await?;

    Ok(())
}

async fn leave(ctx: &Context, channel: ChannelId, _user_id: UserId, guild_id: Option<GuildId>, color: u32) -> MusicResult<()> {
    let guild_id = guild_id.ok_or("This command only works in a guild")?;
    let manager = songbird::get(ctx)
        .await
        .ok_or("Songbird Voice client placed in at initialisation.")?
        .clone();

    if manager.get(guild_id).is_none() {
        send_info(ctx, channel, color, "Music", "Not connected to a voice channel").await?;
        return Ok(());
    }

    manager.remove(guild_id).await?;

    send_info(ctx, channel, color, "Music", "Left the voice channel").await?;
    Ok(())
}

async fn play(ctx: &Context, channel: ChannelId, _user_id: UserId, guild_id: Option<GuildId>, query: &str, color: u32) -> MusicResult<()> {
    let guild_id = guild_id.ok_or("This command only works in a guild")?;
    if query.trim().is_empty() {
        send_info(ctx, channel, color, "Music", "Provide a song name: music play <song>").await?;
        return Ok(());
    }

    let manager = songbird::get(ctx)
        .await
        .ok_or("Songbird Voice client placed in at initialisation.")?
        .clone();

    let handler_lock = if let Some(lock) = manager.get(guild_id) {
        lock
    } else {
        send_info(ctx, channel, color, "Music", "Bot is not in a voice channel (use music join)").await?;
        return Ok(());
    };

    // Support direct URLs: YouTube links will be played directly; Spotify track links will be resolved via the Spotify Web API and then searched on YouTube
    let raw_query = query.trim().to_string();
    let mut search_query = raw_query.clone();

    // If it's a Spotify link, try to resolve it to a title+artist using the Spotify API
    if raw_query.starts_with("http") && raw_query.contains("spotify") {
        if let Some(id) = parse_spotify_track_id(&raw_query) {
            if let Ok(token) = fetch_spotify_token_from_env().await {
                if let Ok(Some((title, artist, duration_opt, thumbnail_opt))) = fetch_spotify_track_by_id(&token.access_token, &id).await {
                    // Use the Spotify metadata to search YouTube and store metadata in TrackMetaStore
                    search_query = format!("{} {}", title, artist);

                    if let Some(ms) = ctx.data.read().await.get::<crate::TrackMetaStore>().cloned() {
                        let mut mm = ms.lock().await;
                        mm.insert(guild_id, crate::TrackMeta { title: Some(title.clone()), artist: Some(artist.clone()), duration: duration_opt, thumbnail: thumbnail_opt.clone() });
                    }


                }
            }
        }
    } else {
        // Not a Spotify link — perform the existing 'spotify-first' lookup for plain queries
        search_query = match spotify_first_then_query(query).await {
            Ok(Some(s)) => s,
            Ok(None) => query.to_string(),
            Err(e) => {
                eprintln!("Spotify lookup failed, falling back to direct search: {e:?}");
                query.to_string()
            }
        };
    }

    // Use Songbird's YoutubeDl lazy input to resolve and play the query
    let req_client = Client::builder().build()?;
    let http_client = req_client.clone();

    // If the user provided a YouTube URL directly, play that URL; otherwise use a search
    let mut ytdl = if raw_query.starts_with("http") && (raw_query.contains("youtube.com") || raw_query.contains("youtu.be")) {
        songbird::input::YoutubeDl::new(req_client, raw_query.clone())
            .user_args(vec!["-f".into(), "bestaudio[ext=webm]/bestaudio/best".into()])
    } else {
        songbird::input::YoutubeDl::new_search(req_client, search_query.clone())
            .user_args(vec!["-f".into(), "bestaudio[ext=webm]/bestaudio/best".into()])
    };
    let input: songbird::input::Input = ytdl.clone().into();

    let mut handler = handler_lock.lock().await;

    // If a Spotify link is provided, try streaming directly via a configured command or a bundled `.bin` helper; otherwise fall back to YouTube search
    if raw_query.starts_with("http") && raw_query.contains("spotify") {
        // Allow opting out of direct Spotify streaming and force the YouTube fallback
        let prefer_youtube = std::env::var("SPOTIFY_PREFER_YOUTUBE").map(|s| matches!(s.as_str(), "1" | "true" | "TRUE" | "True")).unwrap_or(false);
        if prefer_youtube {
            let _ = send_info(ctx, channel, color, "Music", "Spotify direct streaming disabled by `SPOTIFY_PREFER_YOUTUBE`; falling back to YouTube search").await;
        } else if let Some(cmd) = get_spotify_stream_cmd(&raw_query) {
            // Spawn via shell so users can compose pipelines; expect the command to write raw PCM/WAV to stdout
            match std::process::Command::new("sh").arg("-c").arg(&cmd).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn() {
                Ok(child_proc) => {
                    // First attempt: try to play the raw child output directly
                    let container = songbird::input::ChildContainer::from(child_proc);
                    let child_input: songbird::input::Input = container.into();
                    let new_handle = handler.play_input(child_input);

                    match new_handle.make_playable_async().await {
                        Ok(()) => {
                            let _ = new_handle.play();
                            let _ = new_handle.set_volume(0.20);
                            let gid = guild_id;
                            let _ = store_handle(ctx, gid, new_handle.clone()).await;

                            let _ = send_info(
                                ctx,
                                channel,
                                color,
                                "Music",
                                &format!("Now streaming from Spotify: {}", raw_query),
                            )
                            .await?;

                            return Ok(());
                        }
                        Err(e) => {
                            eprintln!("Initial spotify stream parse failed: {e:?}; attempting ffmpeg transcode fallback");

                            // Try several common input hints to ffmpeg to handle helpers that emit raw PCM, WAV, MP3, or Opus
                            let input_formats = [
                                "",                    // let ffmpeg probe
                                "-f wav",             // WAV container
                                "-f s16le -ar 44100 -ac 2", // raw signed 16-bit PCM 44.1kHz stereo
                                "-f s16le -ar 48000 -ac 2", // raw signed 16-bit PCM 48kHz stereo
                                "-f mp3",
                                "-f opus",
                            ];

                            // Collect stderr logs for diagnostics
                            let mut stderr_logs: Vec<String> = Vec::new();

                            for fmt in &input_formats {
                                let ff_cmd = if fmt.is_empty() {
                                    format!("{cmd} | ffmpeg -hide_banner -loglevel error -i - -vn -c:a pcm_s16le -ar 48000 -ac 2 -f wav -", cmd = cmd)
                                } else {
                                    format!("{cmd} | ffmpeg -hide_banner -loglevel error {fmt} -i - -vn -c:a pcm_s16le -ar 48000 -ac 2 -f wav -", cmd = cmd, fmt = fmt)
                                };

                                match std::process::Command::new("sh").arg("-c").arg(&ff_cmd).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn() {
                                    Ok(mut child_proc2) => {
                                        // Prepare a stderr file to capture ffmpeg diagnostics
                                        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                                        let uniq = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
                                        let stderr_log = cwd.join(format!("spotify-{}-ffstderr-{}.log", std::process::id(), uniq));

                                        if let Some(mut stderr) = child_proc2.stderr.take() {
                                            let stderr_log_clone = stderr_log.clone();
                                            std::thread::spawn(move || {
                                                use std::io::Read;
                                                let mut buf = String::new();
                                                let _ = stderr.read_to_string(&mut buf);
                                                let _ = std::fs::write(&stderr_log_clone, &buf);
                                            });
                                        }

                                        let container2 = songbird::input::ChildContainer::from(child_proc2);
                                        let child_input2: songbird::input::Input = container2.into();
                                        let new_handle2 = handler.play_input(child_input2);

                                        match new_handle2.make_playable_async().await {
                                            Ok(()) => {
                                                let _ = new_handle2.play();
                                                let _ = new_handle2.set_volume(0.20);
                                                let gid = guild_id;
                                                let _ = store_handle(ctx, gid, new_handle2.clone()).await;

                                                let _ = send_info(
                                                    ctx,
                                                    channel,
                                                    color,
                                                    "Music",
                                                    &format!("Now streaming from Spotify (transcoded, fmt='{}'): {}", fmt, raw_query),
                                                )
                                                .await?;

                                                return Ok(());
                                            }
                                            Err(e2) => {
                                                eprintln!("Transcoded spotify stream (fmt='{}') failed to play: {e2:?}", fmt);

                                                // Read stderr log (if present) for diagnostics and append
                                                if let Ok(s) = tokio::fs::read_to_string(&stderr_log).await {
                                                    if !s.is_empty() {
                                                        stderr_logs.push(format!("fmt='{}' stderr:\n{}", fmt, s));
                                                        let _ = tokio::fs::remove_file(&stderr_log).await;
                                                    }
                                                }

                                                // try next format
                                                continue;
                                            }
                                        }
                                    }
                                    Err(e2) => {
                                        eprintln!("Failed to spawn ffmpeg transcode pipeline (fmt='{}'): {e2:?}", fmt);
                                        stderr_logs.push(format!("fmt='{}' spawn failed: {e2:?}", fmt));
                                        continue;
                                    }
                                }
                            }

                            // If we reach here, all attempts failed. Optionally send verbose diagnostics
                            if std::env::var("MUSIC_VERBOSE").is_ok() {
                                let msg = if stderr_logs.is_empty() { "No ffmpeg stderr captured".to_string() } else { stderr_logs.join("\n-----\n") };
                                let _ = send_info(ctx, channel, color, "Music - Spotify ffmpeg diagnostics", &msg).await;
                            }

                            let _ = send_info(ctx, channel, color, "Music", "Spotify stream failed (all transcode attempts failed), falling back to YouTube search").await;
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Failed to spawn spotify stream command: {e:?}");
                    let _ = send_info(ctx, channel, color, "Music", "Failed to start Spotify stream command, falling back to YouTube search").await;
                }
            }
        } else {
            let _ = send_info(ctx, channel, color, "Music", "No Spotify stream command configured (set SPOTIFY_STREAM_CMD or place `librespot-wrapper` in .bin). Falling back to YouTube search").await;
        }
    }

    // `play` accepts a Track; Input implements conversion so `.into()` works
    let handle = handler.play(input.into());

    // Attempt to make the lazy track playable (yt-dlp in background)
    match handle.make_playable_async().await {
        Ok(()) => {
            // Ensure track is unpaused/playing
            let _ = handle.play();
            // Set default volume
            let _ = handle.set_volume(0.20);

            // Try to fetch aux metadata (title/artist/duration/thumbnail) and store it for remaining-time calculations
            if let Ok(list) = ytdl.search(Some(1)).await {
                if let Some(meta) = list.into_iter().next() {
                    let title = meta.track.or(meta.title);
                    let artist = meta.artist;
                    let thumbnail = meta.thumbnail;
                    let duration = meta.duration;

                    if let Some(ms) = ctx.data.read().await.get::<crate::TrackMetaStore>().cloned() {
                        let mut mm = ms.lock().await;
                        mm.insert(guild_id, crate::TrackMeta { title, artist, duration, thumbnail });
                    }
                }
            }

            // Store the handle for control panels
            let gid = guild_id;
            let _ = store_handle(ctx, gid, handle.clone()).await;

            send_info(
                ctx,
                channel,
                color,
                "Music",
                &format!("Now playing: {search_query}"),
            )
            .await?;
            return Ok(());
        }
        Err(e) => {
            eprintln!("Failed to make track playable: {e:?}");

            // Attempt to gather metadata from ytdl for diagnostics
            let diagnostic = match ytdl.search(Some(1)).await {
                Ok(list) => list
                    .into_iter()
                    .map(|m| format!("title={:?} source_url={:?} duration={:?}", m.title, m.source_url, m.duration))
                    .collect::<Vec<_>>()
                    .join(" | "),
                Err(err2) => format!("failed to get ytdl metadata: {err2:?}"),
            };

            // Try a series of fallbacks:
            // 1) Direct URL from yt-dlp -g for preferred formats
            // 2) Download to a temporary file and play it, removing it after finish (last resort)
            use tokio::process::Command;

            // Attempt direct urls based on format preference
            let formats = [
                "bestaudio[ext=webm]/bestaudio/best",
                "bestaudio[ext=m4a]/bestaudio/best",
                "bestaudio/best",
            ];

            for fmt in &formats {
                let search_arg = format!("ytsearch1:{}", search_query);
                let output = Command::new("yt-dlp")
                    .arg("-f")
                    .arg(fmt)
                    .arg("-j")
                    .arg(&search_arg)
                    .output()
                    .await;

                match output {
                    Ok(o) if o.status.success() => {
                        let stdout = String::from_utf8_lossy(&o.stdout);
                        if let Some(json_line) = stdout.lines().next() {
                            if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_line) {
                                if let Some(url) = val.get("url").and_then(|v| v.as_str()) {
                                    // Build header map if provided
                                    let mut headers = reqwest::header::HeaderMap::new();
                                    if let Some(hm) = val.get("http_headers").and_then(|v| v.as_object()) {
                                        for (k, v) in hm.iter() {
                                            if let Some(s) = v.as_str() {
                                                if let (Ok(hn), Ok(hv)) = (
                                                    reqwest::header::HeaderName::from_bytes(k.as_bytes()),
                                                    reqwest::header::HeaderValue::from_str(s),
                                                ) {
                                                    headers.insert(hn, hv);
                                                }
                                            }
                                        }
                                    }

                                    // If JSON contains metadata, store title/artist/thumbnail/duration in TrackMetaStore
                                    let title = val.get("title").and_then(|v| v.as_str()).map(|s| s.to_string());
                                    let artist = val.get("artist").and_then(|v| v.as_str()).map(|s| s.to_string())
                                        .or_else(|| val.get("uploader").and_then(|v| v.as_str()).map(|s| s.to_string()));
                                    let thumbnail = val.get("thumbnail").and_then(|v| v.as_str()).map(|s| s.to_string());

                                    let mut duration_opt: Option<std::time::Duration> = None;
                                    if let Some(dv) = val.get("duration") {
                                        if let Some(f) = dv.as_f64() {
                                            duration_opt = Some(std::time::Duration::from_secs_f64(f));
                                        } else if let Some(u) = dv.as_u64() {
                                            duration_opt = Some(std::time::Duration::from_secs(u));
                                        }
                                    }

                                    if let Some(ms) = ctx.data.read().await.get::<crate::TrackMetaStore>().cloned() {
                                        let mut mm = ms.lock().await;
                                        mm.insert(guild_id, crate::TrackMeta { title, artist, duration: duration_opt, thumbnail });
                                    }

                                    let mut http_input = songbird::input::HttpRequest::new_with_headers(http_client.clone(), url.to_string(), headers.clone());
                                    if let Some(fs) = val.get("filesize").and_then(|v| v.as_u64()) {
                                        http_input.content_length = Some(fs);
                                    }

                                    let new_handle = handler.play_input(http_input.into());

                                    match new_handle.make_playable_async().await {
                                        Ok(()) => {
                                            let _ = new_handle.play();
                                            // Set default volume
                                            let _ = new_handle.set_volume(0.20);
                                            let gid = guild_id;
                                            let _ = store_handle(ctx, gid, new_handle.clone()).await;
                                            send_info(
                                                ctx,
                                                channel,
                                                color,
                                                "Music",
                                                &format!("Now playing (format {}): {search_query}", fmt),
                                            )
                                            .await?;
                                            return Ok(());
                                        }
                                        Err(e2) => {
                                            eprintln!("Format fallback {} failed: {e2:?}", fmt);

                                            // Try an ffmpeg child-stream fallback: spawn ffmpeg to read the URL and pipe PCM to stdout
                                            // Build header string for ffmpeg if provided
                                            let mut header_str = String::new();
                                            for (hn, hv) in headers.iter() {
                                                header_str.push_str(&format!("{}: {}\r\n", hn.as_str(), hv.to_str().unwrap_or_default()));
                                            }

                                            // Use std::process::Command so we get a std::process::Child suitable for ChildContainer
                                            let mut ff_cmd = std::process::Command::new("ffmpeg");
                                            if !header_str.is_empty() {
                                                ff_cmd.arg("-headers").arg(header_str);
                                            }
// Use WAV (pcm_s16le) container so symphonia can probe the stream reliably
                                                let child_proc_res = ff_cmd
                                                .arg("-i")
                                                .arg(url.to_string())
                                                .arg("-vn")
                                                .arg("-c:a").arg("pcm_s16le")
                                                .arg("-f").arg("wav")
                                                .arg("-ar").arg("48000")
                                                .arg("-ac").arg("2")
                                                .arg("pipe:1")
                                                .stdout(std::process::Stdio::piped())
                                                    .stderr(std::process::Stdio::piped())
                                                .spawn();

                                            match child_proc_res {
                                                Ok(mut child_proc) => {
                                                    // Prepare a stderr file to capture ffmpeg diagnostics we can send to Discord if requested
                                                    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
                                                    let uniq_child = std::time::SystemTime::now()
                                                        .duration_since(std::time::UNIX_EPOCH)
                                                        .map(|d| d.as_nanos())
                                                        .unwrap_or(0);
                                                    let stderr_log = cwd.join(format!("yt-{}-{}-ffstderr.log", std::process::id(), uniq_child));

                                                    // Capture ffmpeg stderr into a file for later inspection
                                                    if let Some(mut stderr) = child_proc.stderr.take() {
                                                        let stderr_log_clone = stderr_log.clone();
                                                        std::thread::spawn(move || {
                                                            use std::io::Read;
                                                            let mut buf = String::new();
                                                            let _ = stderr.read_to_string(&mut buf);
                                                            let _ = std::fs::write(&stderr_log_clone, &buf);
                                                            if !buf.is_empty() {
                                                                eprintln!("ffmpeg child stderr written to {}", stderr_log_clone.display());
                                                            }
                                                        });
                                                    }

                                                    // Wrap the std child in Songbird's ChildContainer adapter
                                                    let container = songbird::input::ChildContainer::from(child_proc);
                                                    let child_input: songbird::input::Input = container.into();
                                                    let child_handle = handler.play_input(child_input);

                                                    match child_handle.make_playable_async().await {
                                                        Ok(()) => {
                                                            // If we had a stderr file, remove it on success
                                                            let _ = tokio::fs::remove_file(&stderr_log).await;

                                                            let _ = child_handle.play();
                                                            // Set default volume
                                                            let _ = child_handle.set_volume(0.20);
                                                            send_info(
                                                                ctx,
                                                                channel,
                                                                color,
                                                                "Music",
                                                                &format!("Now playing (ffmpeg stream): {search_query}"),
                                                            )
                                                            .await?;
                                                            return Ok(());
                                                        }
                                                        Err(e3) => {
                                                            eprintln!("ffmpeg child playback failed: {e3:?}");
                                                            // If verbose, send stderr file content to the channel for debugging
                                                            if std::env::var("MUSIC_VERBOSE").is_ok() {
                                                                if let Ok(s) = tokio::fs::read_to_string(&stderr_log).await {
                                                                    if !s.is_empty() {
                                                                        let _ = send_info(
                                                                            ctx,
                                                                            channel,
                                                                            color,
                                                                            "Music - ffmpeg stderr",
                                                                            &s,
                                                                        )
                                                                        .await;
                                                                    }
                                                                }
                                                            }
                                                            // Clean up stderr file
                                                            let _ = tokio::fs::remove_file(&stderr_log).await;

                                                            continue;
                                                        }
                                                    }
                                                }
                                                Err(err_spawn) => {
                                                    eprintln!("Failed to spawn ffmpeg for child stream: {err_spawn:?}");
                                                    continue;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Ok(o) => {
                        eprintln!("yt-dlp -g for format {} failed: {}", fmt, String::from_utf8_lossy(&o.stderr));
                        continue;
                    }
                    Err(err2) => {
                        eprintln!("Failed to run yt-dlp for format {}: {err2:?}", fmt);
                        continue;
                    }
                }
            }

            // Final fallback: download a file into the bot's current working dir and play it, then remove after finish
            // Use an output template so yt-dlp chooses the extension (avoid mismatches)
            let cwd = std::env::current_dir()?;
            let uniq = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos();
            let out_template_prefix = format!("yt-{}-{}", std::process::id(), uniq);
            let out_template = cwd.join(format!("{}.%(ext)s", out_template_prefix));

            let download_arg = format!("ytsearch1:{}", search_query);
            let out = Command::new("yt-dlp")
                .arg("-f")
                .arg("bestaudio")
                .arg("-o")
                .arg(out_template.to_string_lossy().to_string())
                .arg(&download_arg)
                .output()
                .await?;

            if !out.status.success() {
                eprintln!("yt-dlp download failed: {}", String::from_utf8_lossy(&out.stderr));
                send_info(
                    ctx,
                    channel,
                    color,
                    "Music",
                    &format!("Failed to play {search_query}: {e:?}. Diagnostic: {diagnostic}. Also failed to download fallback."),
                )
                .await?;
                return Ok(());
            }

            // Attempt to discover the actual downloaded file written by yt-dlp in the cwd
            let mut found: Option<PathBuf> = None;
            let mut rd = tokio::fs::read_dir(&cwd).await?;
            while let Some(entry) = rd.next_entry().await? {
                let name = entry.file_name();
                if let Some(s) = name.to_str() {
                    if s.starts_with(&out_template_prefix) {
                        found = Some(entry.path());
                        break;
                    }
                }
            }

            if found.is_none() {
                eprintln!("yt-dlp reported success but couldn't find file with prefix {} in {}", out_template_prefix, cwd.display());
                eprintln!("yt-dlp stdout: {}", String::from_utf8_lossy(&out.stdout));
                eprintln!("yt-dlp stderr: {}", String::from_utf8_lossy(&out.stderr));

                send_info(
                    ctx,
                    channel,
                    color,
                    "Music",
                    &format!("Downloaded fallback reported success but the expected file wasn't found in {}. yt-dlp output: stdout: {} stderr: {}", cwd.display(), String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)),
                )
                .await?;
                return Ok(());
            }

            let tmp_path = found.unwrap();
            eprintln!("Using downloaded file: {}", tmp_path.display());

            // Play the downloaded file (or the discovered one)
            let file_input = songbird::input::File::new(tmp_path.clone());
            let new_handle = handler.play_input(file_input.into());

            match new_handle.make_playable_async().await {
                Ok(()) => {
                    // Attach deletion event on End or Error (remove the downloaded file by default)
                    struct RemoveOnEnd(std::path::PathBuf);
                    #[async_trait]
                    impl songbird::events::EventHandler for RemoveOnEnd {
                        async fn act(&self, _ctx: &songbird::events::EventContext<'_>) -> Option<songbird::events::Event> {
                            let _ = tokio::fs::remove_file(&self.0).await;
                            Some(songbird::events::Event::Cancel)
                        }
                    }

                    // Register for End and Error events AFTER we know the file was playable
                    let _ = new_handle.add_event(songbird::events::Event::Track(songbird::events::TrackEvent::End), RemoveOnEnd(tmp_path.clone()));
                    let _ = new_handle.add_event(songbird::events::Event::Track(songbird::events::TrackEvent::Error), RemoveOnEnd(tmp_path.clone()));

                    let _ = new_handle.play();
                    // Set default volume
                    let _ = new_handle.set_volume(0.20);

                    let gid = guild_id;
                    let _ = store_handle(ctx, gid, new_handle.clone()).await;

                    send_info(
                        ctx,
                        channel,
                        color,
                        "Music",
                        &format!("Now playing (downloaded): {search_query}"),
                    )
                    .await?;
                    return Ok(());
                }
                Err(e2) => {
                    eprintln!("Download fallback failed: {e2:?}. Trying ffmpeg transcode...");

                    // Verify the downloaded file still exists before attempting ffmpeg transcode
                    if tokio::fs::metadata(&tmp_path).await.is_err() {
                        eprintln!("Transcode: expected downloaded file no longer exists: {}", tmp_path.display());
                        send_info(
                            ctx,
                            channel,
                            color,
                            "Music",
                            &format!("Failed to transcode: expected downloaded file missing: {}. Aborting fallback.", tmp_path.display()),
                        )
                        .await?;
                        return Ok(());
                    }

                    // Attempt to transcode the downloaded file to a more-compatible audio file using ffmpeg
                    // Transcode to an Ogg/Opus file (more broadly probeable)
                    // Transcode to a WAV file (pcm_s16le) so symphonia can probe it reliably
                    let trans_path = std::env::current_dir()?.join(format!("yt-{}-{}.wav", std::process::id(), uniq));

                    let ffout = Command::new("ffmpeg")
                        .arg("-y")
                        .arg("-i")
                        .arg(tmp_path.to_string_lossy().to_string())
                        .arg("-ac")
                        .arg("2")
                        .arg("-ar")
                        .arg("48000")
                        .arg("-c:a")
                        .arg("pcm_s16le")
                        .arg(trans_path.to_string_lossy().to_string())
                        .output()
                        .await;

                    match ffout {
                        Ok(o) if o.status.success() => {
                            // Play the transcoded file and ensure both files are removed afterwards
                            let file_input2 = songbird::input::File::new(trans_path.clone());
                            let new_handle2 = handler.play_input(file_input2.into());

                            struct RemoveOnEndVec(Vec<std::path::PathBuf>);
                            #[async_trait]
                            impl songbird::events::EventHandler for RemoveOnEndVec {
                                async fn act(&self, _ctx: &songbird::events::EventContext<'_>) -> Option<songbird::events::Event> {
                                    for p in &self.0 {
                                        let _ = tokio::fs::remove_file(p).await;
                                    }
                                    Some(songbird::events::Event::Cancel)
                                }
                            }

                            let to_rm = RemoveOnEndVec(vec![tmp_path.clone(), trans_path.clone()]);
                            let _ = new_handle2.add_event(songbird::events::Event::Track(songbird::events::TrackEvent::End), to_rm);
                            let _ = new_handle2.add_event(songbird::events::Event::Track(songbird::events::TrackEvent::Error), RemoveOnEndVec(vec![tmp_path, trans_path]));

                            match new_handle2.make_playable_async().await {
                                Ok(()) => {
                                    let _ = new_handle2.play();
                                    // Set default volume
                                    let _ = new_handle2.set_volume(0.20);

                                    let gid = guild_id;
                                    let _ = store_handle(ctx, gid, new_handle2.clone()).await;

                                    send_info(
                                        ctx,
                                        channel,
                                        color,
                                        "Music",
                                        &format!("Now playing (transcoded): {search_query}"),
                                    )
                                    .await?;
                                    return Ok(());
                                }
                                Err(e3) => {
                                    eprintln!("Transcoded playback failed: {e3:?}");
                                    // Include ffmpeg stderr in diagnostics if verbose mode is enabled
                                    let ff_stderr = String::from_utf8_lossy(&o.stderr).to_string();
                                    if std::env::var("MUSIC_VERBOSE").is_ok() && !ff_stderr.is_empty() {
                                        let _ = send_info(
                                            ctx,
                                            channel,
                                            color,
                                            "Music - Transcode stderr",
                                            &format!("ffmpeg stderr: {}", ff_stderr),
                                        )
                                        .await;
                                    }

                                    send_info(
                                        ctx,
                                        channel,
                                        color,
                                        "Music",
                                        &format!("Failed to play {search_query}: {e:?}. Transcode playback failed: {e3:?}. Diagnostic: {diagnostic}"),
                                    )
                                    .await?;
                                    return Ok(());
                                }
                            }
                        }
                        Ok(o) => {
                            eprintln!("ffmpeg failed: {}", String::from_utf8_lossy(&o.stderr));
                            let ff_stderr = String::from_utf8_lossy(&o.stderr).to_string();
                            if std::env::var("MUSIC_VERBOSE").is_ok() && !ff_stderr.is_empty() {
                                let _ = send_info(
                                    ctx,
                                    channel,
                                    color,
                                    "Music - Transcode stderr",
                                    &format!("ffmpeg stderr: {}", ff_stderr),
                                )
                                .await;
                            }

                            send_info(
                                ctx,
                                channel,
                                color,
                                "Music",
                                &format!("Failed to play {search_query}: {e:?}. Download fallback succeeded but ffmpeg transcode failed."),
                            )
                            .await?;
                            return Ok(());
                        }
                        Err(err3) => {
                            eprintln!("Failed to run ffmpeg: {err3:?}");
                            send_info(
                                ctx,
                                channel,
                                color,
                                "Music",
                                &format!("Failed to play {search_query}: {e:?}. Download fallback succeeded but ffmpeg couldn't be run."),
                            )
                            .await?;
                            return Ok(());
                        }
                    }
                }
            }
        }
    }
}

async fn send_info(
    ctx: &Context,
    channel: ChannelId,
    color: u32,
    title: &str,
    desc: &str,
) -> MusicResult<()> {
    let embed = CreateEmbed::new()
        .title(title)
        .description(desc)
        .color(color);

    let message = CreateMessage::new().embed(embed);
    channel.send_message(&ctx.http, message).await?;
    Ok(())
}

async fn streamtest(ctx: &Context, channel: ChannelId, guild_id: Option<GuildId>, uri: &str, color: u32) -> MusicResult<()> {
    if uri.trim().is_empty() {
        send_info(ctx, channel, color, "Stream Test", "Provide a Spotify track URL: music streamtest <url>").await?;
        return Ok(());
    }

    // Resolve command
    if let Some(cmd) = get_spotify_stream_cmd(uri) {
        // Prepare temp sample path
        let tmpdir = std::env::temp_dir();
        let uniq = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
        let sample_path = tmpdir.join(format!("spotify-sample-{}.wav", uniq));

        // Build a shell command to record 10 seconds to WAV via ffmpeg, capturing helper stderr to a file for diagnostics
        let helper_log = tmpdir.join(format!("spotify-helper-{}.log", uniq));
        let ff_cmd = format!("( {cmd} ) 2> {helper_log} | ffmpeg -hide_banner -loglevel error -i - -t 10 -vn -c:a pcm_s16le -ar 48000 -ac 2 -f wav {sample}", cmd = cmd, helper_log = helper_log.to_string_lossy(), sample = sample_path.to_string_lossy());

        // Run the command (blocking on child completion)
        let out = tokio::process::Command::new("sh").arg("-c").arg(&ff_cmd).output().await;

        match out {
            Ok(o) => {
                // Read helper stderr for diagnostics
                let helper_stderr = tokio::fs::read_to_string(&helper_log).await.unwrap_or_else(|_| "<no helper stderr>".into());

                if !o.status.success() {
                    let stderr = String::from_utf8_lossy(&o.stderr).to_string();
                    let desc = format!("Recording failed (ffmpeg exit code {}).\nffmpeg stderr:\n{}\n\nhelper stderr:\n{}", o.status.code().unwrap_or(-1), if stderr.is_empty() { "<empty>".into() } else { stderr.clone() }, helper_stderr);
                    send_info(ctx, channel, color, "Stream Test - Record Failed", &desc).await?;
                    let _ = tokio::fs::remove_file(&helper_log).await;
                    return Ok(());
                }

                // Remove helper log on success (but keep it for 5s in case user wants it)
                let helper_log_clone = helper_log.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    let _ = tokio::fs::remove_file(&helper_log_clone).await;
                });

                // Run ffprobe on the resulting file to gather metadata
                let probe = tokio::process::Command::new("ffprobe")
                    .arg("-hide_banner")
                    .arg("-loglevel")
                    .arg("error")
                    .arg("-show_format")
                    .arg("-show_streams")
                    .arg("-print_format")
                    .arg("json")
                    .arg(&sample_path)
                    .output()
                    .await;

                match probe {
                    Ok(p) => {
                        let info = String::from_utf8_lossy(&p.stdout).to_string();
                        // Truncate if too long
                        let info_short = if info.len() > 1900 { format!("{}\n...[truncated]", &info[..1900]) } else { info.clone() };
                        let desc = format!("Recorded 10s sample to `{}`. ffprobe output:\n{}", sample_path.display(), info_short);

                        // Attempt to attach the file if under 8MB
                        let mut sent = channel.send_message(&ctx.http, CreateMessage::new().content(desc.clone())).await?;

                        if let Ok(meta) = tokio::fs::metadata(&sample_path).await {
                            if meta.len() > 8_000_000 {
                                // Too large to attach to Discord; keep local and inform path
                                let _ = send_info(ctx, channel, color, "Stream Test - Sample Saved", &format!("Saved sample to {} ({} bytes). File too large to upload.", sample_path.display(), meta.len())).await;
                            } else {
                                // Attach by editing message and adding the file
                                // Use CreateMessage::new with file attachment
                                let b = tokio::fs::read(&sample_path).await?;
                                let mut msg = CreateMessage::new().content(format!("Sample ({} bytes):", b.len()));
                                msg = msg.add_file(serenity::builder::CreateAttachment::bytes(b, "sample.wav"));
                                let _ = channel.send_message(&ctx.http, msg).await?;
                            }
                        }

                        // Clean up sample file after a short delay so user can download if attached
                        let sp = sample_path.clone();
                        tokio::spawn(async move {
                            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                            let _ = tokio::fs::remove_file(&sp).await;
                        });

                        return Ok(());
                    }
                    Err(e) => {
                        send_info(ctx, channel, color, "Stream Test - Probe Failed", &format!("Recorded sample at {} but ffprobe failed: {e:?}", sample_path.display())).await?;
                        return Ok(());
                    }
                }
            }
            Err(e) => {
                send_info(ctx, channel, color, "Stream Test - Execution Failed", &format!("Failed to run recording command: {e:?}")).await?;
                return Ok(());
            }
        }
    } else {
        send_info(ctx, channel, color, "Stream Test", "No Spotify stream command configured (set SPOTIFY_STREAM_CMD or place `librespot-wrapper` in .bin)").await?;
    }

    Ok(())
}

async fn send_temp_info(ctx: Context, channel: ChannelId, content: &str) -> MusicResult<()> {
    // Send a short non-embedded message and delete it after a short delay to mimic ephemeral behavior
    let msg = channel
        .send_message(&ctx.http, CreateMessage::new().content(content))
        .await?;

    let http = ctx.http.clone();
    let id = msg.id;
    let channel_clone = channel;
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        let _ = channel_clone.delete_message(&http, id).await;
    });

    Ok(())
}

async fn send_control_panel(
    ctx: &Context,
    channel: ChannelId,
    owner: UserId,
    guild_id: GuildId,
    color: u32,
) -> MusicResult<()> {
    use serenity::builder::{CreateActionRow, CreateButton};
    use serenity::all::ButtonStyle;

    // Attempt to fetch current track info
    let mut desc = String::new();
    let maybe_store = ctx.data.read().await.get::<crate::TrackStore>().cloned();

    if let Some(store) = maybe_store {
        let map = store.lock().await;
        if let Some(handle) = map.get(&guild_id) {
            match handle.get_info().await {
                Ok(info) => {
                    // Try to fetch stored total duration for this guild, if present
                    let dur_opt = {
                        let data_read = ctx.data.read().await;
                        data_read.get::<crate::TrackMetaStore>().cloned()
                    };

                    let remaining = if let Some(meta_store) = dur_opt {
                        let meta_map = meta_store.lock().await;
                        if let Some(meta) = meta_map.get(&guild_id) {
                            if let Some(total) = meta.duration {
                                if total > info.position {
                                    let rem = total - info.position;
                                    let secs = rem.as_secs();
                                    let mins = secs / 60;
                                    let secs = secs % 60;
                                    format!("{mins}:{:02}", secs)
                                } else {
                                    "0:00".into()
                                }
                            } else {
                                "Unknown".into()
                            }
                        } else {
                            "Unknown".into()
                        }
                    } else {
                        "Unknown".into()
                    };

                    desc = format!("Status: {:?}\nVolume: {:.2}\nRemaining: {}", info.playing, info.volume, remaining);
                }
                Err(_) => {
                    desc = "Status: Unknown".into();
                }
            }
        } else {
            desc = "No active track".into();
        }
    } else {
        desc = "No active track store".into();
    }

    // Try to get track title/artist/thumbnail from TrackMetaStore to make the embed more prominent
    let mut title_str = "Music Controls".to_string();
    let mut thumbnail_opt: Option<String> = None;
    if let Some(ms) = ctx.data.read().await.get::<crate::TrackMetaStore>().cloned() {
        let mm = ms.lock().await;
        if let Some(meta) = mm.get(&guild_id) {
            match (&meta.title, &meta.artist) {
                (Some(t), Some(a)) => title_str = format!("{} — {}", t, a),
                (Some(t), None) => title_str = t.clone(),
                (None, Some(a)) => title_str = a.clone(),
                _ => {}
            }
            thumbnail_opt = meta.thumbnail.clone();
        }
    }

    let mut embed = CreateEmbed::new().title(title_str).description(desc).color(color);
    if let Some(th) = thumbnail_opt {
        embed = embed.thumbnail(th);
    }

    // Build buttons with owner and guild embedded in custom id
    let owner_id = owner.to_string();
    let guild_id_s = guild_id.to_string();

    let pause_id = format!("music:pause:{}:{}", owner_id, guild_id_s);
    let resume_id = format!("music:resume:{}:{}", owner_id, guild_id_s);
    let stop_id = format!("music:stop:{}:{}", owner_id, guild_id_s);
    let vol_down_id = format!("music:vol_down:{}:{}", owner_id, guild_id_s);
    let vol_up_id = format!("music:vol_up:{}:{}", owner_id, guild_id_s);

    let row1 = CreateActionRow::Buttons(vec![
        CreateButton::new(pause_id).style(ButtonStyle::Primary).label("Pause"),
        CreateButton::new(resume_id).style(ButtonStyle::Success).label("Resume"),
        CreateButton::new(stop_id).style(ButtonStyle::Danger).label("Stop"),
    ]);

    let row2 = CreateActionRow::Buttons(vec![
        CreateButton::new(vol_down_id).style(ButtonStyle::Secondary).label("Vol -"),
        CreateButton::new(vol_up_id).style(ButtonStyle::Secondary).label("Vol +"),
    ]);

    let mut message = CreateMessage::new().embed(embed);
    message = message.components(vec![row1, row2]);

    // Send the control panel message and capture it so we can update it live
    let sent = channel.send_message(&ctx.http, message).await?;

    // Spawn a background task to periodically update the remaining time and state
    let ctx_clone = ctx.clone();
    let mut message_clone = sent.clone();
    let guild_copy = guild_id;
    let col = color;
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;

            // Fetch handle from TypeMap
            let maybe_store = ctx_clone.data.read().await.get::<crate::TrackStore>().cloned();
            if maybe_store.is_none() {
                let ce = CreateEmbed::new().title("Music Controls").description("No active track store").color(col);
                let edit_msg = serenity::builder::EditMessage::new().embed(ce);
                let _ = message_clone.edit(&ctx_clone.http, edit_msg).await;
                break;
            }

            let store = maybe_store.unwrap();
            let map = store.lock().await;
            if let Some(handle) = map.get(&guild_copy) {
                match handle.get_info().await {
                    Ok(info) => {
                        // Try to fetch stored total duration for this guild, if present
                        let duration_str = {
                            let data_read = ctx_clone.data.read().await;
                            data_read.get::<crate::TrackMetaStore>().cloned()
                        };

                        let remaining = if let Some(meta_store) = duration_str {
                            let meta_map = meta_store.lock().await;
                            if let Some(meta) = meta_map.get(&guild_copy) {
                                if let Some(total) = meta.duration {
                                    if total > info.position {
                                        let rem = total - info.position;
                                        let secs = rem.as_secs();
                                        let mins = secs / 60;
                                        let secs = secs % 60;
                                        format!("{mins}:{:02}", secs)
                                    } else {
                                        "0:00".into()
                                    }
                                } else {
                                    "Unknown".into()
                                }
                            } else {
                                "Unknown".into()
                            }
                        } else {
                            "Unknown".into()
                        };

                        let new_desc = format!("Status: {:?}\nVolume: {:.2}\nRemaining: {}", info.playing, info.volume, remaining);

                        // Look up meta for title/artist/thumbnail
                        let mut title_str = "Music Controls".to_string();
                        let mut thumbnail: Option<String> = None;
                        if let Some(ms2) = ctx_clone.data.read().await.get::<crate::TrackMetaStore>().cloned() {
                            let mm2 = ms2.lock().await;
                            if let Some(meta) = mm2.get(&guild_copy) {
                                match (&meta.title, &meta.artist) {
                                    (Some(t), Some(a)) => title_str = format!("{} — {}", t, a),
                                    (Some(t), None) => title_str = t.clone(),
                                    (None, Some(a)) => title_str = a.clone(),
                                    _ => {}
                                }
                                thumbnail = meta.thumbnail.clone();
                            }
                        }

                        let mut ce = CreateEmbed::new().title(title_str).description(new_desc).color(col);
                        if let Some(turl) = thumbnail {
                            ce = ce.thumbnail(turl);
                        }

                        let edit_msg = serenity::builder::EditMessage::new().embed(ce);
                        let _ = message_clone.edit(&ctx_clone.http, edit_msg).await;

                        // Stop updating when track stops
                        if matches!(info.playing, songbird::tracks::PlayMode::Stop) {
                            break;
                        }
                    }
                    Err(_) => {
                        let ce = CreateEmbed::new().title("Music Controls").description("Status: Unknown").color(col);
                        let edit_msg = serenity::builder::EditMessage::new().embed(ce);
                        let _ = message_clone.edit(&ctx_clone.http, edit_msg).await;
                        break;
                    }
                }
            } else {
                let ce = CreateEmbed::new().title("Music Controls").description("No active track").color(col);
                let edit_msg = serenity::builder::EditMessage::new().embed(ce);
                let _ = message_clone.edit(&ctx_clone.http, edit_msg).await;
                break;
            }
        }
    });

    Ok(())
}

fn voice_channel_for_user_id(ctx: &Context, guild_id: GuildId, user_id: UserId) -> Option<ChannelId> {
    ctx.cache
        .guild(guild_id)
        .and_then(|guild| guild.voice_states.get(&user_id).and_then(|vs| vs.channel_id))
}

// Backwards-compatible wrapper if a Message is available
#[allow(dead_code)]
fn voice_channel_for_user(ctx: &Context, msg: &Message) -> Option<ChannelId> {
    let guild_id = msg.guild_id?;
    voice_channel_for_user_id(ctx, guild_id, msg.author.id)
}

fn prepend_path(bin: &str) -> MusicResult<()> {
    let bin_path = PathBuf::from(bin);
    let mut paths: Vec<PathBuf> = env::var_os("PATH")
        .map(|p| env::split_paths(&p).collect())
        .unwrap_or_default();

    if !paths.iter().any(|p| p == &bin_path) {
        paths.insert(0, bin_path);
        let new_path = env::join_paths(paths)?;
        unsafe {
            env::set_var("PATH", &new_path);
        }
    }
    Ok(())
}

async fn spotify_first_then_query(user_query: &str) -> MusicResult<Option<String>> {
    let client_id = match env::var("SPOTIFY_CLIENT_ID") {
        Ok(v) if !v.is_empty() => v,
        _ => return Ok(None),
    };
    let client_secret = match env::var("SPOTIFY_CLIENT_SECRET") {
        Ok(v) if !v.is_empty() => v,
        _ => return Ok(None),
    };

    let token = fetch_spotify_token(&client_id, &client_secret).await?;
    let track = search_spotify_track(&token.access_token, user_query).await?;

    Ok(track.map(|(name, artist)| format!("{} {}", name, artist)))
}

// Convenience wrapper to fetch a token using env vars (returns SpotifyToken or Err)
async fn fetch_spotify_token_from_env() -> MusicResult<SpotifyToken> {
    let client_id = env::var("SPOTIFY_CLIENT_ID").map_err(|_| "SPOTIFY_CLIENT_ID not set")?;
    let client_secret = env::var("SPOTIFY_CLIENT_SECRET").map_err(|_| "SPOTIFY_CLIENT_SECRET not set")?;
    fetch_spotify_token(&client_id, &client_secret).await
}

// Fetch a Spotify track by its id using the Web API, returning (title, artist, duration_opt, thumbnail_opt)
async fn fetch_spotify_track_by_id(token: &str, id: &str) -> MusicResult<Option<(String, String, Option<std::time::Duration>, Option<String>)>> {
    let url = format!("https://api.spotify.com/v1/tracks/{}", id);
    let client = Client::builder().build()?;
    let res = client.get(&url).bearer_auth(token).send().await?.error_for_status()?;
    let v: serde_json::Value = res.json().await?;

    let name = v.get("name").and_then(|s| s.as_str()).map(|s| s.to_string());
    let artist = v.get("artists").and_then(|a| a.as_array()).and_then(|arr| arr.get(0)).and_then(|a0| a0.get("name")).and_then(|n| n.as_str()).map(|s| s.to_string());
    let duration = v.get("duration_ms").and_then(|d| d.as_u64()).map(|ms| std::time::Duration::from_millis(ms));
    let thumbnail = v.get("album").and_then(|al| al.get("images")).and_then(|imgs| imgs.as_array()).and_then(|arr| arr.get(0)).and_then(|i0| i0.get("url")).and_then(|u| u.as_str()).map(|s| s.to_string());

    if let (Some(n), Some(a)) = (name, artist) {
        Ok(Some((n, a, duration, thumbnail)))
    } else {
        Ok(None)
    }
}

// Parse track id from a spotify URL or URI, returning the 'id' part
fn parse_spotify_track_id(s: &str) -> Option<String> {
    // spotify:track:ID
    if let Some(pos) = s.find("spotify:track:") {
        return s[pos + "spotify:track:".len()..].split(&['?', '&'][..]).next().map(|x| x.to_string());
    }

    // https://open.spotify.com/track/ID
    if let Some(idx) = s.find("/track/") {
        return s[idx + "/track/".len()..].split(&['?', '&', '/'][..]).next().map(|x| x.to_string());
    }

    None
}

// Construct a spotify stream command by checking env and falling back to `.bin/librespot-wrapper` if present.
fn get_spotify_stream_cmd(uri: &str) -> Option<String> {
    // Prefer explicit env var
    if let Ok(t) = std::env::var("SPOTIFY_STREAM_CMD") {
        // Allow user to include quotes in their template; but if they didn't, we'll still quote for safety
        let quoted = t.replace("{uri}", &shell_quote(uri));
        return Some(quoted);
    }

    // Fallback: look for `.bin/librespot-wrapper` in current directory
    if let Ok(cwd) = std::env::current_dir() {
        let candidate = cwd.join(".bin").join("librespot-wrapper");
        if candidate.is_file() {
            // Check executable bit on unix-like systems
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = std::fs::metadata(&candidate) {
                    let perm = meta.permissions();
                    if perm.mode() & 0o111 == 0 {
                        // not executable
                        return None;
                    }
                }
            }

            // If the input was an open.spotify.com link, prefer the spotify:track:ID form
            if let Some(id) = parse_spotify_track_id(uri) {
                let s_uri = format!("spotify:track:{}", id);
                return Some(format!("{} --uri {} --stdout", candidate.to_string_lossy(), shell_quote(&s_uri)));
            }

            return Some(format!("{} --uri {} --stdout", candidate.to_string_lossy(), shell_quote(uri)));
        }
    }

    None
}

// Simple shell-quoting helper for safe substitution
fn shell_quote(s: &str) -> String {
    if s.contains('"') {
        // fallback to single quotes, escaping if necessary
        let replaced = s.replace('"', "\\\"");
        format!("\"{}\"", replaced)
    } else {
        format!("\"{}\"", s)
    }
}

async fn fetch_spotify_token(client_id: &str, client_secret: &str) -> MusicResult<SpotifyToken> {
    let auth = format!("{}:{}", client_id, client_secret);
    let auth_b64 = B64_ENGINE.encode(auth);

    let client = Client::builder().build()?;
    let res = client
        .post("https://accounts.spotify.com/api/token")
        .header("Authorization", format!("Basic {}", auth_b64))
        .form(&[("grant_type", "client_credentials")])
        .send()
        .await?
        .error_for_status()?;

    let token: SpotifyToken = res.json().await?;
    Ok(token)
}

async fn search_spotify_track(token: &str, query: &str) -> MusicResult<Option<(String, String)>> {
    let client = Client::builder().build()?;

    let res = client
        .get("https://api.spotify.com/v1/search")
        .query(&[("q", query), ("type", "track"), ("limit", "1")])
        .bearer_auth(token)
        .send()
        .await?
        .error_for_status()?;

    let data: SpotifySearch = res.json().await?;
    let track = data.tracks.items.into_iter().next();
    Ok(track.map(|t| {
        let artist = t
            .artists
            .get(0)
            .map(|a| a.name.clone())
            .unwrap_or_else(|| "Unknown".to_string());
        (t.name, artist)
    }))
}
