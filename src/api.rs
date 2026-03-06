use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use hmac::{Hmac, Mac};
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use rand::Rng;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::io::Read;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const TWEETS_URL: &str = "https://api.x.com/2/tweets";
const MEDIA_UPLOAD_URL: &str = "https://upload.twitter.com/1.1/media/upload.json";
const MEDIA_METADATA_URL: &str = "https://upload.twitter.com/1.1/media/metadata/create.json";
const ME_URL: &str = "https://api.x.com/2/users/me";

const MAX_TWEET_LENGTH: usize = 280;
const MAX_THREAD_LENGTH: usize = 25;
const MAX_RETRIES: u32 = 3;
const RETRY_BASE_DELAY_MS: u64 = 1000;
const CHUNK_SIZE: usize = 5 * 1024 * 1024; // 5MB per chunk
const MAX_PROCESSING_WAIT_SECS: u64 = 600; // 10 minutes

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

// --- Media types ---

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MediaType {
    Image,
    AnimatedGif,
    Video,
}

impl MediaType {
    fn media_category(&self) -> &'static str {
        match self {
            MediaType::Image => "tweet_image",
            MediaType::AnimatedGif => "tweet_gif",
            MediaType::Video => "tweet_video",
        }
    }
}

impl fmt::Display for MediaType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MediaType::Image => write!(f, "image"),
            MediaType::AnimatedGif => write!(f, "animated_gif"),
            MediaType::Video => write!(f, "video"),
        }
    }
}

struct MediaInfo {
    mime: String,
    media_type: MediaType,
    max_size: u64,
}

pub struct MediaAttachment {
    pub path: String,
    pub alt_text: Option<String>,
}

pub struct MediaUploadResult {
    pub media_id: String,
    pub media_type: String,
    pub state: String,
}

// --- API response types ---

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
struct SimpleMediaResponse {
    media_id_string: String,
}

#[derive(Deserialize)]
struct ChunkedMediaResponse {
    media_id_string: String,
    #[serde(default)]
    processing_info: Option<ProcessingInfo>,
}

#[derive(Deserialize)]
struct ProcessingInfo {
    state: String,
    check_after_secs: Option<u64>,
    progress_percent: Option<u32>,
    error: Option<ProcessingError>,
}

#[derive(Deserialize)]
struct ProcessingError {
    message: String,
}

#[derive(Deserialize)]
pub struct MeResponse {
    pub data: MeData,
}

#[derive(Clone, Deserialize)]
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

// --- Follows response types ---

#[derive(Deserialize)]
struct FollowsResponse {
    data: Option<Vec<UserSummary>>,
    meta: Option<FollowsMeta>,
}

#[derive(Deserialize)]
pub struct UserSummary {
    pub id: String,
    pub name: String,
    pub username: String,
    pub description: Option<String>,
    pub public_metrics: Option<PublicMetrics>,
}

#[derive(Deserialize)]
pub struct PublicMetrics {
    pub followers_count: u64,
    pub following_count: u64,
    pub tweet_count: u64,
}

#[derive(Deserialize)]
pub struct FollowsMeta {
    pub result_count: Option<u32>,
    pub next_token: Option<String>,
    pub previous_token: Option<String>,
}

pub struct FollowsResult {
    pub users: Vec<UserSummary>,
    pub next_token: Option<String>,
}

// --- User lookup response types ---

#[derive(Deserialize)]
struct UserLookupResponse {
    data: Option<UserProfile>,
}

#[derive(Deserialize)]
pub struct UserProfile {
    pub id: String,
    pub name: String,
    pub username: String,
    pub description: Option<String>,
    pub location: Option<String>,
    pub url: Option<String>,
    pub profile_image_url: Option<String>,
    pub protected: Option<bool>,
    pub verified: Option<bool>,
    pub verified_type: Option<String>,
    pub created_at: Option<String>,
    pub public_metrics: Option<PublicMetrics>,
}

// --- Like response types ---

#[derive(Deserialize)]
struct LikeResponse {
    data: LikeData,
}

#[derive(Deserialize)]
struct LikeData {
    liked: bool,
}

// --- Delete tweet response types ---

#[derive(Deserialize)]
struct DeleteTweetResponse {
    data: DeleteTweetData,
}

#[derive(Deserialize)]
struct DeleteTweetData {
    deleted: bool,
}

// --- Retweet response types ---

#[derive(Deserialize)]
struct RetweetResponse {
    data: RetweetData,
}

#[derive(Deserialize)]
struct RetweetData {
    retweeted: bool,
}

// --- Search response types ---

#[derive(Deserialize)]
struct SearchResponse {
    data: Option<Vec<SearchTweet>>,
    includes: Option<SearchIncludes>,
    meta: Option<SearchMeta>,
}

#[derive(Deserialize)]
struct SearchTweet {
    id: String,
    text: String,
    author_id: Option<String>,
    created_at: Option<String>,
    public_metrics: Option<TweetPublicMetrics>,
}

#[derive(Deserialize)]
struct TweetPublicMetrics {
    like_count: u64,
    retweet_count: u64,
    reply_count: u64,
}

#[derive(Deserialize)]
struct SearchIncludes {
    users: Option<Vec<SearchUser>>,
}

#[derive(Deserialize)]
struct SearchUser {
    id: String,
    username: String,
}

#[derive(Deserialize)]
struct SearchMeta {
    next_token: Option<String>,
}

pub struct SearchResult {
    pub tweets: Vec<SearchTweetResult>,
    pub next_token: Option<String>,
}

pub struct SearchTweetResult {
    pub id: String,
    pub text: String,
    pub username: Option<String>,
    pub created_at: Option<String>,
    pub like_count: u64,
    pub retweet_count: u64,
    pub reply_count: u64,
}

// --- XClient implementation ---

impl XClient {
    pub fn new(config: Config) -> Self {
        let http = Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .expect("failed to build HTTP client");
        Self { config, http }
    }

    pub async fn get_me(&self) -> Result<MeData, String> {
        let resp = self
            .retry_503(|| {
                let auth = self.oauth_header("GET", ME_URL, &BTreeMap::new());
                self.http.get(ME_URL).header("Authorization", auth)
            })
            .await?;

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

    // --- Media upload (public) ---

    pub async fn upload_media(
        &self,
        path: &str,
        alt_text: Option<&str>,
    ) -> Result<MediaUploadResult, String> {
        let file_path = Path::new(path);
        if !file_path.exists() {
            return Err(format!("File not found: {path}"));
        }

        let metadata =
            std::fs::metadata(file_path).map_err(|e| format!("Cannot read file metadata: {e}"))?;
        let file_size = metadata.len();

        let info = media_info_from_path(file_path)?;

        if file_size > info.max_size {
            return Err(format!(
                "File too large: {} bytes (max {}MB for {})",
                file_size,
                info.max_size / (1024 * 1024),
                info.media_type
            ));
        }

        if alt_text.is_some() && info.media_type == MediaType::Video {
            return Err("Alt text is not supported for videos (use subtitles instead)".into());
        }

        let (media_id, state) =
            if info.media_type == MediaType::Image && file_size <= 5 * 1024 * 1024 {
                // Simple upload for small images
                let id = self.simple_upload(file_path, &info.mime).await?;
                (id, "succeeded".to_string())
            } else {
                // Chunked upload for videos, GIFs, and large images
                let media_id = self
                    .chunked_upload_init(file_size, &info.mime, info.media_type.media_category())
                    .await?;

                self.chunked_upload_append(&media_id, file_path, file_size)
                    .await?;

                let finalize_resp = self.chunked_upload_finalize(&media_id).await?;

                let state = if finalize_resp.processing_info.is_some() {
                    self.poll_processing_status(&media_id).await?
                } else {
                    "succeeded".to_string()
                };

                (media_id, state)
            };

        // Set alt text if provided (images and GIFs only — video already rejected above)
        if let Some(alt) = alt_text {
            self.set_media_alt_text(&media_id, alt).await?;
        }

        Ok(MediaUploadResult {
            media_id,
            media_type: info.media_type.to_string(),
            state,
        })
    }

    // --- Tweet posting ---

    pub async fn post_tweet(
        &self,
        text: &str,
        media: &[MediaAttachment],
        media_ids: Option<&[String]>,
        reply_to: Option<&str>,
        username: &str,
    ) -> Result<PostResult, String> {
        self.validate_tweet_text(text)?;

        let resolved_ids = if !media.is_empty() {
            // Pre-flight validation: check all files before uploading any
            let mut infos = Vec::new();
            for attachment in media {
                let path = Path::new(&attachment.path);
                if !path.exists() {
                    return Err(format!("File not found: {}", attachment.path));
                }
                let info = media_info_from_path(path)?;
                let file_size = std::fs::metadata(path)
                    .map_err(|e| format!("Cannot read file metadata: {e}"))?
                    .len();
                if file_size > info.max_size {
                    return Err(format!(
                        "File too large: {} ({} bytes, max {}MB)",
                        attachment.path,
                        file_size,
                        info.max_size / (1024 * 1024)
                    ));
                }
                if attachment.alt_text.is_some() && info.media_type == MediaType::Video {
                    return Err("Alt text is not supported for videos".into());
                }
                infos.push(info);
            }
            validate_media_combination(&infos)?;

            // Upload each attachment
            let mut ids = Vec::new();
            for attachment in media {
                let result = self
                    .upload_media(&attachment.path, attachment.alt_text.as_deref())
                    .await?;
                if result.state != "succeeded" {
                    return Err(format!(
                        "Media processing {}: {}",
                        result.state, attachment.path
                    ));
                }
                ids.push(result.media_id);
            }
            Some(ids)
        } else if let Some(ids) = media_ids {
            Some(ids.to_vec())
        } else {
            None
        };

        let body = TweetBody {
            text: text.to_string(),
            media: resolved_ids.map(|ids| TweetMedia { media_ids: ids }),
            reply: reply_to.map(|id| TweetReply {
                in_reply_to_tweet_id: id.to_string(),
            }),
        };

        let resp = self
            .retry_503(|| {
                let auth = self.oauth_header("POST", TWEETS_URL, &BTreeMap::new());
                self.http
                    .post(TWEETS_URL)
                    .header("Authorization", auth)
                    .json(&body)
            })
            .await?;

        self.check_auth_error(&resp);
        let status = resp.status();
        if status.as_u16() == 429 {
            let reset = self.rate_limit_reset(&resp);
            return Err(format!(
                "Rate limited (429). {}Try again later.",
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
        tweets: &[(String, Vec<MediaAttachment>)],
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
                error: Some(format!(
                    "Thread exceeds maximum of {MAX_THREAD_LENGTH} tweets"
                )),
            };
        }

        let mut posted = Vec::new();
        let mut reply_to: Option<String> = None;

        for (i, (text, media)) in tweets.iter().enumerate() {
            if i > 0 {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }

            let result = self
                .post_tweet(text, media, None, reply_to.as_deref(), username)
                .await;

            match result {
                Ok(post) => {
                    reply_to = Some(post.tweet_id.clone());
                    posted.push(post);
                }
                Err(e) => {
                    return ThreadResult {
                        posted,
                        error: Some(format!(
                            "Tweet {} of {} failed: {e}",
                            i + 1,
                            tweets.len()
                        )),
                    };
                }
            }
        }

        ThreadResult {
            posted,
            error: None,
        }
    }

    // --- Follows lookup ---

    pub async fn get_followers(
        &self,
        user_id: &str,
        max_results: u32,
        pagination_token: Option<&str>,
    ) -> Result<FollowsResult, String> {
        let url = format!("https://api.x.com/2/users/{user_id}/followers");
        self.get_follows(&url, max_results, pagination_token).await
    }

    pub async fn get_following(
        &self,
        user_id: &str,
        max_results: u32,
        pagination_token: Option<&str>,
    ) -> Result<FollowsResult, String> {
        let url = format!("https://api.x.com/2/users/{user_id}/following");
        self.get_follows(&url, max_results, pagination_token).await
    }

    async fn get_follows(
        &self,
        base_url: &str,
        max_results: u32,
        pagination_token: Option<&str>,
    ) -> Result<FollowsResult, String> {
        let mut params = BTreeMap::new();
        params.insert("max_results".to_string(), max_results.to_string());
        params.insert(
            "user.fields".to_string(),
            "id,name,username,description,public_metrics".to_string(),
        );
        if let Some(token) = pagination_token {
            params.insert("pagination_token".to_string(), token.to_string());
        }

        // Build query string for the URL
        let query_string: String = params
            .iter()
            .map(|(k, v)| format!("{}={}", pct_encode(k), pct_encode(v)))
            .collect::<Vec<_>>()
            .join("&");
        let full_url = format!("{base_url}?{query_string}");

        let resp = self
            .retry_503(|| {
                // GET query params are included in OAuth signature
                let auth = self.oauth_header("GET", base_url, &params);
                self.http.get(&full_url).header("Authorization", auth)
            })
            .await?;

        self.check_auth_error(&resp);
        let status = resp.status();
        if status.as_u16() == 429 {
            let reset = self.rate_limit_reset(&resp);
            return Err(format!("Rate limited (429). {reset}Try again later."));
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("X API error ({status}): {body}"));
        }

        let response: FollowsResponse = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse follows response: {e}"))?;

        Ok(FollowsResult {
            users: response.data.unwrap_or_default(),
            next_token: response.meta.and_then(|m| m.next_token),
        })
    }

    // --- User lookup ---

    pub async fn lookup_user_by_username(
        &self,
        username: &str,
    ) -> Result<UserProfile, String> {
        let url = format!("https://api.x.com/2/users/by/username/{username}");
        self.get_user_profile(&url).await
    }

    pub async fn lookup_user_by_id(&self, id: &str) -> Result<UserProfile, String> {
        let url = format!("https://api.x.com/2/users/{id}");
        self.get_user_profile(&url).await
    }

    async fn get_user_profile(&self, base_url: &str) -> Result<UserProfile, String> {
        let mut params = BTreeMap::new();
        params.insert(
            "user.fields".to_string(),
            "id,name,username,description,location,url,profile_image_url,\
             protected,verified,verified_type,created_at,public_metrics"
                .to_string(),
        );

        let query_string: String = params
            .iter()
            .map(|(k, v)| format!("{}={}", pct_encode(k), pct_encode(v)))
            .collect::<Vec<_>>()
            .join("&");
        let full_url = format!("{base_url}?{query_string}");

        let resp = self
            .retry_503(|| {
                let auth = self.oauth_header("GET", base_url, &params);
                self.http.get(&full_url).header("Authorization", auth)
            })
            .await?;

        self.check_auth_error(&resp);
        let status = resp.status();
        if status.as_u16() == 429 {
            let reset = self.rate_limit_reset(&resp);
            return Err(format!("Rate limited (429). {reset}Try again later."));
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("X API error ({status}): {body}"));
        }

        let response: UserLookupResponse = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse user lookup response: {e}"))?;

        response.data.ok_or_else(|| "User not found".to_string())
    }

    // --- Likes ---

    pub async fn like_tweet(&self, user_id: &str, tweet_id: &str) -> Result<bool, String> {
        let url = format!("https://api.x.com/2/users/{user_id}/likes");
        let body = serde_json::json!({ "tweet_id": tweet_id });

        let resp = self
            .retry_503(|| {
                let auth = self.oauth_header("POST", &url, &BTreeMap::new());
                self.http
                    .post(&url)
                    .header("Authorization", auth)
                    .json(&body)
            })
            .await?;

        self.check_auth_error(&resp);
        let status = resp.status();
        if status.as_u16() == 429 {
            let reset = self.rate_limit_reset(&resp);
            return Err(format!("Rate limited (429). {reset}Try again later."));
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("X API error ({status}): {body}"));
        }

        let response: LikeResponse = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse like response: {e}"))?;

        Ok(response.data.liked)
    }

    pub async fn unlike_tweet(&self, user_id: &str, tweet_id: &str) -> Result<bool, String> {
        let url = format!("https://api.x.com/2/users/{user_id}/likes/{tweet_id}");

        let resp = self
            .retry_503(|| {
                let auth = self.oauth_header("DELETE", &url, &BTreeMap::new());
                self.http
                    .delete(&url)
                    .header("Authorization", auth)
            })
            .await?;

        self.check_auth_error(&resp);
        let status = resp.status();
        if status.as_u16() == 429 {
            let reset = self.rate_limit_reset(&resp);
            return Err(format!("Rate limited (429). {reset}Try again later."));
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("X API error ({status}): {body}"));
        }

        let response: LikeResponse = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse unlike response: {e}"))?;

        Ok(response.data.liked)
    }

    // --- Delete tweet ---

    pub async fn delete_tweet(&self, tweet_id: &str) -> Result<bool, String> {
        let url = format!("{TWEETS_URL}/{tweet_id}");

        let resp = self
            .retry_503(|| {
                let auth = self.oauth_header("DELETE", &url, &BTreeMap::new());
                self.http.delete(&url).header("Authorization", auth)
            })
            .await?;

        self.check_auth_error(&resp);
        let status = resp.status();
        if status.as_u16() == 429 {
            let reset = self.rate_limit_reset(&resp);
            return Err(format!("Rate limited (429). {reset}Try again later."));
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("X API error ({status}): {body}"));
        }

        let response: DeleteTweetResponse = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse delete response: {e}"))?;

        Ok(response.data.deleted)
    }

    // --- Retweets ---

    pub async fn retweet(&self, user_id: &str, tweet_id: &str) -> Result<bool, String> {
        let url = format!("https://api.x.com/2/users/{user_id}/retweets");
        let body = serde_json::json!({ "tweet_id": tweet_id });

        let resp = self
            .retry_503(|| {
                let auth = self.oauth_header("POST", &url, &BTreeMap::new());
                self.http
                    .post(&url)
                    .header("Authorization", auth)
                    .json(&body)
            })
            .await?;

        self.check_auth_error(&resp);
        let status = resp.status();
        if status.as_u16() == 429 {
            let reset = self.rate_limit_reset(&resp);
            return Err(format!("Rate limited (429). {reset}Try again later."));
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("X API error ({status}): {body}"));
        }

        let response: RetweetResponse = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse retweet response: {e}"))?;

        Ok(response.data.retweeted)
    }

    pub async fn unretweet(&self, user_id: &str, tweet_id: &str) -> Result<bool, String> {
        let url = format!("https://api.x.com/2/users/{user_id}/retweets/{tweet_id}");

        let resp = self
            .retry_503(|| {
                let auth = self.oauth_header("DELETE", &url, &BTreeMap::new());
                self.http.delete(&url).header("Authorization", auth)
            })
            .await?;

        self.check_auth_error(&resp);
        let status = resp.status();
        if status.as_u16() == 429 {
            let reset = self.rate_limit_reset(&resp);
            return Err(format!("Rate limited (429). {reset}Try again later."));
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("X API error ({status}): {body}"));
        }

        let response: RetweetResponse = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse unretweet response: {e}"))?;

        Ok(response.data.retweeted)
    }

    // --- Search ---

    pub async fn search_recent_tweets(
        &self,
        query: &str,
        max_results: u32,
        sort_order: Option<&str>,
        pagination_token: Option<&str>,
    ) -> Result<SearchResult, String> {
        let base_url = "https://api.x.com/2/tweets/search/recent";

        let mut params = BTreeMap::new();
        params.insert("query".to_string(), query.to_string());
        params.insert("max_results".to_string(), max_results.to_string());
        params.insert(
            "tweet.fields".to_string(),
            "id,text,author_id,created_at,public_metrics".to_string(),
        );
        params.insert("expansions".to_string(), "author_id".to_string());
        params.insert("user.fields".to_string(), "username".to_string());
        if let Some(order) = sort_order {
            params.insert("sort_order".to_string(), order.to_string());
        }
        if let Some(token) = pagination_token {
            params.insert("pagination_token".to_string(), token.to_string());
        }

        let query_string: String = params
            .iter()
            .map(|(k, v)| format!("{}={}", pct_encode(k), pct_encode(v)))
            .collect::<Vec<_>>()
            .join("&");
        let full_url = format!("{base_url}?{query_string}");

        let resp = self
            .retry_503(|| {
                let auth = self.oauth_header("GET", base_url, &params);
                self.http.get(&full_url).header("Authorization", auth)
            })
            .await?;

        self.check_auth_error(&resp);
        let status = resp.status();
        if status.as_u16() == 429 {
            let reset = self.rate_limit_reset(&resp);
            return Err(format!("Rate limited (429). {reset}Try again later."));
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("X API error ({status}): {body}"));
        }

        let response: SearchResponse = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse search response: {e}"))?;

        // Build author_id -> username lookup from includes
        let user_map: std::collections::HashMap<String, String> = response
            .includes
            .and_then(|inc| inc.users)
            .unwrap_or_default()
            .into_iter()
            .map(|u| (u.id, u.username))
            .collect();

        let tweets = response
            .data
            .unwrap_or_default()
            .into_iter()
            .map(|t| {
                let username = t
                    .author_id
                    .as_ref()
                    .and_then(|aid| user_map.get(aid))
                    .cloned();
                let (like_count, retweet_count, reply_count) =
                    t.public_metrics.map_or((0, 0, 0), |m| {
                        (m.like_count, m.retweet_count, m.reply_count)
                    });
                SearchTweetResult {
                    id: t.id,
                    text: t.text,
                    username,
                    created_at: t.created_at,
                    like_count,
                    retweet_count,
                    reply_count,
                }
            })
            .collect();

        Ok(SearchResult {
            tweets,
            next_token: response.meta.and_then(|m| m.next_token),
        })
    }

    // --- Simple upload (images ≤5MB) ---

    async fn simple_upload(&self, file_path: &Path, mime: &str) -> Result<String, String> {
        let file_bytes =
            std::fs::read(file_path).map_err(|e| format!("Failed to read file: {e}"))?;
        let file_name = file_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let mime_owned = mime.to_string();

        let resp = self
            .retry_503(|| {
                let part = reqwest::multipart::Part::bytes(file_bytes.clone())
                    .file_name(file_name.clone())
                    .mime_str(&mime_owned)
                    .expect("validated MIME type");
                let form = reqwest::multipart::Form::new().part("media", part);
                let auth = self.oauth_header("POST", MEDIA_UPLOAD_URL, &BTreeMap::new());
                self.http
                    .post(MEDIA_UPLOAD_URL)
                    .header("Authorization", auth)
                    .multipart(form)
            })
            .await?;

        self.check_auth_error(&resp);
        let status = resp.status();
        if status.as_u16() == 429 {
            let reset = self.rate_limit_reset(&resp);
            return Err(format!("Media upload rate limited (429). {reset}"));
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("Media upload error ({status}): {body}"));
        }

        let media: SimpleMediaResponse = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse media response: {e}"))?;
        Ok(media.media_id_string)
    }

    // --- Chunked upload (GIFs, videos, large images) ---

    async fn chunked_upload_init(
        &self,
        total_bytes: u64,
        mime: &str,
        media_category: &str,
    ) -> Result<String, String> {
        let mut params = BTreeMap::new();
        params.insert("command".into(), "INIT".into());
        params.insert("total_bytes".into(), total_bytes.to_string());
        params.insert("media_type".into(), mime.to_string());
        params.insert("media_category".into(), media_category.to_string());

        let resp = self
            .retry_503(|| {
                // Form-encoded params are included in OAuth signature
                let auth = self.oauth_header("POST", MEDIA_UPLOAD_URL, &params);
                self.http
                    .post(MEDIA_UPLOAD_URL)
                    .header("Authorization", auth)
                    .form(&params)
            })
            .await?;

        self.check_auth_error(&resp);
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("Media INIT error ({status}): {body}"));
        }

        let media: ChunkedMediaResponse = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse INIT response: {e}"))?;
        Ok(media.media_id_string)
    }

    async fn chunked_upload_append(
        &self,
        media_id: &str,
        file_path: &Path,
        total_bytes: u64,
    ) -> Result<(), String> {
        let mut file =
            std::fs::File::open(file_path).map_err(|e| format!("Failed to open file: {e}"))?;

        let mut segment_index = 0u32;
        let mut bytes_sent = 0u64;

        loop {
            let mut chunk = Vec::with_capacity(CHUNK_SIZE);
            let bytes_read = file
                .by_ref()
                .take(CHUNK_SIZE as u64)
                .read_to_end(&mut chunk)
                .map_err(|e| format!("Failed to read file chunk: {e}"))?;

            if bytes_read == 0 {
                break;
            }

            // Retry this chunk up to 2 extra times on failure
            let mut last_err = String::new();
            let mut success = false;
            for attempt in 0..3 {
                if attempt > 0 {
                    tracing::warn!(
                        "Chunk {segment_index} failed, retrying ({attempt}/2): {last_err}"
                    );
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                match self
                    .send_append_chunk(media_id, segment_index, &chunk)
                    .await
                {
                    Ok(()) => {
                        success = true;
                        break;
                    }
                    Err(e) => last_err = e,
                }
            }
            if !success {
                return Err(format!(
                    "Chunk {segment_index} failed after 3 attempts: {last_err}"
                ));
            }

            bytes_sent += bytes_read as u64;
            segment_index += 1;
            tracing::info!("Uploaded {bytes_sent}/{total_bytes} bytes ({segment_index} chunks)");
        }

        Ok(())
    }

    async fn send_append_chunk(
        &self,
        media_id: &str,
        segment_index: u32,
        chunk: &[u8],
    ) -> Result<(), String> {
        let media_id_owned = media_id.to_string();
        let segment_str = segment_index.to_string();
        let chunk_owned = chunk.to_vec();

        let resp = self
            .retry_503(|| {
                let part = reqwest::multipart::Part::bytes(chunk_owned.clone())
                    .file_name("media")
                    .mime_str("application/octet-stream")
                    .expect("valid MIME");
                let form = reqwest::multipart::Form::new()
                    .text("command", "APPEND")
                    .text("media_id", media_id_owned.clone())
                    .text("segment_index", segment_str.clone())
                    .part("media", part);
                // Multipart params excluded from OAuth signature per RFC 5849
                let auth = self.oauth_header("POST", MEDIA_UPLOAD_URL, &BTreeMap::new());
                self.http
                    .post(MEDIA_UPLOAD_URL)
                    .header("Authorization", auth)
                    .multipart(form)
            })
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("APPEND error ({status}): {body}"));
        }
        Ok(())
    }

    async fn chunked_upload_finalize(
        &self,
        media_id: &str,
    ) -> Result<ChunkedMediaResponse, String> {
        let mut params = BTreeMap::new();
        params.insert("command".into(), "FINALIZE".into());
        params.insert("media_id".into(), media_id.to_string());

        let resp = self
            .retry_503(|| {
                let auth = self.oauth_header("POST", MEDIA_UPLOAD_URL, &params);
                self.http
                    .post(MEDIA_UPLOAD_URL)
                    .header("Authorization", auth)
                    .form(&params)
            })
            .await?;

        self.check_auth_error(&resp);
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("Media FINALIZE error ({status}): {body}"));
        }

        resp.json::<ChunkedMediaResponse>()
            .await
            .map_err(|e| format!("Failed to parse FINALIZE response: {e}"))
    }

    async fn poll_processing_status(&self, media_id: &str) -> Result<String, String> {
        let start = Instant::now();

        loop {
            if start.elapsed().as_secs() > MAX_PROCESSING_WAIT_SECS {
                return Ok("timed_out".into());
            }

            let mut params = BTreeMap::new();
            params.insert("command".into(), "STATUS".into());
            params.insert("media_id".into(), media_id.to_string());

            let url = format!(
                "{}?command=STATUS&media_id={}",
                MEDIA_UPLOAD_URL, media_id
            );

            let resp = self
                .retry_503(|| {
                    // GET query params are included in OAuth signature
                    let auth = self.oauth_header("GET", MEDIA_UPLOAD_URL, &params);
                    self.http.get(&url).header("Authorization", auth)
                })
                .await?;

            let status = resp.status();
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(format!("Media STATUS error ({status}): {body}"));
            }

            let response: ChunkedMediaResponse = resp
                .json()
                .await
                .map_err(|e| format!("Failed to parse STATUS response: {e}"))?;

            match response.processing_info {
                Some(info) => match info.state.as_str() {
                    "succeeded" => return Ok("succeeded".into()),
                    "failed" => {
                        let msg = info
                            .error
                            .map(|e| e.message)
                            .unwrap_or_else(|| "Unknown processing error".into());
                        return Err(format!("Media processing failed: {msg}"));
                    }
                    "pending" | "in_progress" => {
                        let wait = info.check_after_secs.unwrap_or(5);
                        if let Some(pct) = info.progress_percent {
                            tracing::info!(
                                "Processing media: {pct}% complete, checking in {wait}s"
                            );
                        } else {
                            tracing::info!(
                                "Processing media: state={}, checking in {wait}s",
                                info.state
                            );
                        }
                        tokio::time::sleep(Duration::from_secs(wait)).await;
                    }
                    other => {
                        return Err(format!("Unknown processing state: {other}"));
                    }
                },
                None => {
                    // No processing_info means processing is complete
                    return Ok("succeeded".into());
                }
            }
        }
    }

    // --- Alt text / metadata ---

    async fn set_media_alt_text(&self, media_id: &str, alt_text: &str) -> Result<(), String> {
        let body = serde_json::json!({
            "media_id": media_id,
            "alt_text": {
                "text": alt_text
            }
        });

        let resp = self
            .retry_503(|| {
                let auth = self.oauth_header("POST", MEDIA_METADATA_URL, &BTreeMap::new());
                self.http
                    .post(MEDIA_METADATA_URL)
                    .header("Authorization", auth)
                    .json(&body)
            })
            .await?;

        self.check_auth_error(&resp);
        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(format!("Alt text error ({status}): {body_text}"));
        }

        Ok(())
    }

    // --- Helpers ---

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

    // --- Retry logic ---

    async fn retry_503(
        &self,
        build: impl Fn() -> reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, String> {
        for attempt in 0..=MAX_RETRIES {
            if attempt > 0 {
                let delay = RETRY_BASE_DELAY_MS * 2u64.pow(attempt - 1);
                tracing::warn!(
                    "X API returned 503, retrying in {delay}ms (attempt {attempt}/{MAX_RETRIES})"
                );
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
            let resp = build()
                .send()
                .await
                .map_err(|e| format!("HTTP request failed: {e}"))?;
            if resp.status().as_u16() != 503 || attempt == MAX_RETRIES {
                return Ok(resp);
            }
        }
        unreachable!()
    }

    // --- OAuth 1.0a ---

    fn oauth_header(
        &self,
        method: &str,
        url: &str,
        extra_params: &BTreeMap<String, String>,
    ) -> String {
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

        let mut mac =
            HmacSha1::new_from_slice(signing_key.as_bytes()).expect("HMAC accepts any key length");
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

    fn signature_base_string(
        method: &str,
        url: &str,
        params: &BTreeMap<String, String>,
    ) -> String {
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

// --- Free functions ---

fn pct_encode(input: &str) -> String {
    utf8_percent_encode(input, RFC3986).to_string()
}

fn media_info_from_path(path: &Path) -> Result<MediaInfo, String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "jpg" | "jpeg" => Ok(MediaInfo {
            mime: "image/jpeg".into(),
            media_type: MediaType::Image,
            max_size: 5 * 1024 * 1024,
        }),
        "png" => Ok(MediaInfo {
            mime: "image/png".into(),
            media_type: MediaType::Image,
            max_size: 5 * 1024 * 1024,
        }),
        "webp" => Ok(MediaInfo {
            mime: "image/webp".into(),
            media_type: MediaType::Image,
            max_size: 5 * 1024 * 1024,
        }),
        "gif" => Ok(MediaInfo {
            mime: "image/gif".into(),
            media_type: MediaType::AnimatedGif,
            max_size: 15 * 1024 * 1024,
        }),
        "mp4" => Ok(MediaInfo {
            mime: "video/mp4".into(),
            media_type: MediaType::Video,
            max_size: 512 * 1024 * 1024,
        }),
        _ => Err(format!(
            "Unsupported media format '.{ext}'. Supported: jpeg, png, gif, webp, mp4"
        )),
    }
}

fn validate_media_combination(infos: &[MediaInfo]) -> Result<(), String> {
    if infos.len() <= 1 {
        return Ok(());
    }
    if infos.len() > 4 {
        return Err("Maximum 4 media attachments per tweet".into());
    }

    let has_video = infos.iter().any(|m| m.media_type == MediaType::Video);
    let has_gif = infos.iter().any(|m| m.media_type == MediaType::AnimatedGif);

    if has_video {
        return Err(
            "Videos cannot be mixed with other media. Attach only one video per tweet.".into(),
        );
    }
    if has_gif {
        return Err(
            "Animated GIFs cannot be mixed with other media. Attach only one GIF per tweet.".into(),
        );
    }

    Ok(())
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
