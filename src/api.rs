use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use hmac::{Hmac, Mac};
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use rand::Rng;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const TWEETS_URL: &str = "https://api.x.com/2/tweets";
const MEDIA_UPLOAD_URL: &str = "https://upload.twitter.com/1.1/media/upload.json";
const ME_URL: &str = "https://api.x.com/2/users/me";

const ALLOWED_MIME_TYPES: &[&str] = &["image/jpeg", "image/png", "image/gif", "image/webp"];
const MAX_MEDIA_SIZE: u64 = 5 * 1024 * 1024; // 5MB
const MAX_TWEET_LENGTH: usize = 280;
const MAX_THREAD_LENGTH: usize = 25;

/// RFC 3986 unreserved characters — everything else gets percent-encoded.
const RFC3986: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'!')
    .add(b'#')
    .add(b'$')
    .add(b'%')
    .add(b'&')
    .add(b'\'')
    .add(b'(')
    .add(b')')
    .add(b'*')
    .add(b'+')
    .add(b',')
    .add(b'/')
    .add(b':')
    .add(b';')
    .add(b'=')
    .add(b'?')
    .add(b'@')
    .add(b'[')
    .add(b']');

type HmacSha1 = Hmac<sha1::Sha1>;

#[derive(Clone, Deserialize)]
pub struct Config {
    pub api_key: String,
    pub api_key_secret: String,
    pub access_token: String,
    pub access_token_secret: String,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("api_key", &"***REDACTED***")
            .field("api_key_secret", &"***REDACTED***")
            .field("access_token", &"***REDACTED***")
            .field("access_token_secret", &"***REDACTED***")
            .finish()
    }
}

impl Config {
    pub fn validate(&self) -> Result<(), String> {
        if self.api_key.trim().is_empty() {
            return Err("'api_key' is empty in config".into());
        }
        if self.api_key_secret.trim().is_empty() {
            return Err("'api_key_secret' is empty in config".into());
        }
        if self.access_token.trim().is_empty() {
            return Err("'access_token' is empty in config".into());
        }
        if self.access_token_secret.trim().is_empty() {
            return Err("'access_token_secret' is empty in config".into());
        }
        Ok(())
    }
}

pub struct XClient {
    config: Config,
    http: Client,
}

#[derive(Serialize)]
struct TweetBody {
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    media: Option<TweetMedia>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reply: Option<TweetReply>,
}

#[derive(Serialize)]
struct TweetMedia {
    media_ids: Vec<String>,
}

#[derive(Serialize)]
struct TweetReply {
    in_reply_to_tweet_id: String,
}

#[derive(Deserialize)]
struct TweetResponse {
    data: TweetData,
}

#[derive(Deserialize)]
struct TweetData {
    id: String,
}

#[derive(Deserialize)]
struct MediaResponse {
    media_id_string: String,
}

#[derive(Deserialize)]
pub struct MeResponse {
    pub data: MeData,
}

#[derive(Deserialize)]
pub struct MeData {
    pub id: String,
    pub name: String,
    pub username: String,
}

pub struct PostResult {
    pub tweet_id: String,
    pub url: String,
}

pub struct ThreadResult {
    pub posted: Vec<PostResult>,
    pub error: Option<String>,
}

impl XClient {
    pub fn new(config: Config) -> Self {
        let http = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("failed to build HTTP client");
        Self { config, http }
    }

    pub async fn get_me(&self) -> Result<MeData, String> {
        let auth = self.oauth_header("GET", ME_URL, &BTreeMap::new());
        let resp = self
            .http
            .get(ME_URL)
            .header("Authorization", auth)
            .send()
            .await
            .map_err(|e| format!("HTTP request failed: {e}"))?;

        self.check_auth_error(&resp);
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("X API error ({status}): {body}"));
        }

        let me: MeResponse = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse response: {e}"))?;
        Ok(me.data)
    }

    pub async fn post_tweet(
        &self,
        text: &str,
        image_path: Option<&str>,
        reply_to: Option<&str>,
        username: &str,
    ) -> Result<PostResult, String> {
        self.validate_tweet_text(text)?;

        let media_id = match image_path {
            Some(path) => Some(self.upload_media(path).await?),
            None => None,
        };

        let body = TweetBody {
            text: text.to_string(),
            media: media_id.map(|id| TweetMedia {
                media_ids: vec![id],
            }),
            reply: reply_to.map(|id| TweetReply {
                in_reply_to_tweet_id: id.to_string(),
            }),
        };

        let auth = self.oauth_header("POST", TWEETS_URL, &BTreeMap::new());
        let resp = self
            .http
            .post(TWEETS_URL)
            .header("Authorization", auth)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("HTTP request failed: {e}"))?;

        self.check_auth_error(&resp);
        let status = resp.status();
        if status.as_u16() == 429 {
            let reset = self.rate_limit_reset(&resp);
            return Err(format!(
                "Rate limited (429). {}Try again later. Free tier allows ~17 tweets/24h.",
                reset
            ));
        }
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(format!("X API error ({status}): {body_text}"));
        }

        let tweet: TweetResponse = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse tweet response: {e}"))?;

        Ok(PostResult {
            url: format!("https://x.com/{}/status/{}", username, tweet.data.id),
            tweet_id: tweet.data.id,
        })
    }

    pub async fn post_thread(
        &self,
        tweets: &[(String, Option<String>)],
        username: &str,
    ) -> ThreadResult {
        if tweets.is_empty() {
            return ThreadResult {
                posted: vec![],
                error: Some("Thread must contain at least one tweet".into()),
            };
        }
        if tweets.len() > MAX_THREAD_LENGTH {
            return ThreadResult {
                posted: vec![],
                error: Some(format!("Thread exceeds maximum of {MAX_THREAD_LENGTH} tweets")),
            };
        }

        let mut posted = Vec::new();
        let mut reply_to: Option<String> = None;

        for (i, (text, image)) in tweets.iter().enumerate() {
            if i > 0 {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }

            let result = self
                .post_tweet(
                    text,
                    image.as_deref(),
                    reply_to.as_deref(),
                    username,
                )
                .await;

            match result {
                Ok(post) => {
                    reply_to = Some(post.tweet_id.clone());
                    posted.push(post);
                }
                Err(e) => {
                    return ThreadResult {
                        posted,
                        error: Some(format!("Tweet {} of {} failed: {e}", i + 1, tweets.len())),
                    };
                }
            }
        }

        ThreadResult {
            posted,
            error: None,
        }
    }

    async fn upload_media(&self, path: &str) -> Result<String, String> {
        let file_path = Path::new(path);
        if !file_path.exists() {
            return Err(format!("File not found: {path}"));
        }

        let metadata = std::fs::metadata(file_path)
            .map_err(|e| format!("Cannot read file metadata: {e}"))?;
        if metadata.len() > MAX_MEDIA_SIZE {
            return Err(format!(
                "File too large: {} bytes (max {}MB)",
                metadata.len(),
                MAX_MEDIA_SIZE / (1024 * 1024)
            ));
        }

        let mime = mime_from_path(file_path)?;
        let file_bytes = std::fs::read(file_path)
            .map_err(|e| format!("Failed to read file: {e}"))?;
        let file_name = file_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let part = reqwest::multipart::Part::bytes(file_bytes)
            .file_name(file_name)
            .mime_str(&mime)
            .map_err(|e| format!("Invalid MIME type: {e}"))?;
        let form = reqwest::multipart::Form::new().part("media", part);

        // For multipart uploads, only OAuth params go in signature (no body params)
        let auth = self.oauth_header("POST", MEDIA_UPLOAD_URL, &BTreeMap::new());
        let resp = self
            .http
            .post(MEDIA_UPLOAD_URL)
            .header("Authorization", auth)
            .multipart(form)
            .send()
            .await
            .map_err(|e| format!("Media upload failed: {e}"))?;

        self.check_auth_error(&resp);
        let status = resp.status();
        if status.as_u16() == 429 {
            let reset = self.rate_limit_reset(&resp);
            return Err(format!(
                "Media upload rate limited (429). {}Media uploads may have separate rate limits.",
                reset
            ));
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("Media upload error ({status}): {body}"));
        }

        let media: MediaResponse = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse media response: {e}"))?;
        Ok(media.media_id_string)
    }

    fn validate_tweet_text(&self, text: &str) -> Result<(), String> {
        if text.trim().is_empty() {
            return Err("Tweet text cannot be empty".into());
        }
        if text.chars().count() > MAX_TWEET_LENGTH {
            return Err(format!(
                "Tweet text is {} characters (max {MAX_TWEET_LENGTH})",
                text.chars().count()
            ));
        }
        Ok(())
    }

    fn rate_limit_reset(&self, resp: &reqwest::Response) -> String {
        if let Some(reset) = resp.headers().get("x-rate-limit-reset") {
            if let Ok(val) = reset.to_str() {
                return format!("Rate limit resets at timestamp {val}. ");
            }
        }
        String::new()
    }

    fn check_auth_error(&self, resp: &reqwest::Response) {
        if resp.status().as_u16() == 401 {
            tracing::error!(
                "Received 401 Unauthorized from X API. \
                 Your OAuth credentials may be revoked or invalid. \
                 Regenerate them at https://developer.x.com/"
            );
        }
    }

    // --- OAuth 1.0a ---

    fn oauth_header(&self, method: &str, url: &str, extra_params: &BTreeMap<String, String>) -> String {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .to_string();

        let nonce = {
            let mut bytes = [0u8; 16];
            rand::thread_rng().fill(&mut bytes);
            hex::encode(bytes)
        };

        let mut params = BTreeMap::new();
        params.insert("oauth_consumer_key".into(), self.config.api_key.clone());
        params.insert("oauth_nonce".into(), nonce);
        params.insert("oauth_signature_method".into(), "HMAC-SHA1".into());
        params.insert("oauth_timestamp".into(), timestamp);
        params.insert("oauth_token".into(), self.config.access_token.clone());
        params.insert("oauth_version".into(), "1.0".into());

        for (k, v) in extra_params {
            params.insert(k.clone(), v.clone());
        }

        let base_string = Self::signature_base_string(method, url, &params);
        let signing_key = format!(
            "{}&{}",
            pct_encode(&self.config.api_key_secret),
            pct_encode(&self.config.access_token_secret)
        );

        let mut mac = HmacSha1::new_from_slice(signing_key.as_bytes())
            .expect("HMAC accepts any key length");
        mac.update(base_string.as_bytes());
        let signature = BASE64.encode(mac.finalize().into_bytes());

        params.insert("oauth_signature".into(), signature);

        let header_parts: Vec<String> = params
            .iter()
            .filter(|(k, _)| k.starts_with("oauth_"))
            .map(|(k, v)| format!("{}=\"{}\"", pct_encode(k), pct_encode(v)))
            .collect();

        format!("OAuth {}", header_parts.join(", "))
    }

    fn signature_base_string(method: &str, url: &str, params: &BTreeMap<String, String>) -> String {
        let param_string: String = params
            .iter()
            .map(|(k, v)| format!("{}={}", pct_encode(k), pct_encode(v)))
            .collect::<Vec<_>>()
            .join("&");

        format!(
            "{}&{}&{}",
            method.to_uppercase(),
            pct_encode(url),
            pct_encode(&param_string)
        )
    }
}

fn pct_encode(input: &str) -> String {
    utf8_percent_encode(input, RFC3986).to_string()
}

fn mime_from_path(path: &Path) -> Result<String, String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let mime = match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => return Err(format!(
            "Unsupported image format '.{ext}'. Allowed: jpeg, png, gif, webp"
        )),
    };

    if !ALLOWED_MIME_TYPES.contains(&mime) {
        return Err(format!("MIME type {mime} is not allowed"));
    }

    Ok(mime.to_string())
}

/// Hex encoding for nonce — avoids adding a full crate dependency.
mod hex {
    pub fn encode(bytes: impl AsRef<[u8]>) -> String {
        bytes
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }
}
