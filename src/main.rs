use serenity::{
    async_trait,
    builder::{CreateEmbed, CreateMessage},
    model::{channel::Message, gateway::Ready},
    prelude::*,
};
use songbird::SerenityInit;
use dotenvy::dotenv;
use std::env;

mod music;

use crate::music::{ensure_media_tools, handle_music};
use serenity::all::Interaction;
use serenity::model::id::{GuildId, UserId};
use serenity::prelude::TypeMapKey;
use serenity::builder::{CreateInteractionResponse, CreateInteractionResponseMessage};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

const PREFIX: &str = "!is ";
const EMBED_COLOR: u32 = 0x5865F2;

struct Handler;

// Storage for currently playing track handles per guild
struct TrackStore;
impl TypeMapKey for TrackStore { type Value = Arc<Mutex<HashMap<GuildId, songbird::tracks::TrackHandle>>>; }

// Per-guild rich metadata for the currently playing track (title, artist, duration, thumbnail)
#[derive(Clone, Debug, Default)]
pub struct TrackMeta {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub duration: Option<std::time::Duration>,
    pub thumbnail: Option<String>,
}

struct TrackMetaStore;
impl TypeMapKey for TrackMetaStore { type Value = Arc<Mutex<HashMap<GuildId, TrackMeta>>>; }

#[async_trait]
impl EventHandler for Handler {
    async fn message(&self, ctx: Context, msg: Message) {
        // move any data we need out of the potentially non-Send `Message` before awaiting
        let author_is_bot = msg.author.bot;
        let channel_id = msg.channel_id;
        let author_id = msg.author.id;
        let guild_id = msg.guild_id;
        let content = msg.content.clone();

        // Drop the original `Message` so the async future doesn't hold non-Send pointers
        drop(msg);

        if author_is_bot {
            return;
        }

        if let Some(command) = content.trim().strip_prefix(PREFIX) {
            let command = command.trim();

            let mut parts = command.split_whitespace();
            let cmd = parts.next().unwrap_or("");
            let args = parts.collect::<Vec<_>>().join(" ");

            let cmd_lower = cmd.to_ascii_lowercase();

            match cmd_lower.as_str() {
                "ping" => {
                    if let Err(why) = channel_id.say(&ctx.http, "Pong!").await {
                        eprintln!("Error sending message: {why:?}");
                    }
                }
                "help" => {
                    let fields: Vec<(String, String, bool)> = [
                        ("ping", "Pong reply"),
                        ("help", "Show this menu"),
                        ("music join", "Join your voice channel"),
                        ("music play <song>", "Search (Spotify -> YouTube) and queue"),
                        ("music leave", "Disconnect from voice"),
                        ("music control", "Show music control panel"),
                    ]
                    .iter()
                    .map(|(name, desc)| {
                        (format!("{}{}", PREFIX, name), (*desc).to_string(), false)
                    })
                    .collect();

                    let embed = CreateEmbed::new()
                        .title("Help Menu")
                        .description("Use the commands below with the prefix")
                        .color(EMBED_COLOR)
                        .fields(fields.clone());

                    let message = CreateMessage::new().embed(embed);

                    let send_result = channel_id.send_message(&ctx.http, message).await;

                    if let Err(why) = send_result {
                        eprintln!("Error sending help: {why:?}");
                    }
                }
                "music" => {
                    let user_vc = guild_id.and_then(|gid| {
                        ctx.cache
                            .guild(gid)
                            .and_then(|g| g.voice_states.get(&author_id).and_then(|vs| vs.channel_id))
                    });

                    if let Err(why) = handle_music(&ctx, channel_id, user_vc, author_id, guild_id, &args, EMBED_COLOR).await {
                        eprintln!("Error handling music command: {why:?}");
                    }
                }
                _ => {}
            }
        }
    }

    async fn ready(&self, ctx: Context, ready: Ready) {
        println!("Connected as {}", ready.user.name);
        println!("Ready: {} guilds", ctx.cache.guild_count());
    }

    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        if let Interaction::Component(mc) = interaction {
            // custom_id format: music:<action>:<user_id>:<guild_id>
            let custom_id = mc.data.custom_id.clone();
            let mut parts = custom_id.split(':');
            let prefix = parts.next().unwrap_or("");
            if prefix != "music" { return; }
            let action = parts.next().unwrap_or("");
            let owner_id = parts.next().and_then(|s: &str| s.parse::<u64>().ok()).map(|v| UserId::new(v));
            let guild_id = parts.next().and_then(|s: &str| s.parse::<u64>().ok()).map(|v| GuildId::new(v));

            // Verify interaction user is same as panel owner
            if let Some(owner) = owner_id {
                if mc.user.id != owner {
                    let _ = mc.create_response(&ctx.http, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content("You are not the owner of this control panel.").ephemeral(true))).await;
                    return;
                }
            }

            // Fetch handle from TypeMap
            let data_read = ctx.data.read().await;
            if let Some(store) = data_read.get::<TrackStore>() {
                let mut map = store.lock().await;
                if let Some(gid) = guild_id {
                    if let Some(handle) = map.get(&gid) {
                        // perform actions
                        let _ = match action {
                            "pause" => handle.pause().map(|_| "Paused".to_string()).unwrap_or_else(|e| format!("Pause failed: {e:?}")),
                            "resume" => handle.play().map(|_| "Resumed".to_string()).unwrap_or_else(|e| format!("Resume failed: {e:?}")),
                            "stop" => {
                                let r = handle.stop();
                                // remove from map
                                map.remove(&gid);
                                r.map(|_| "Stopped".to_string()).unwrap_or_else(|e| format!("Stop failed: {e:?}"))
                            }
                            "vol_up" => {
                                match handle.get_info().await {
                                    Ok(info) => {
                                        let mut v = info.volume;
                                        v = (v + 0.1).min(5.0);
                                        match handle.set_volume(v) {
                                            Ok(()) => format!("Volume: {:.2}", v),
                                            Err(e) => format!("Set volume failed: {e:?}"),
                                        }
                                    }
                                    Err(e) => format!("Failed to get info: {e:?}"),
                                }
                            }
                            "vol_down" => {
                                match handle.get_info().await {
                                    Ok(info) => {
                                        let mut v = info.volume;
                                        v = (v - 0.1).max(0.0);
                                        match handle.set_volume(v) {
                                            Ok(()) => format!("Volume: {:.2}", v),
                                            Err(e) => format!("Set volume failed: {e:?}"),
                                        }
                                    }
                                    Err(e) => format!("Failed to get info: {e:?}"),
                                }
                            }
                            _ => "Unknown action".to_string(),
                        };

                        // Acknowledge the interaction without sending a visible message
                        let _ = mc.create_response(&ctx.http, CreateInteractionResponse::Acknowledge).await;

                        // Update the control panel embed to reflect current state (status, volume, remaining)
                        let (new_desc, title_and_thumb) = if let Some(handle2) = map.get(&gid) {
                            match handle2.get_info().await {
                                Ok(info2) => {
                                    // Try to fetch stored metadata for this guild, if present
                                    let meta_opt = {
                                        let data_read = ctx.data.read().await;
                                        data_read.get::<TrackMetaStore>().cloned()
                                    };

                                    // remaining uses a clone of meta_opt so we can reuse meta_opt later
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
                                                (Some(t), Some(a)) => title_str = format!("{} — {}", t, a),
                                                (Some(t), None) => title_str = t.clone(),
                                                (None, Some(a)) => title_str = a.clone(),
                                                _ => {}
                                            }
                                            thumbnail = meta.thumbnail.clone();
                                        }
                                    }

                                    (format!("Status: {:?}\nVolume: {:.2}\nRemaining: {}", info2.playing, info2.volume, remaining), (title_str, thumbnail))
                                }
                                Err(_) => ("Status: Unknown".into(), ("Music Controls".into(), None)),
                            }
                        } else {
                            ("No active track".into(), ("Music Controls".into(), None))
                        };

                        // Edit the original control panel message (if available) to update embed description and title
                        let mut ce = CreateEmbed::new()
                            .title(title_and_thumb.0)
                            .description(new_desc)
                            .color(EMBED_COLOR);

                        if let Some(th) = title_and_thumb.1 {
                            ce = ce.thumbnail(th);
                        }

                        let edit_msg = serenity::builder::EditMessage::new().embed(ce);
                        let _ = mc.message.clone().edit(&ctx.http, edit_msg).await;

                        return;
                    }
                }
            }

            let _ = mc.create_response(&ctx.http, CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content("No active track to control.").ephemeral(true))).await;
        }
    }
}

#[tokio::main]
async fn main() {
    dotenv().ok();
    let token = env::var("DISCORD_TOKEN").expect("DISCORD_TOKEN not set");

    ensure_media_tools()
        .await
        .expect("Failed to prepare media tools (yt-dlp)");

    // Attempt to prepare an optional Spotify helper binary (librespot wrapper)
    if let Err(e) = crate::music::ensure_spotify_helper().await {
        eprintln!("Failed to prepare Spotify helper: {e:?}");
    }

    let intents = GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::DIRECT_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT
        | GatewayIntents::GUILDS
        | GatewayIntents::GUILD_MEMBERS
        | GatewayIntents::GUILD_VOICE_STATES;

    let mut client = Client::builder(token, intents)
        .register_songbird()
        .event_handler(Handler)
        .await
        .expect("Err creating client");

    // Initialize shared track handle storage and meta store
    {
        let mut data = client.data.write().await;
        data.insert::<TrackStore>(Arc::new(Mutex::new(HashMap::new())));
        data.insert::<TrackMetaStore>(Arc::new(Mutex::new(HashMap::new())));
    }

    if let Err(why) = client.start().await {
        eprintln!("Client error: {why:?}");
    }
}
