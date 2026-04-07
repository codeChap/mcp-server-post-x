mod api;
mod params;
mod server;

use api::AppConfig;
use rmcp::{ServiceExt, transport::stdio};
use server::PostXServer;
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

fn load_config() -> Result<AppConfig, Box<dyn std::error::Error>> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
    let path = PathBuf::from(home)
        .join(".config")
        .join("mcp-server-post-x")
        .join("config.toml");

    let content = std::fs::read_to_string(&path).map_err(|e| {
        format!(
            "Failed to read config file: {}\n\
             Create it with your X OAuth credentials.\n\n\
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
