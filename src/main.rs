use poise::serenity_prelude as serenity;
use serenity::builder::{
    CreateEmbed, CreateInteractionResponse, CreateInteractionResponseMessage,
};
use serenity::model::id::{GuildId, UserId};
use serenity::prelude::*;
use songbird::SerenityInit;
use dotenvy::dotenv;
use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use tokio::sync::Mutex;

mod music;
mod start;
mod config;
mod modalert;

use crate::config::ensure_default_config;
use crate::modalert::{
    ensure_modalert_store, is_modalert_enabled, save_modalert_store, ModAlertStore,
};
use crate::music::{ensure_media_tools, handle_music};
use crate::start::handle_start;

// ---------- Shared constants ----------
const PREFIX: &str = "!is"; // users can type "!is ..."
const EMBED_COLOR: u32 = 0x5865F2;

// ---------- Poise data & error ----------
pub struct Data;
pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Ctx<'a> = poise::Context<'a, Data, Error>;

// ---------- Shared TypeMap stores ----------
struct TrackStore;
impl TypeMapKey for TrackStore {
    type Value = Arc<Mutex<HashMap<GuildId, songbird::tracks::TrackHandle>>>;
}

#[derive(Clone, Debug, Default)]
pub struct TrackMeta {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub duration: Option<std::time::Duration>,
    pub thumbnail: Option<String>,
}
struct TrackMetaStore;
impl TypeMapKey for TrackMetaStore {
    type Value = Arc<Mutex<HashMap<GuildId, TrackMeta>>>;
}

// ---------- Commands ----------
#[poise::command(prefix_command, slash_command)]
async fn ping(ctx: Ctx<'_>) -> Result<(), Error> {
    ctx.say("Pong!").await?;
    Ok(())
}

#[poise::command(prefix_command, slash_command)]
async fn help(
    ctx: Ctx<'_>,
    #[description = "Specific command to show help for"] command: Option<String>,
) -> Result<(), Error> {
    poise::builtins::help(
        ctx,
        command.as_deref(),
        poise::builtins::HelpConfiguration::default(),
    )
    .await?;
    Ok(())
}

#[poise::command(prefix_command, slash_command)]
async fn modalert(ctx: Ctx<'_>) -> Result<(), Error> {
    ctx.defer().await?;
    let sctx = ctx.serenity_context();
    let guild_id = match ctx.guild_id() {
        Some(g) => g,
        None => {
            ctx.say("This command can only be used in a server.").await?;
            return Ok(());
        }
    };

    // Only server owner can toggle
    let is_owner = {
        if let Some(g) = sctx.cache.guild(guild_id) {
            g.owner_id == ctx.author().id
        } else if let Ok(pg) = guild_id.to_partial_guild(&sctx.http).await {
            pg.owner_id == ctx.author().id
        } else {
            false
        }
    };

    if !is_owner {
        ctx.say("Only the server owner can toggle mod alerts.").await?;
        return Ok(());
    }

    let toggled_on = {
        let data = sctx.data.read().await;
        if let Some(store) = data.get::<ModAlertStore>() {
            let mut set = store.lock().await;
            if set.contains(&guild_id) {
                set.remove(&guild_id);
                false
            } else {
                set.insert(guild_id);
                true
            }
        } else {
            false
        }
    };

    if let Err(e) = save_modalert_store(sctx).await {
        eprintln!("Failed saving modalert store: {e:?}");
    }

    if toggled_on {
        ctx.say("Mod alerts enabled for this server.").await?;
    } else {
        ctx.say("Mod alerts disabled for this server.").await?;
    }
    Ok(())
}

#[poise::command(
    prefix_command,
    slash_command,
    subcommands("music_join", "music_play", "music_leave", "music_control"),
    rename = "music",
    track_edits
)]
async fn music(_ctx: Ctx<'_>) -> Result<(), Error> {
    Ok(())
}

#[poise::command(prefix_command, slash_command, rename = "join")]
async fn music_join(
    ctx: Ctx<'_>,
    #[description = "Voice channel id or mention (optional)"] channel: Option<String>,
) -> Result<(), Error> {
    ctx.defer().await?;
    let sctx = ctx.serenity_context();
    let channel_id = ctx.channel_id();
    let author_id = ctx.author().id;
    let guild_id = ctx.guild_id();

    // Try to parse a channel id/mention if provided
    let arg = channel.unwrap_or_default();
    let parsed_channel: Option<serenity::model::id::ChannelId> = arg
        .split_whitespace()
        .next()
        .and_then(|s| s.trim().trim_start_matches("<#").trim_end_matches('>').parse::<u64>().ok())
        .map(serenity::model::id::ChannelId::from);

    // Best-effort detection if none provided
    let user_vc = if parsed_channel.is_some() {
        parsed_channel
    } else {
        guild_id.and_then(|gid| {
            sctx.cache
                .guild(gid)
                .and_then(|g| g.voice_states.get(&author_id).and_then(|vs| vs.channel_id))
        })
    };

    handle_music(
        sctx,
        channel_id,
        user_vc,
        author_id,
        guild_id,
        "join",
        EMBED_COLOR,
    )
    .await
    .map_err(|e| e.into())
}

#[poise::command(prefix_command, slash_command, rename = "play")]
async fn music_play(
    ctx: Ctx<'_>,
    #[description = "Song name or URL"] query: String,
) -> Result<(), Error> {
    ctx.defer().await?;
    let sctx = ctx.serenity_context();
    let channel_id = ctx.channel_id();
    let author_id = ctx.author().id;
    let guild_id = ctx.guild_id();
    let args = format!("play {}", query);
    handle_music(sctx, channel_id, None, author_id, guild_id, &args, EMBED_COLOR).await?;
    Ok(())
}

#[poise::command(prefix_command, slash_command, rename = "leave")]
async fn music_leave(ctx: Ctx<'_>) -> Result<(), Error> {
    ctx.defer().await?;
    let sctx = ctx.serenity_context();
    let channel_id = ctx.channel_id();
    let author_id = ctx.author().id;
    let guild_id = ctx.guild_id();
    handle_music(sctx, channel_id, None, author_id, guild_id, "leave", EMBED_COLOR).await?;
    Ok(())
}

#[poise::command(prefix_command, slash_command, rename = "control")]
async fn music_control(ctx: Ctx<'_>) -> Result<(), Error> {
    ctx.defer().await?;
    let sctx = ctx.serenity_context();
    let channel_id = ctx.channel_id();
    let author_id = ctx.author().id;
    let guild_id = ctx.guild_id();
    handle_music(sctx, channel_id, None, author_id, guild_id, "control", EMBED_COLOR).await?;
    Ok(())
}

#[poise::command(prefix_command, slash_command, rename = "start")]
async fn start_service(
    ctx: Ctx<'_>,
    #[description = "Service key (or 'list')"] service: String,
    #[description = "Extra args (optional)"] args: Option<String>,
) -> Result<(), Error> {
    ctx.defer().await?;
    let sctx = ctx.serenity_context();
    let channel_id = ctx.channel_id();
    let joined = if let Some(a) = args {
        format!("{} {}", service, a)
    } else {
        service
    };
    handle_start(sctx, channel_id, joined.trim()).await.map_err(|e| e.into())
}

// ---------- Event forwarding ----------
async fn poise_event_handler(
    ctx: &serenity::Context,
    event: &serenity::FullEvent,
    framework_ctx: poise::FrameworkContext<'_, Data, Error>,
    _data: &Data,
) -> Result<(), Error> {
    match event {
        serenity::FullEvent::Ready { data_about_bot, .. } => {
            println!("Connected as {}", data_about_bot.user.name);
        }
        serenity::FullEvent::GuildCreate { guild, .. } => {
            let gid = guild.id;
            if let Err(e) = poise::builtins::register_in_guild(
                ctx,
                &framework_ctx.options().commands,
                gid,
            )
            .await
            {
                eprintln!("Failed to register commands in guild {}: {e:?}", gid);
            }
        }
        serenity::FullEvent::GuildMemberUpdate { old_if_available, new, event } => {
            let gid = event.guild_id;
            if !is_modalert_enabled(ctx, gid).await {
                return Ok(());
            }

            let new_until = new
                .as_ref()
                .and_then(|m| m.communication_disabled_until)
                .or(event.communication_disabled_until);
            let old_until = old_if_available
                .as_ref()
                .and_then(|m| m.communication_disabled_until);

            let is_timeout_newly_applied = match (old_until, new_until) {
                (Some(old_ts), Some(new_ts)) => new_ts > old_ts,
                (None, Some(_)) => true,
                _ => false,
            };
            if !is_timeout_newly_applied { return Ok(()); }

            let user_tag = new
                .as_ref()
                .map(|m| m.user.tag())
                .unwrap_or_else(|| event.user.tag());

            let owner_id = if let Some(g) = ctx.cache.guild(gid) { g.owner_id } else {
                match gid.to_partial_guild(&ctx.http).await {
                    Ok(pg) => pg.owner_id,
                    Err(_) => return Ok(()),
                }
            };
            let content = format!(
                "Moderation alert: {} was timed out in server {}.",
                user_tag,
                gid
            );
            if let Ok(dm) = owner_id.create_dm_channel(&ctx.http).await {
                let _ = dm.say(&ctx.http, content).await;
            }
        }
        serenity::FullEvent::InteractionCreate { interaction } => {
            if let serenity::all::Interaction::Component(mc) = interaction.clone() {
                // custom_id format: music:<action>:<user_id>:<guild_id>
                let custom_id = mc.data.custom_id.clone();
                let mut parts = custom_id.split(':');
                let prefix = parts.next().unwrap_or("");
                if prefix != "music" { return Ok(()); }
                let action = parts.next().unwrap_or("");
                let owner_id = parts
                    .next()
                    .and_then(|s: &str| s.parse::<u64>().ok())
                    .map(|v| UserId::new(v));
                let guild_id = parts
                    .next()
                    .and_then(|s: &str| s.parse::<u64>().ok())
                    .map(|v| GuildId::new(v));

                if let Some(owner) = owner_id {
                    if mc.user.id != owner {
                        let _ = mc
                            .create_response(
                                &ctx.http,
                                CreateInteractionResponse::Message(
                                    CreateInteractionResponseMessage::new()
                                        .content("You are not the owner of this control panel.")
                                        .ephemeral(true),
                                ),
                            )
                            .await;
                        return Ok(());
                    }
                }

                // Fetch handle from TypeMap
                let data_read = ctx.data.read().await;
                if let Some(store) = data_read.get::<TrackStore>() {
                    let mut map = store.lock().await;
                    if let Some(gid) = guild_id {
                        if let Some(handle) = map.get(&gid) {
                            let _ = match action {
                                "pause" => handle
                                    .pause()
                                    .map(|_| "Paused".to_string())
                                    .unwrap_or_else(|e| format!("Pause failed: {e:?}")),
                                "resume" => handle
                                    .play()
                                    .map(|_| "Resumed".to_string())
                                    .unwrap_or_else(|e| format!("Resume failed: {e:?}")),
                                "stop" => {
                                    let r = handle.stop();
                                    map.remove(&gid);
                                    r.map(|_| "Stopped".to_string())
                                        .unwrap_or_else(|e| format!("Stop failed: {e:?}"))
                                }
                                "vol_up" => match handle.get_info().await {
                                    Ok(info) => {
                                        let mut v = info.volume;
                                        v = (v + 0.1).min(5.0);
                                        match handle.set_volume(v) {
                                            Ok(()) => format!("Volume: {:.2}", v),
                                            Err(e) => format!("Set volume failed: {e:?}"),
                                        }
                                    }
                                    Err(e) => format!("Failed to get info: {e:?}"),
                                },
                                "vol_down" => match handle.get_info().await {
                                    Ok(info) => {
                                        let mut v = info.volume;
                                        v = (v - 0.1).max(0.0);
                                        match handle.set_volume(v) {
                                            Ok(()) => format!("Volume: {:.2}", v),
                                            Err(e) => format!("Set volume failed: {e:?}"),
                                        }
                                    }
                                    Err(e) => format!("Failed to get info: {e:?}"),
                                },
                                _ => "Unknown action".to_string(),
                            };

                            // Acknowledge the interaction
                            let _ = mc
                                .create_response(&ctx.http, CreateInteractionResponse::Acknowledge)
                                .await;

                            // Update the control panel embed to reflect current state
                            let (new_desc, title_and_thumb) = if let Some(handle2) = map.get(&gid)
                            {
                                match handle2.get_info().await {
                                    Ok(info2) => {
                                        let meta_opt = {
                                            let data_read = ctx.data.read().await;
                                            data_read.get::<TrackMetaStore>().cloned()
                                        };

                                        let remaining = if let Some(meta_store) = meta_opt.clone() {
                                            let meta_map = meta_store.lock().await;
                                            if let Some(meta) = meta_map.get(&gid) {
                                                if let Some(total) = meta.duration {
                                                    if total > info2.position {
                                                        let rem = total - info2.position;
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

                                        let mut title_str = "Music Controls".to_string();
                                        let mut thumbnail: Option<String> = None;
                                        if let Some(meta_store) = meta_opt {
                                            let meta_map = meta_store.lock().await;
                                            if let Some(meta) = meta_map.get(&gid) {
                                                match (&meta.title, &meta.artist) {
                                                    (Some(t), Some(a)) => {
                                                        title_str = format!("{} — {}", t, a)
                                                    }
                                                    (Some(t), None) => title_str = t.clone(),
                                                    (None, Some(a)) => title_str = a.clone(),
                                                    _ => {}
                                                }
                                                thumbnail = meta.thumbnail.clone();
                                            }
                                        }

                                        (
                                            format!(
                                                "Status: {:?}\nVolume: {:.2}\nRemaining: {}",
                                                info2.playing, info2.volume, remaining
                                            ),
                                            (title_str, thumbnail),
                                        )
                                    }
                                    Err(_) => (
                                        "Status: Unknown".into(),
                                        ("Music Controls".into(), None),
                                    ),
                                }
                            } else {
                                (
                                    "No active track".into(),
                                    ("Music Controls".into(), None),
                                )
                            };

                            let mut ce = CreateEmbed::new()
                                .title(title_and_thumb.0)
                                .description(new_desc)
                                .color(EMBED_COLOR);
                            if let Some(th) = title_and_thumb.1 {
                                ce = ce.thumbnail(th);
                            }
                            let edit_msg = serenity::builder::EditMessage::new().embed(ce);
                            let _ = mc.message.clone().edit(&ctx.http, edit_msg).await;
                        } else {
                            let _ = mc
                                .create_response(
                                    &ctx.http,
                                    CreateInteractionResponse::Message(
                                        CreateInteractionResponseMessage::new()
                                            .content("No active track to control.")
                                            .ephemeral(true),
                                    ),
                                )
                                .await;
                        }
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}

// ---------- Main & framework ----------
#[tokio::main]
async fn main() {
    dotenv().ok();
    let token = env::var("DISCORD_TOKEN").expect("DISCORD_TOKEN not set");

    // Ensure config.jsonc exists (creates default if missing)
    if let Err(e) = ensure_default_config().await {
        eprintln!("Failed to ensure config: {e:?}");
    }

    ensure_media_tools()
        .await
        .expect("Failed to prepare media tools (yt-dlp)");

    // Attempt to prepare an optional Spotify helper binary (librespot wrapper)
    if let Err(e) = crate::music::ensure_spotify_helper().await {
        eprintln!("Failed to prepare Spotify helper: {e:?}");
    }

    let intents = serenity::GatewayIntents::GUILD_MESSAGES
        | serenity::GatewayIntents::DIRECT_MESSAGES
        | serenity::GatewayIntents::MESSAGE_CONTENT
        | serenity::GatewayIntents::GUILDS
        | serenity::GatewayIntents::GUILD_MEMBERS
        | serenity::GatewayIntents::GUILD_VOICE_STATES;

    let framework = poise::Framework::builder()
        .setup(|ctx, _ready, framework| {
            Box::pin(async move {
                // Initialize shared stores
                {
                    let mut data = ctx.data.write().await;
                    data.insert::<TrackStore>(Arc::new(Mutex::new(HashMap::new())));
                    data.insert::<TrackMetaStore>(Arc::new(Mutex::new(HashMap::new())));
                    // Load ModAlert settings into shared store
                    if let Ok(store) = ensure_modalert_store().await {
                        data.insert::<ModAlertStore>(store);
                    }
                }

                // Register in all existing guilds for immediate availability
                for gid in ctx.cache.guilds() {
                    if let Err(e) = poise::builtins::register_in_guild(ctx, &framework.options().commands, gid).await {
                        eprintln!("Failed to register commands in guild {}: {e:?}", gid);
                    }
                }

                // Optional: clear any previously set global commands to prevent duplicates
                // If you want to keep global commands, comment this out.
                let _ = serenity::all::Command::set_global_commands(&ctx.http, vec![]).await;
                Ok(Data)
            })
        })
        .options(poise::FrameworkOptions {
            commands: vec![
                ping(),
                help(),
                modalert(),
                music(),
                music_join(),
                music_play(),
                music_leave(),
                music_control(),
                start_service(),
            ],
            prefix_options: poise::PrefixFrameworkOptions {
                prefix: Some(PREFIX.into()),
                ..Default::default()
            },
            event_handler: |ctx, event, framework, data| {
                Box::pin(poise_event_handler(ctx, event, framework, data))
            },
            ..Default::default()
        })
        .build();

    let mut client = serenity::ClientBuilder::new(token, intents)
        .register_songbird()
        .framework(framework)
        .await
        .expect("Err creating client");

    if let Err(why) = client.start().await {
        eprintln!("Client error: {why:?}");
    }
}

