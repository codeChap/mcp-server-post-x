use crate::api::{Config, MeData, MediaAttachment, PostResult, UserProfile, UserSummary, XClient};
use crate::params::{
    FollowsLookupParams, LookupUserParams, PostThreadParams, PostTweetParams, UploadMediaParams,
};
use rmcp::{
    ErrorData as McpError, ServerHandler, handler::server::tool::ToolRouter,
    handler::server::wrapper::Parameters, model::*, tool, tool_handler, tool_router,
};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct PostXServer {
    client: Arc<XClient>,
    cached_me: Arc<Mutex<Option<MeData>>>,
    tool_router: ToolRouter<Self>,
}

impl PostXServer {
    async fn ensure_me(&self) -> Result<MeData, String> {
        {
            let cached = self.cached_me.lock().await;
            if let Some(ref me) = *cached {
                return Ok(me.clone());
            }
        }

        let me = self.client.get_me().await?;
        {
            let mut cached = self.cached_me.lock().await;
            *cached = Some(me.clone());
        }
        Ok(me)
    }

    fn format_post_result(result: &PostResult) -> String {
        format!("Tweet posted!\nID: {}\nURL: {}", result.tweet_id, result.url)
    }

    fn format_follows(users: &[UserSummary], next_token: &Option<String>, label: &str) -> String {
        if users.is_empty() {
            return format!("No {label} found.");
        }

        let mut output = format!("Showing {} {}:\n", users.len(), label);
        for (i, user) in users.iter().enumerate() {
            let followers_str = user
                .public_metrics
                .as_ref()
                .map(|m| format!(" - {} followers", m.followers_count))
                .unwrap_or_default();
            output.push_str(&format!(
                "  {}. @{} ({}){}\n",
                i + 1,
                user.username,
                user.name,
                followers_str,
            ));
            if let Some(ref desc) = user.description {
                if !desc.is_empty() {
                    let truncated = if desc.len() > 100 {
                        format!("{}...", &desc[..97])
                    } else {
                        desc.clone()
                    };
                    output.push_str(&format!("     {truncated}\n"));
                }
            }
        }

        if let Some(token) = next_token {
            output.push_str(&format!("\nMore results available. Next page token: {token}"));
        }

        output
    }

    fn format_user_profile(p: &UserProfile) -> String {
        let mut output = format!("@{} ({})\n", p.username, p.name);
        output.push_str(&format!("  ID: {}\n", p.id));

        if let Some(desc) = &p.description {
            if !desc.is_empty() {
                output.push_str(&format!("  Bio: {desc}\n"));
            }
        }
        if let Some(loc) = &p.location {
            if !loc.is_empty() {
                output.push_str(&format!("  Location: {loc}\n"));
            }
        }
        if let Some(url) = &p.url {
            if !url.is_empty() {
                output.push_str(&format!("  URL: {url}\n"));
            }
        }
        if let Some(metrics) = &p.public_metrics {
            output.push_str(&format!(
                "  Followers: {} | Following: {} | Tweets: {}\n",
                metrics.followers_count, metrics.following_count, metrics.tweet_count
            ));
        }
        if let Some(verified_type) = &p.verified_type {
            output.push_str(&format!("  Verified: {verified_type}\n"));
        } else if p.verified == Some(true) {
            output.push_str("  Verified: yes\n");
        }
        if p.protected == Some(true) {
            output.push_str("  Protected: yes\n");
        }
        if let Some(created) = &p.created_at {
            // Truncate to date only (2013-12-14T04:35:55Z -> 2013-12-14)
            let date = created.split('T').next().unwrap_or(created);
            output.push_str(&format!("  Joined: {date}\n"));
        }
        if let Some(img) = &p.profile_image_url {
            output.push_str(&format!("  Avatar: {img}\n"));
        }

        output
    }
}

#[tool_router]
impl PostXServer {
    pub fn new(config: Config) -> Self {
        Self {
            client: Arc::new(XClient::new(config)),
            cached_me: Arc::new(Mutex::new(None)),
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Post a single tweet to X (Twitter). Supports text with optional media: up to 4 images, or 1 video, or 1 GIF. Use 'media' to upload+attach files, or 'media_ids' for pre-uploaded media (not both)."
    )]
    async fn post_tweet(
        &self,
        Parameters(params): Parameters<PostTweetParams>,
    ) -> Result<CallToolResult, McpError> {
        // Validate mutual exclusivity
        let has_media = params.media.as_ref().is_some_and(|m| !m.is_empty());
        let has_media_ids = params.media_ids.as_ref().is_some_and(|ids| !ids.is_empty());
        if has_media && has_media_ids {
            return Ok(CallToolResult::error(vec![Content::text(
                "'media' and 'media_ids' are mutually exclusive. Use one or the other.",
            )]));
        }

        let me = match self.ensure_me().await {
            Ok(me) => me,
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(e)])),
        };

        let media_attachments: Vec<MediaAttachment> = params
            .media
            .unwrap_or_default()
            .into_iter()
            .map(|m| MediaAttachment {
                path: m.path,
                alt_text: m.alt_text,
            })
            .collect();

        let media_ids = params.media_ids;

        match self
            .client
            .post_tweet(
                &params.text,
                &media_attachments,
                media_ids.as_deref(),
                None,
                &me.username,
            )
            .await
        {
            Ok(result) => Ok(CallToolResult::success(vec![Content::text(
                Self::format_post_result(&result),
            )])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
        }
    }

    #[tool(
        description = "Post a thread of tweets to X (Twitter). Each tweet can optionally include media attachments. Max 25 tweets per thread."
    )]
    async fn post_thread(
        &self,
        Parameters(params): Parameters<PostThreadParams>,
    ) -> Result<CallToolResult, McpError> {
        if params.tweets.is_empty() {
            return Err(McpError::invalid_params(
                "Thread must contain at least one tweet",
                None,
            ));
        }
        if params.tweets.len() > 25 {
            return Err(McpError::invalid_params(
                "Thread cannot exceed 25 tweets",
                None,
            ));
        }

        let me = match self.ensure_me().await {
            Ok(me) => me,
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(e)])),
        };

        let tweets: Vec<(String, Vec<MediaAttachment>)> = params
            .tweets
            .into_iter()
            .map(|t| {
                let media = t
                    .media
                    .unwrap_or_default()
                    .into_iter()
                    .map(|m| MediaAttachment {
                        path: m.path,
                        alt_text: m.alt_text,
                    })
                    .collect();
                (t.text, media)
            })
            .collect();

        let tweet_count = tweets.len();
        let result = self.client.post_thread(&tweets, &me.username).await;

        let mut output = String::new();
        if !result.posted.is_empty() {
            output.push_str(&format!(
                "Posted {}/{} tweets:\n",
                result.posted.len(),
                tweet_count
            ));
            for (i, post) in result.posted.iter().enumerate() {
                output.push_str(&format!(
                    "  {}. ID: {} — {}\n",
                    i + 1,
                    post.tweet_id,
                    post.url
                ));
            }
        }

        if let Some(err) = &result.error {
            output.push_str(&format!("\nError: {err}"));
            return Ok(CallToolResult::error(vec![Content::text(output)]));
        }

        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    #[tool(
        description = "Upload media to X (Twitter) for later attachment via media_ids. Returns a media_id. Supports: jpeg/png/webp (max 5MB), gif (max 15MB), mp4 video (max 512MB). Alt text supported for images and GIFs only."
    )]
    async fn upload_media(
        &self,
        Parameters(params): Parameters<UploadMediaParams>,
    ) -> Result<CallToolResult, McpError> {
        match self
            .client
            .upload_media(&params.path, params.alt_text.as_deref())
            .await
        {
            Ok(result) => {
                let text = format!(
                    "Media uploaded!\nMedia ID: {}\nType: {}\nState: {}",
                    result.media_id, result.media_type, result.state
                );
                Ok(CallToolResult::success(vec![Content::text(text)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
        }
    }

    #[tool(
        description = "Get the authenticated X (Twitter) user's profile (id, name, username). Useful for verifying credentials."
    )]
    async fn get_me(&self) -> Result<CallToolResult, McpError> {
        match self.client.get_me().await {
            Ok(me) => {
                {
                    let mut cached = self.cached_me.lock().await;
                    *cached = Some(me.clone());
                }
                let text = format!(
                    "Authenticated as:\n  Name: {}\n  Username: @{}\n  ID: {}",
                    me.name, me.username, me.id
                );
                Ok(CallToolResult::success(vec![Content::text(text)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
        }
    }

    #[tool(
        description = "Get the authenticated user's followers on X (Twitter). Returns usernames, display names, follower counts, and bios."
    )]
    async fn get_followers(
        &self,
        Parameters(params): Parameters<FollowsLookupParams>,
    ) -> Result<CallToolResult, McpError> {
        let me = match self.ensure_me().await {
            Ok(me) => me,
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(e)])),
        };

        let max_results = params.max_results.unwrap_or(20).clamp(1, 100);

        match self
            .client
            .get_followers(&me.id, max_results, params.pagination_token.as_deref())
            .await
        {
            Ok(result) => {
                let text = Self::format_follows(&result.users, &result.next_token, "followers");
                Ok(CallToolResult::success(vec![Content::text(text)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
        }
    }

    #[tool(
        description = "Get who the authenticated user follows on X (Twitter). Returns usernames, display names, follower counts, and bios."
    )]
    async fn get_following(
        &self,
        Parameters(params): Parameters<FollowsLookupParams>,
    ) -> Result<CallToolResult, McpError> {
        let me = match self.ensure_me().await {
            Ok(me) => me,
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(e)])),
        };

        let max_results = params.max_results.unwrap_or(20).clamp(1, 100);

        match self
            .client
            .get_following(&me.id, max_results, params.pagination_token.as_deref())
            .await
        {
            Ok(result) => {
                let text = Self::format_follows(&result.users, &result.next_token, "following");
                Ok(CallToolResult::success(vec![Content::text(text)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
        }
    }

    #[tool(
        description = "Look up an X (Twitter) user's profile by username or numeric user ID. Returns bio, location, follower/following counts, verified status, and more."
    )]
    async fn lookup_user(
        &self,
        Parameters(params): Parameters<LookupUserParams>,
    ) -> Result<CallToolResult, McpError> {
        let user = params.user.trim().strip_prefix('@').unwrap_or(params.user.trim());

        if user.is_empty() {
            return Ok(CallToolResult::error(vec![Content::text(
                "User parameter cannot be empty.",
            )]));
        }

        let is_id = user.chars().all(|c| c.is_ascii_digit());

        let result = if is_id {
            self.client.lookup_user_by_id(user).await
        } else {
            self.client.lookup_user_by_username(user).await
        };

        match result {
            Ok(profile) => {
                let text = Self::format_user_profile(&profile);
                Ok(CallToolResult::success(vec![Content::text(text)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
        }
    }
}

#[tool_handler]
impl ServerHandler for PostXServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::default(),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation {
                name: "mcp-server-post-x".to_string(),
                title: None,
                version: env!("CARGO_PKG_VERSION").to_string(),
                icons: None,
                website_url: None,
            },
            instructions: Some(
                "X (Twitter) server. Use post_tweet to post tweets with optional media, \
                 upload_media to pre-upload media, post_thread to post threads, \
                 get_me to verify credentials, get_followers/get_following to list follows, \
                 lookup_user to view any user's profile."
                    .to_string(),
            ),
        }
    }
}
