use serde::Deserialize;
use std::collections::HashMap;
use std::io::ErrorKind;

#[derive(Debug, Deserialize)]
struct AppConfig {
    #[serde(default)]
    start: Option<StartConfig>,
}

#[derive(Debug, Deserialize)]
struct StartConfig {
    services: HashMap<String, ServiceConfig>,
}

#[derive(Debug, Deserialize)]
struct ServiceConfig {
    url: String,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    headers: Option<HashMap<String, String>>,
    #[serde(default)]
    body: Option<serde_json::Value>,
    #[serde(default)]
    args_field: Option<String>,
    #[serde(default)]
    timeout_secs: Option<u64>,
}

const CONFIG_PATH: &str = "config.jsonc";
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

async fn load_config() -> Result<StartConfig, Box<dyn std::error::Error + Send + Sync>> {
        let contents = match tokio::fs::read_to_string(CONFIG_PATH).await {
                Ok(s) => s,
                Err(e) if e.kind() == ErrorKind::NotFound => {
                        // Auto-create a default config if missing
                        tokio::fs::write(CONFIG_PATH, DEFAULT_CONFIG).await?;
                        DEFAULT_CONFIG.to_string()
                }
                Err(e) => return Err(Box::new(e)),
        };

        let cfg: AppConfig = json5::from_str(&contents)?;
        if let Some(start) = cfg.start {
                Ok(start)
        } else {
                Err("config.jsonc missing 'start' section".into())
        }
}

pub async fn handle_start(
    ctx: &serenity::prelude::Context,
    channel_id: serenity::all::ChannelId,
    args: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        channel_id
            .say(&ctx.http, "Usage: !is start <service> [args]")
            .await?;
        return Ok(());
    }

    let mut parts = trimmed.split_whitespace();
    let service_key = parts.next().unwrap_or("").to_string();
    let extra_args = parts.collect::<Vec<_>>().join(" ");

    let cfg = match load_config().await {
        Ok(c) => c,
        Err(e) => {
            channel_id
                .say(&ctx.http, format!(
                    "Config not found or invalid: {e}. Expected config.jsonc in working dir (auto-created)."
                ))
                .await?;
            return Ok(());
        }
    };

    let svc = match cfg.services.get(&service_key) {
        Some(s) => s,
        None => {
            let available = if cfg.services.is_empty() {
                "<none>".to_string()
            } else {
                cfg.services.keys().cloned().collect::<Vec<_>>().join(", ")
            };
            channel_id
                .say(
                    &ctx.http,
                    format!(
                        "Unknown service '{service_key}'. Available: {available}"
                    ),
                )
                .await?;
            return Ok(());
        }
    };

    let method = svc
        .method
        .as_deref()
        .unwrap_or("POST")
        .to_ascii_uppercase();
    if method != "POST" {
        channel_id
            .say(
                &ctx.http,
                format!("Service '{service_key}' uses unsupported method '{method}'. Only POST is supported."),
            )
            .await?;
        return Ok(());
    }

    // Build JSON body
    let mut body = match svc.body.clone().unwrap_or(serde_json::json!({})) {
        serde_json::Value::Object(map) => map,
        _ => serde_json::Map::new(),
    };

    if !extra_args.is_empty() {
        let key = svc.args_field.as_deref().unwrap_or("args");
        body.insert(key.to_string(), serde_json::Value::String(extra_args));
    }

    // Build client with optional timeout
    let mut client_builder = reqwest::Client::builder();
    if let Some(t) = svc.timeout_secs {
        client_builder = client_builder.timeout(std::time::Duration::from_secs(t));
    }
    let client = client_builder.build()?;

    let mut req = client.post(&svc.url);
    if let Some(hs) = &svc.headers {
        for (k, v) in hs {
            req = req.header(k, v);
        }
    }
    req = req.json(&body);

    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            channel_id
                .say(&ctx.http, format!("Request error for '{service_key}': {e}"))
                .await?;
            return Ok(());
        }
    };

    let status = resp.status();
    let text = resp.text().await.unwrap_or_else(|_| "<no body>".to_string());

    // Discord message length safety
    let mut preview = text.trim().to_string();
    if preview.is_empty() {
        preview = "<empty>".to_string();
    }
    let max_len = 1800usize; // leave room for header lines
    if preview.len() > max_len {
        preview.truncate(max_len);
        preview.push_str("... (truncated)");
    }

    let msg = format!(
        "Service: {service_key}\nURL: {}\nStatus: {}\nBody:\n{}",
        svc.url,
        status,
        preview
    );

    channel_id.say(&ctx.http, msg).await?;
    Ok(())
}