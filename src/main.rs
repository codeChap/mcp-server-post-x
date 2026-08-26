mod api;
mod params;
mod server;

use api::AppConfig;
use rmcp::{transport::stdio, ServiceExt};
use server::XServer;
use std::path::{Path, PathBuf};
use tracing_subscriber::EnvFilter;

const CONFIG_DIR_NEW: &str = "mcp-server-x";
const CONFIG_DIR_LEGACY: &str = "mcp-server-post-x";

fn load_config() -> Result<(AppConfig, Option<PathBuf>), Box<dyn std::error::Error>> {
    let path = resolve_config_path();

    // Support a pure environment-variable single-account mode when no config file exists.
    // This is extremely useful for containers, CI, and headless deployments.
    if !path.exists() {
        if let Some(config) = try_load_from_env() {
            tracing::info!(
                "Config loaded from environment variables (single account, no config file)"
            );
            return Ok((config, None));
        }
    }

    let content = std::fs::read_to_string(&path).map_err(|e| {
        format!(
            "Failed to read config file: {}\n\
             Create it with your X OAuth credentials, or use environment variables\n\
             (X_API_KEY, X_API_KEY_SECRET, X_ACCESS_TOKEN, X_ACCESS_TOKEN_SECRET;\n\
             POST_X_* is still accepted).\n\n\
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
    Ok((config, Some(path)))
}

fn config_home() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.trim().is_empty() {
            return PathBuf::from(xdg);
        }
    }

    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
    PathBuf::from(home).join(".config")
}

fn resolve_config_path() -> PathBuf {
    resolve_config_path_from(&config_home())
}

/// Prefer `mcp-server-x/config.toml`; fall back to the legacy `mcp-server-post-x` path.
fn resolve_config_path_from(config_home: &Path) -> PathBuf {
    let new_path = config_home.join(CONFIG_DIR_NEW).join("config.toml");
    if new_path.exists() {
        return new_path;
    }
    let legacy_path = config_home.join(CONFIG_DIR_LEGACY).join("config.toml");
    if legacy_path.exists() {
        return legacy_path;
    }
    new_path
}

fn first_nonempty(keys: &[&str], get: impl Fn(&str) -> Option<String>) -> Option<String> {
    for key in keys {
        if let Some(v) = get(key) {
            let t = v.trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
    }
    None
}

fn env_prefer(new: &str, old: &str) -> Option<String> {
    first_nonempty(&[new, old], |k| std::env::var(k).ok())
}

/// Attempt to construct a single-account config purely from environment variables.
/// Prefers X_API_KEY / X_API_KEY_SECRET / X_ACCESS_TOKEN / X_ACCESS_TOKEN_SECRET,
/// then the legacy POST_X_* names. Optional: X_ACCOUNT_NAME / POST_X_ACCOUNT_NAME
/// (defaults to "default").
fn try_load_from_env() -> Option<AppConfig> {
    let api_key = env_prefer("X_API_KEY", "POST_X_API_KEY")?;
    let api_key_secret = env_prefer("X_API_KEY_SECRET", "POST_X_API_KEY_SECRET")?;
    let access_token = env_prefer("X_ACCESS_TOKEN", "POST_X_ACCESS_TOKEN")?;
    let access_token_secret = env_prefer("X_ACCESS_TOKEN_SECRET", "POST_X_ACCESS_TOKEN_SECRET")?;

    let account_name = env_prefer("X_ACCOUNT_NAME", "POST_X_ACCOUNT_NAME")
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

    let (config, config_path) = load_config()?;
    let server = XServer::new(config, config_path);
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_base(label: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "mcp-server-x-{}-{}-{}",
            label,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn config_path_defaults_to_new_name() {
        let base = temp_base("none");
        let path = resolve_config_path_from(&base);
        assert_eq!(path, base.join("mcp-server-x").join("config.toml"));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn config_path_falls_back_to_legacy() {
        let base = temp_base("legacy");
        let legacy_dir = base.join("mcp-server-post-x");
        fs::create_dir_all(&legacy_dir).unwrap();
        fs::write(legacy_dir.join("config.toml"), "placeholder").unwrap();

        let path = resolve_config_path_from(&base);
        assert_eq!(path, legacy_dir.join("config.toml"));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn config_path_prefers_new_over_legacy() {
        let base = temp_base("both");
        let new_dir = base.join("mcp-server-x");
        let legacy_dir = base.join("mcp-server-post-x");
        fs::create_dir_all(&new_dir).unwrap();
        fs::create_dir_all(&legacy_dir).unwrap();
        fs::write(new_dir.join("config.toml"), "new").unwrap();
        fs::write(legacy_dir.join("config.toml"), "old").unwrap();

        let path = resolve_config_path_from(&base);
        assert_eq!(path, new_dir.join("config.toml"));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn first_nonempty_prefers_new_key() {
        let got = first_nonempty(&["X_API_KEY", "POST_X_API_KEY"], |k| match k {
            "X_API_KEY" => Some("  new  ".into()),
            "POST_X_API_KEY" => Some("old".into()),
            _ => None,
        });
        assert_eq!(got.as_deref(), Some("new"));
    }

    #[test]
    fn first_nonempty_skips_blank_and_uses_legacy() {
        let got = first_nonempty(&["X_API_KEY", "POST_X_API_KEY"], |k| match k {
            "X_API_KEY" => Some("   ".into()),
            "POST_X_API_KEY" => Some("legacy".into()),
            _ => None,
        });
        assert_eq!(got.as_deref(), Some("legacy"));
    }
}
