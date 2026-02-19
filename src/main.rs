mod api;
mod params;
mod server;

use api::Config;
use rmcp::{ServiceExt, transport::stdio};
use server::PostXServer;
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

fn load_config() -> Result<Config, Box<dyn std::error::Error>> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
    let path = PathBuf::from(home)
        .join(".config")
        .join("mcp-server-post-x")
        .join("config.toml");

    let content = std::fs::read_to_string(&path).map_err(|e| {
        format!(
            "Failed to read config file: {}\n\
             Create it with your X OAuth credentials.\n\
             Example:\n\n\
             api_key = \"your-api-key\"\n\
             api_key_secret = \"your-api-key-secret\"\n\
             access_token = \"your-access-token\"\n\
             access_token_secret = \"your-access-token-secret\"\n\n\
             Get credentials at https://developer.x.com/\n\n\
             Error: {e}",
            path.display()
        )
    })?;

    let config: Config = toml::from_str(&content)
        .map_err(|e| format!("Failed to parse {}: {e}", path.display()))?;

    config
        .validate()
        .map_err(|e| format!("Invalid config at {}: {e}", path.display()))?;

    tracing::info!("Config loaded and validated from {}", path.display());
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
