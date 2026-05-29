mod api;
mod params;
mod server;

use api::AppConfig;
use rmcp::{ServiceExt, transport::stdio};
use server::PostXServer;
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

fn load_config() -> Result<AppConfig, Box<dyn std::error::Error>> {
    let path = resolve_config_path();

    // Support a pure environment-variable single-account mode when no config file exists.
    // This is extremely useful for containers, CI, and headless deployments.
    if !path.exists() {
        if let Some(config) = try_load_from_env() {
            tracing::info!(
                "Config loaded from environment variables (single account, no config file)"
            );
            return Ok(config);
        }
    }

    let content = std::fs::read_to_string(&path).map_err(|e| {
        format!(
            "Failed to read config file: {}\n\
             Create it with your X OAuth credentials, or use environment variables\n\
             (see POST_X_* vars below).\n\n\
             Example:\n\n\
             default_account = \"myaccount\"\n\n\
             [accounts.myaccount]\n\
             api_key = \"...\"\n\
             api_key_secret = \"...\"\n\
             access_token = \"...\"\n\
             access_token_secret = \"...\"\n\n\
             Get credentials at https://developer.x.com/\n\n\
             Error: {e}",
            path.display()
        )
    })?;

    let config = AppConfig::from_toml(&content)
        .map_err(|e| format!("Config error at {}: {e}", path.display()))?;

    tracing::info!(
        "Config loaded: {} account(s), default='{}' from {}",
        config.accounts.len(),
        config.default_account,
        path.display()
    );
    Ok(config)
}

/// Resolve the config file path with proper XDG_CONFIG_HOME support.
fn resolve_config_path() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.trim().is_empty() {
            return PathBuf::from(xdg)
                .join("mcp-server-post-x")
                .join("config.toml");
        }
    }

    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
    PathBuf::from(home)
        .join(".config")
        .join("mcp-server-post-x")
        .join("config.toml")
}

/// Attempt to construct a single-account config purely from environment variables.
/// Looks for POST_X_API_KEY, POST_X_API_KEY_SECRET, POST_X_ACCESS_TOKEN, POST_X_ACCESS_TOKEN_SECRET.
/// Optional: POST_X_ACCOUNT_NAME (defaults to "default").
fn try_load_from_env() -> Option<AppConfig> {
    let api_key = std::env::var("POST_X_API_KEY").ok()?.trim().to_string();
    let api_key_secret = std::env::var("POST_X_API_KEY_SECRET").ok()?.trim().to_string();
    let access_token = std::env::var("POST_X_ACCESS_TOKEN").ok()?.trim().to_string();
    let access_token_secret = std::env::var("POST_X_ACCESS_TOKEN_SECRET").ok()?.trim().to_string();

    if api_key.is_empty() || api_key_secret.is_empty() || access_token.is_empty() || access_token_secret.is_empty() {
        return None;
    }

    let account_name = std::env::var("POST_X_ACCOUNT_NAME")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "default".to_string());

    // Build a minimal TOML and reuse the existing validated parser. This guarantees
    // identical validation rules (no empty fields, etc.).
    let toml = format!(
        "default_account = \"{account_name}\"\n\n\
         [accounts.{account_name}]\n\
         api_key = \"{api_key}\"\n\
         api_key_secret = \"{api_key_secret}\"\n\
         access_token = \"{access_token}\"\n\
         access_token_secret = \"{access_token_secret}\"\n"
    );

    AppConfig::from_toml(&toml).ok()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let config = load_config()?;
    let server = PostXServer::new(config);
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
