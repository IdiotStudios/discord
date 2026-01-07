use serde::{Deserialize, Serialize};
use serenity::prelude::*;
use serenity::model::id::GuildId;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

const MODALERT_PATH: &str = "modalerts.json";

pub struct ModAlertStore;
impl TypeMapKey for ModAlertStore {
    type Value = Arc<Mutex<HashSet<GuildId>>>;
}

#[derive(Serialize, Deserialize, Default)]
struct ModAlertDisk {
    enabled_guilds: Vec<u64>,
}

async fn load_disk() -> Result<HashSet<GuildId>, Box<dyn std::error::Error + Send + Sync>> {
    if !Path::new(MODALERT_PATH).exists() {
        // Create empty file
        let data = ModAlertDisk::default();
        let s = serde_json::to_string_pretty(&data)?;
        tokio::fs::write(MODALERT_PATH, s).await?;
        return Ok(HashSet::new());
    }

    let s = tokio::fs::read_to_string(MODALERT_PATH).await?;
    let data: ModAlertDisk = serde_json::from_str(&s)?;
    let set: HashSet<GuildId> = data.enabled_guilds.into_iter().map(GuildId::new).collect();
    Ok(set)
}

async fn save_disk(set: &HashSet<GuildId>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let data = ModAlertDisk {
        enabled_guilds: set.iter().map(|g| g.get()).collect(),
    };
    let s = serde_json::to_string_pretty(&data)?;
    tokio::fs::write(MODALERT_PATH, s).await?;
    Ok(())
}

pub async fn ensure_modalert_store(
    
) -> Result<Arc<Mutex<HashSet<GuildId>>>, Box<dyn std::error::Error + Send + Sync>> {
    let set = load_disk().await?;
    Ok(Arc::new(Mutex::new(set)))
}

pub async fn save_modalert_store(ctx: &Context) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let data = ctx.data.read().await;
    if let Some(store) = data.get::<ModAlertStore>() {
        let set = store.lock().await;
        save_disk(&set).await?
    }
    Ok(())
}

pub async fn is_modalert_enabled(ctx: &Context, gid: GuildId) -> bool {
    let data = ctx.data.read().await;
    if let Some(store) = data.get::<ModAlertStore>() {
        let set = store.lock().await;
        set.contains(&gid)
    } else {
        false
    }
}
