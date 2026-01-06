use serde::Deserialize;
use std::{collections::HashMap, io::ErrorKind};

pub const CONFIG_PATH: &str = "config.jsonc";

const DEFAULT_CONFIG: &str = r#"// Global bot config (JSONC: supports comments)
{
  // Start command configuration
  "start": {
    "services": {
      // Example Minecraft service
      "mc": {
        "url": "http://localhost:8080/start",
        "method": "POST",
        "headers": {
          "Content-Type": "application/json"
        },
        "body": { "action": "start" },
        "args_field": "args",
        "timeout_secs": 10
      }
    }
  }
}
"#;

#[derive(Debug, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub start: Option<StartConfig>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct StartConfig {
    pub services: HashMap<String, ServiceConfig>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ServiceConfig {
    pub url: String,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub headers: Option<HashMap<String, String>>,
    #[serde(default)]
    pub body: Option<serde_json::Value>,
    #[serde(default)]
    pub args_field: Option<String>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

pub async fn ensure_default_config() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match tokio::fs::metadata(CONFIG_PATH).await {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == ErrorKind::NotFound => {
            tokio::fs::write(CONFIG_PATH, DEFAULT_CONFIG).await?;
            Ok(())
        }
        Err(e) => Err(Box::new(e)),
    }
}

pub async fn load_config() -> Result<AppConfig, Box<dyn std::error::Error + Send + Sync>> {
    let _ = ensure_default_config().await;

    let contents = tokio::fs::read_to_string(CONFIG_PATH).await?;
    let cfg: AppConfig = json5::from_str(&contents)?;
    Ok(cfg)
}
