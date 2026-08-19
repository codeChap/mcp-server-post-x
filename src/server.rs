macro_rules! try_tool {
    ($expr:expr) => {
        match $expr {
            Ok(v) => v,
            Err(e) => return Ok(e),
        }
    };
}

use crate::api::{
    AppConfig, DmEventResult, MeData, MediaAttachment, PostResult, SearchTweetResult, Trend,
    UserProfile, UserSummary, XClient,
};
use crate::params::{
    AccountOnlyParams, FollowsLookupParams, GetAllFollowsParams, GetBookmarksParams,
    GetDmEventsParams, GetTrendsParams, LookupUserParams, PostThreadParams, PostTweetParams,
    SearchTweetsParams, SendDmParams, TimelineParams, TweetIdParams, UpdateProfileBannerParams,
    UpdateProfileParams, UploadMediaParams,
};
use reqwest::Client;
use rmcp::{
    ErrorData as McpError, ServerHandler, handler::server::tool::ToolRouter,
    handler::server::wrapper::Parameters, model::*, tool, tool_handler, tool_router,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct XServer {
    clients: HashMap<String, Arc<XClient>>,
    default_account: String,
    cached_me: Arc<Mutex<HashMap<String, MeData>>>,
    instructions: String,
    tool_router: ToolRouter<Self>,
}

impl XServer {
    fn resolve_account<'a>(
        &'a self,
        account: Option<&'a str>,
    ) -> Result<(&'a str, &'a Arc<XClient>), String> {
        let name = match account {
            Some(a) if !a.trim().is_empty() => a.trim(),
            _ => &self.default_account,
        };
        let client = self.clients.get(name).ok_or_else(|| {
            let available: Vec<&str> = self.clients.keys().map(|s| s.as_str()).collect();
            format!(
                "Unknown account '{name}'. Available: {}",
                available.join(", ")
            )
        })?;
        Ok((name, client))
    }

    fn require_account<'a>(
        &'a self,
        account: Option<&'a str>,
    ) -> Result<(&'a str, &'a Arc<XClient>), CallToolResult> {
        self.resolve_account(account)
            .map_err(|e| CallToolResult::error(vec![Content::text(e)]))
    }

    async fn ensure_me(&self, account: Option<&str>) -> Result<(String, MeData), String> {
        let (name, client) = self.resolve_account(account)?;
        {
            let cached = self.cached_me.lock().await;
            if let Some(me) = cached.get(name) {
                return Ok((name.to_string(), me.clone()));
            }
        }

        let me = client.get_me().await?;
        {
            let mut cached = self.cached_me.lock().await;
            cached.insert(name.to_string(), me.clone());
        }
        Ok((name.to_string(), me))
    }

    #[allow(clippy::type_complexity)]
    fn require_me_for(
        &self,
        account: Option<&str>,
    ) -> impl std::future::Future<Output = Result<(String, Arc<XClient>, MeData), CallToolResult>> + '_
    {
        let account = account.map(|s| s.to_string());
        async move {
            let (name, client) = self
                .resolve_account(account.as_deref())
                .map_err(|e| CallToolResult::error(vec![Content::text(e)]))?;
            let client = client.clone();
            let (name, me) = self
                .ensure_me(Some(name))
                .await
                .map_err(|e| CallToolResult::error(vec![Content::text(e)]))?;
            Ok((name, client, me))
        }
    }

    fn require_tweet_id(raw: &str) -> Result<&str, CallToolResult> {
        let id = Self::extract_tweet_id(raw);
        if id.is_empty() {
            Err(CallToolResult::error(vec![Content::text(
                "Tweet ID cannot be empty.",
            )]))
        } else {
            Ok(id)
        }
    }

    fn ok_or_err(result: Result<String, String>) -> CallToolResult {
        match result {
            Ok(text) => CallToolResult::success(vec![Content::text(text)]),
            Err(e) => CallToolResult::error(vec![Content::text(e)]),
        }
    }

    fn format_post_result(result: &PostResult, account: &str) -> String {
        format!(
            "Tweet posted as @{account}!\nID: {}\nURL: {}",
            result.tweet_id, result.url
        )
    }

    fn truncate_str(s: &str, max_bytes: usize) -> &str {
        if s.len() <= max_bytes {
            return s;
        }
        let mut end = max_bytes;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        &s[..end]
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
            if let Some(desc) = user.description.as_ref().filter(|d| !d.is_empty()) {
                let truncated = if desc.len() > 100 {
                    format!("{}...", Self::truncate_str(desc, 97))
                } else {
                    desc.clone()
                };
                output.push_str(&format!("     {truncated}\n"));
            }
        }

        Self::append_pagination(&mut output, next_token);
        output
    }

    fn format_all_follows(users: &[UserSummary], label: &str) -> String {
        if users.is_empty() {
            return format!("No {label} found.");
        }

        let mut output = format!("Total {}: {}\n", label, users.len());
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
            if let Some(desc) = user.description.as_ref().filter(|d| !d.is_empty()) {
                let truncated = if desc.len() > 100 {
                    format!("{}...", Self::truncate_str(desc, 97))
                } else {
                    desc.clone()
                };
                output.push_str(&format!("     {truncated}\n"));
            }
        }

        output
    }

    fn format_user_profile(p: &UserProfile) -> String {
        let mut output = format!("@{} ({})\n", p.username, p.name);
        output.push_str(&format!("  ID: {}\n", p.id));

        if let Some(desc) = p.description.as_ref().filter(|d| !d.is_empty()) {
            output.push_str(&format!("  Bio: {desc}\n"));
        }
        if let Some(loc) = p.location.as_ref().filter(|l| !l.is_empty()) {
            output.push_str(&format!("  Location: {loc}\n"));
        }
        if let Some(url) = p.url.as_ref().filter(|u| !u.is_empty()) {
            output.push_str(&format!("  URL: {url}\n"));
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
            let date = created.split('T').next().unwrap_or(created);
            output.push_str(&format!("  Joined: {date}\n"));
        }
        if let Some(img) = &p.profile_image_url {
            output.push_str(&format!("  Avatar: {img}\n"));
        }

        output
    }

    /// Client-side length checks (X counts Unicode scalar values) so the caller
    /// gets a friendly error before the request instead of a raw 400/403.
    fn validate_profile_lengths(params: &UpdateProfileParams) -> Option<String> {
        fn check(field: &str, val: &Option<String>, max: usize) -> Option<String> {
            val.as_ref().and_then(|v| {
                let len = v.chars().count();
                (len > max).then(|| format!("{field} is too long: {len} characters (max {max})."))
            })
        }

        if params.name.as_ref().is_some_and(|n| n.trim().is_empty()) {
            return Some("name cannot be empty.".to_string());
        }

        check("name", &params.name, 50)
            .or_else(|| check("description", &params.description, 160))
            .or_else(|| check("location", &params.location, 30))
            .or_else(|| check("url", &params.url, 100))
    }

    fn extract_tweet_id(input: &str) -> &str {
        let trimmed = input.trim();
        if let Some(rest) = trimmed.split("/status/").nth(1) {
            rest.split(['?', '#', '/']).next().unwrap_or(trimmed)
        } else {
            trimmed
        }
    }

    fn format_search_results(
        query: &str,
        tweets: &[SearchTweetResult],
        next_token: &Option<String>,
    ) -> String {
        if tweets.is_empty() {
            return format!("No results found for \"{query}\".");
        }

        let mut output = format!("Search results for \"{}\" ({} results):\n", query, tweets.len());
        for (i, t) in tweets.iter().enumerate() {
            let author = t
                .username
                .as_deref()
                .map(|u| format!("@{u}"))
                .unwrap_or_else(|| "unknown".to_string());
            let date = t
                .created_at
                .as_deref()
                .and_then(|d| d.split('T').next())
                .unwrap_or("");
            output.push_str(&format!("{}. {} · {}\n", i + 1, author, date));
            output.push_str(&format!("   {}\n", t.text.replace('\n', "\n   ")));
            output.push_str(&format!(
                "   RT:{} Like:{} Reply:{} id:{}\n",
                t.retweet_count, t.like_count, t.reply_count, t.id
            ));
        }

        Self::append_pagination(&mut output, next_token);
        output
    }

    fn format_dm_events(events: &[DmEventResult], next_token: &Option<String>) -> String {
        if events.is_empty() {
            return "No DM events found.".to_string();
        }

        let mut output = format!("DM events ({} messages):\n", events.len());
        for (i, e) in events.iter().enumerate() {
            let sender = e.sender_id.as_deref().unwrap_or("unknown");
            let date = e
                .created_at
                .as_deref()
                .and_then(|d| d.split('T').next())
                .unwrap_or("");
            let conv = e.conversation_id.as_deref().unwrap_or("?");
            let text = e.text.as_deref().unwrap_or("");
            output.push_str(&format!(
                "{}. [{}] sender:{} conv:{}\n   {}\n",
                i + 1,
                date,
                sender,
                conv,
                text
            ));
        }

        Self::append_pagination(&mut output, next_token);
        output
    }

    fn format_trends(trends: &[Trend], woeid: u64) -> String {
        if trends.is_empty() {
            return format!("No trends found for WOEID {woeid}.");
        }

        let location = match woeid {
            1 => "Worldwide".to_string(),
            23424977 => "United States".to_string(),
            23424975 => "United Kingdom".to_string(),
            23424856 => "Japan".to_string(),
            2459115 => "New York".to_string(),
            44418 => "London".to_string(),
            1118370 => "Tokyo".to_string(),
            _ => format!("WOEID {}", woeid),
        };

        let mut output = format!("Trending in {} (WOEID {}):\n", location, woeid);
        for (i, t) in trends.iter().enumerate() {
            let vol = t
                .tweet_count
                .map(|v| format!(" ({v} posts)"))
                .unwrap_or_default();
            output.push_str(&format!("  {}. {}{}\n", i + 1, t.name, vol));
        }
        output
    }

    fn append_pagination(output: &mut String, next_token: &Option<String>) {
        if let Some(token) = next_token {
            output.push_str(&format!("\nMore results available. Next page token: {token}"));
        }
    }
}

#[tool_router]
impl XServer {
    pub fn new(config: AppConfig) -> Self {
        let http = Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .expect("failed to build HTTP client");

        let default_account = config.default_account.clone();

        let account_names: Vec<String> = config.accounts.keys().cloned().collect();

        let clients: HashMap<String, Arc<XClient>> = config
            .accounts
            .into_iter()
            .map(|(name, acct)| (name, Arc::new(XClient::new(acct, http.clone()))))
            .collect();

        let instructions = {
            let accounts_str: Vec<String> = account_names
                .iter()
                .map(|name| {
                    if name == &default_account {
                        format!("{name} (default)")
                    } else {
                        name.clone()
                    }
                })
                .collect();
            format!(
                "X (Twitter) server with multi-account support. \
                 All tools accept an optional 'account' parameter to select \
                 which X account to use (omit for default). \
                 Available accounts: {}. \
                 Tools: post_tweet, post_thread, upload_media, \
                 delete_tweet, search_tweets, get_timeline, get_bookmarks, get_me, lookup_user, \
                 get_followers, get_following, get_all_followers, get_all_following, \
                 follow_user, unfollow_user, like_tweet, unlike_tweet, retweet, \
                 unretweet, bookmark_tweet, unbookmark_tweet, get_trends, update_profile, update_profile_banner, get_dm_events, send_dm, list_accounts.",
                accounts_str.join(", ")
            )
        };

        Self {
            clients,
            default_account,
            cached_me: Arc::new(Mutex::new(HashMap::new())),
            instructions,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(description = "List available X (Twitter) accounts and which is the default.")]
    async fn list_accounts(
        &self,
        Parameters(_params): Parameters<AccountOnlyParams>,
    ) -> Result<CallToolResult, McpError> {
        let cached = self.cached_me.lock().await;
        let mut output = format!("Available accounts ({}):\n", self.clients.len());
        for name in self.clients.keys() {
            let default_marker = if name == &self.default_account {
                " (default)"
            } else {
                ""
            };
            let username = cached
                .get(name)
                .map(|me| format!(" — @{}", me.username))
                .unwrap_or_default();
            output.push_str(&format!("  - {name}{default_marker}{username}\n"));
        }
        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    #[tool(
        description = "Post a single tweet to X (Twitter). Supports text with optional media: up to 4 images, or 1 video, or 1 GIF. Use 'media' to upload+attach files, or 'media_ids' for pre-uploaded media (not both)."
    )]
    async fn post_tweet(
        &self,
        Parameters(params): Parameters<PostTweetParams>,
    ) -> Result<CallToolResult, McpError> {
        let has_media = params.media.as_ref().is_some_and(|m| !m.is_empty());
        let has_media_ids = params.media_ids.as_ref().is_some_and(|ids| !ids.is_empty());
        if has_media && has_media_ids {
            return Ok(CallToolResult::error(vec![Content::text(
                "'media' and 'media_ids' are mutually exclusive. Use one or the other.",
            )]));
        }

        let (account, client, me) =
            try_tool!(self.require_me_for(params.account.as_deref()).await);

        let media_attachments: Vec<MediaAttachment> = params
            .media
            .unwrap_or_default()
            .into_iter()
            .map(Into::into)
            .collect();

        let result = client
            .post_tweet(
                &params.text,
                &media_attachments,
                params.media_ids.as_deref(),
                params.reply_to.as_deref(),
                &me.username,
            )
            .await;

        Ok(Self::ok_or_err(
            result.map(|r| Self::format_post_result(&r, &account)),
        ))
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

        let (account, client, me) =
            try_tool!(self.require_me_for(params.account.as_deref()).await);

        let tweets: Vec<(String, Vec<MediaAttachment>)> = params
            .tweets
            .into_iter()
            .map(|t| {
                let media = t
                    .media
                    .unwrap_or_default()
                    .into_iter()
                    .map(Into::into)
                    .collect();
                (t.text, media)
            })
            .collect();

        let tweet_count = tweets.len();
        let result = client.post_thread(&tweets, &me.username).await;

        let mut output = String::new();
        if !result.posted.is_empty() {
            output.push_str(&format!(
                "Posted {}/{} tweets as @{account}:\n",
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
        let (account, client) = try_tool!(self.require_account(params.account.as_deref()));

        let result = client
            .upload_media(&params.path, params.alt_text.as_deref())
            .await;

        Ok(Self::ok_or_err(result.map(|r| {
            format!(
                "Media uploaded (account: {account})!\nMedia ID: {}\nType: {}\nState: {}",
                r.media_id, r.media_type, r.state
            )
        })))
    }

    #[tool(
        description = "Update the X (Twitter) profile banner (header image) for the authenticated user. This uses the legacy v1.1 account endpoint. Provide a local static image (JPEG/PNG/WebP, max 5MB; X recommends 1500x500). The banner param is sent base64-encoded (not via media_id). Optional crop parameters supported."
    )]
    async fn update_profile_banner(
        &self,
        Parameters(params): Parameters<UpdateProfileBannerParams>,
    ) -> Result<CallToolResult, McpError> {
        let (account, client) = try_tool!(self.require_account(params.account.as_deref()));

        let result = client
            .update_profile_banner(
                &params.path,
                params.width,
                params.height,
                params.offset_left,
                params.offset_top,
            )
            .await;

        Ok(Self::ok_or_err(result.map(|_| {
            format!(
                "Profile banner updated successfully (account: {account})!\nImage: {}",
                params.path
            )
        })))
    }

    #[tool(
        description = "Update the authenticated X (Twitter) user's profile text: bio/description, display name, location, and/or website URL. At least one field must be provided; only the fields you pass are changed, and passing an empty string clears that field. Uses the legacy v1.1 account/update_profile endpoint (requires Read+Write app permission). Limits: name <=50, description <=160, location <=30, url <=100 characters."
    )]
    async fn update_profile(
        &self,
        Parameters(params): Parameters<UpdateProfileParams>,
    ) -> Result<CallToolResult, McpError> {
        if params.description.is_none()
            && params.name.is_none()
            && params.location.is_none()
            && params.url.is_none()
        {
            return Ok(CallToolResult::error(vec![Content::text(
                "Provide at least one field to update (description, name, location, or url).",
            )]));
        }
        if let Some(err) = Self::validate_profile_lengths(&params) {
            return Ok(CallToolResult::error(vec![Content::text(err)]));
        }

        let (account, client) = try_tool!(self.require_account(params.account.as_deref()));
        let account = account.to_string();
        let client = client.clone();

        let result = client
            .update_profile(
                params.name.as_deref(),
                params.description.as_deref(),
                params.location.as_deref(),
                params.url.as_deref(),
            )
            .await;

        // Display name may have changed — drop cached identity so it refetches.
        if result.is_ok() {
            self.cached_me.lock().await.remove(&account);
        }

        Ok(Self::ok_or_err(result.map(|p| {
            let mut out = format!("Profile updated successfully (account: {account})!");
            if let Some(d) = &p.description {
                out.push_str(&format!("\n  Bio: {d}"));
            }
            if let Some(n) = &p.name {
                out.push_str(&format!("\n  Name: {n}"));
            }
            if let Some(l) = p.location.as_ref().filter(|l| !l.is_empty()) {
                out.push_str(&format!("\n  Location: {l}"));
            }
            out
        })))
    }

    #[tool(
        description = "Get the authenticated X (Twitter) user's profile (id, name, username). Useful for verifying credentials."
    )]
    async fn get_me(
        &self,
        Parameters(params): Parameters<AccountOnlyParams>,
    ) -> Result<CallToolResult, McpError> {
        let (name, client) = try_tool!(self.require_account(params.account.as_deref()));

        match client.get_me().await {
            Ok(me) => {
                {
                    let mut cached = self.cached_me.lock().await;
                    cached.insert(name.to_string(), me.clone());
                }
                let text = format!(
                    "Authenticated as (account: {name}):\n  Name: {}\n  Username: @{}\n  ID: {}",
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
        let (_account, client, me) =
            try_tool!(self.require_me_for(params.account.as_deref()).await);

        let max_results = params.max_results.unwrap_or(20).clamp(1, 100);

        let result = client
            .get_followers(&me.id, max_results, params.pagination_token.as_deref())
            .await;

        Ok(Self::ok_or_err(
            result.map(|r| Self::format_follows(&r.users, &r.next_token, "followers")),
        ))
    }

    #[tool(
        description = "Get who the authenticated user follows on X (Twitter). Returns usernames, display names, follower counts, and bios."
    )]
    async fn get_following(
        &self,
        Parameters(params): Parameters<FollowsLookupParams>,
    ) -> Result<CallToolResult, McpError> {
        let (_account, client, me) =
            try_tool!(self.require_me_for(params.account.as_deref()).await);

        let max_results = params.max_results.unwrap_or(20).clamp(1, 100);

        let result = client
            .get_following(&me.id, max_results, params.pagination_token.as_deref())
            .await;

        Ok(Self::ok_or_err(
            result.map(|r| Self::format_follows(&r.users, &r.next_token, "following")),
        ))
    }

    #[tool(
        description = "Get ALL accounts the authenticated user follows on X (Twitter). Auto-paginates. Has a safety cap (default 5000, max 10000) to avoid massive responses. Use get_following for paginated access without cap."
    )]
    async fn get_all_following(
        &self,
        Parameters(params): Parameters<GetAllFollowsParams>,
    ) -> Result<CallToolResult, McpError> {
        let (_account, client, me) =
            try_tool!(self.require_me_for(params.account.as_deref()).await);

        let max_users = params.max_users.unwrap_or(5000).clamp(1, 10000);
        let result = client.get_all_following(&me.id, max_users).await;

        Ok(Self::ok_or_err(
            result.map(|users| Self::format_all_follows(&users, "following")),
        ))
    }

    #[tool(
        description = "Get ALL followers of the authenticated user on X (Twitter). Auto-paginates. Has a safety cap (default 5000, max 10000) to avoid massive responses. Use get_followers for paginated access without cap."
    )]
    async fn get_all_followers(
        &self,
        Parameters(params): Parameters<GetAllFollowsParams>,
    ) -> Result<CallToolResult, McpError> {
        let (_account, client, me) =
            try_tool!(self.require_me_for(params.account.as_deref()).await);

        let max_users = params.max_users.unwrap_or(5000).clamp(1, 10000);
        let result = client.get_all_followers(&me.id, max_users).await;

        Ok(Self::ok_or_err(
            result.map(|users| Self::format_all_follows(&users, "followers")),
        ))
    }

    #[tool(
        description = "Follow a user on X (Twitter). Accepts a username (with or without @) or numeric user ID."
    )]
    async fn follow_user(
        &self,
        Parameters(params): Parameters<LookupUserParams>,
    ) -> Result<CallToolResult, McpError> {
        let (_account, client, me) =
            try_tool!(self.require_me_for(params.account.as_deref()).await);

        let target_id = match client.resolve_user_id(&params.user).await {
            Ok(id) => id,
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(e)])),
        };

        Ok(Self::ok_or_err(
            client
                .follow_user(&me.id, &target_id)
                .await
                .map(|following| format!("Now following user {}: {following}", params.user.trim())),
        ))
    }

    #[tool(
        description = "Unfollow a user on X (Twitter). Accepts a username (with or without @) or numeric user ID."
    )]
    async fn unfollow_user(
        &self,
        Parameters(params): Parameters<LookupUserParams>,
    ) -> Result<CallToolResult, McpError> {
        let (_account, client, me) =
            try_tool!(self.require_me_for(params.account.as_deref()).await);

        let target_id = match client.resolve_user_id(&params.user).await {
            Ok(id) => id,
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(e)])),
        };

        Ok(Self::ok_or_err(
            client
                .unfollow_user(&me.id, &target_id)
                .await
                .map(|following| {
                    format!("Unfollowed user {} (following: {following})", params.user.trim())
                }),
        ))
    }

    #[tool(
        description = "Look up an X (Twitter) user's profile by username or numeric user ID. Returns bio, location, follower/following counts, verified status, and more."
    )]
    async fn lookup_user(
        &self,
        Parameters(params): Parameters<LookupUserParams>,
    ) -> Result<CallToolResult, McpError> {
        let (_account, client) = try_tool!(self.require_account(params.account.as_deref()));

        let user = params
            .user
            .trim()
            .strip_prefix('@')
            .unwrap_or(params.user.trim());

        if user.is_empty() {
            return Ok(CallToolResult::error(vec![Content::text(
                "User parameter cannot be empty.",
            )]));
        }

        let is_id = user.chars().all(|c| c.is_ascii_digit());

        let result = if is_id {
            client.lookup_user_by_id(user).await
        } else {
            client.lookup_user_by_username(user).await
        };

        Ok(Self::ok_or_err(result.map(|p| Self::format_user_profile(&p))))
    }

    #[tool(description = "Like a tweet on X (Twitter). Accepts a tweet ID or tweet URL.")]
    async fn like_tweet(
        &self,
        Parameters(params): Parameters<TweetIdParams>,
    ) -> Result<CallToolResult, McpError> {
        let tweet_id = try_tool!(Self::require_tweet_id(&params.tweet_id));

        let (_account, client, me) =
            try_tool!(self.require_me_for(params.account.as_deref()).await);

        Ok(Self::ok_or_err(
            client
                .like_tweet(&me.id, tweet_id)
                .await
                .map(|liked| format!("Tweet {tweet_id} liked: {liked}")),
        ))
    }

    #[tool(description = "Unlike a tweet on X (Twitter). Accepts a tweet ID or tweet URL.")]
    async fn unlike_tweet(
        &self,
        Parameters(params): Parameters<TweetIdParams>,
    ) -> Result<CallToolResult, McpError> {
        let tweet_id = try_tool!(Self::require_tweet_id(&params.tweet_id));

        let (_account, client, me) =
            try_tool!(self.require_me_for(params.account.as_deref()).await);

        Ok(Self::ok_or_err(
            client
                .unlike_tweet(&me.id, tweet_id)
                .await
                .map(|liked| format!("Tweet {tweet_id} unliked (liked: {liked})")),
        ))
    }

    #[tool(
        description = "Delete a tweet on X (Twitter). You can only delete your own tweets. Accepts a tweet ID or tweet URL."
    )]
    async fn delete_tweet(
        &self,
        Parameters(params): Parameters<TweetIdParams>,
    ) -> Result<CallToolResult, McpError> {
        let tweet_id = try_tool!(Self::require_tweet_id(&params.tweet_id));

        let (_account, client) = try_tool!(self.require_account(params.account.as_deref()));

        Ok(Self::ok_or_err(
            client
                .delete_tweet(tweet_id)
                .await
                .map(|deleted| format!("Tweet {tweet_id} deleted: {deleted}")),
        ))
    }

    #[tool(description = "Retweet a tweet on X (Twitter). Accepts a tweet ID or tweet URL.")]
    async fn retweet(
        &self,
        Parameters(params): Parameters<TweetIdParams>,
    ) -> Result<CallToolResult, McpError> {
        let tweet_id = try_tool!(Self::require_tweet_id(&params.tweet_id));

        let (_account, client, me) =
            try_tool!(self.require_me_for(params.account.as_deref()).await);

        Ok(Self::ok_or_err(
            client
                .retweet(&me.id, tweet_id)
                .await
                .map(|retweeted| format!("Tweet {tweet_id} retweeted: {retweeted}")),
        ))
    }

    #[tool(description = "Undo a retweet on X (Twitter). Accepts a tweet ID or tweet URL.")]
    async fn unretweet(
        &self,
        Parameters(params): Parameters<TweetIdParams>,
    ) -> Result<CallToolResult, McpError> {
        let tweet_id = try_tool!(Self::require_tweet_id(&params.tweet_id));

        let (_account, client, me) =
            try_tool!(self.require_me_for(params.account.as_deref()).await);

        Ok(Self::ok_or_err(
            client
                .unretweet(&me.id, tweet_id)
                .await
                .map(|retweeted| format!("Tweet {tweet_id} unretweeted (retweeted: {retweeted})")),
        ))
    }

    #[tool(description = "Bookmark a tweet on X (Twitter). Accepts a tweet ID or tweet URL.")]
    async fn bookmark_tweet(
        &self,
        Parameters(params): Parameters<TweetIdParams>,
    ) -> Result<CallToolResult, McpError> {
        let tweet_id = try_tool!(Self::require_tweet_id(&params.tweet_id));

        let (_account, client, me) =
            try_tool!(self.require_me_for(params.account.as_deref()).await);

        Ok(Self::ok_or_err(
            client
                .bookmark_tweet(&me.id, tweet_id)
                .await
                .map(|bookmarked| format!("Tweet {tweet_id} bookmarked: {bookmarked}")),
        ))
    }

    #[tool(description = "Remove a bookmark on X (Twitter). Accepts a tweet ID or tweet URL.")]
    async fn unbookmark_tweet(
        &self,
        Parameters(params): Parameters<TweetIdParams>,
    ) -> Result<CallToolResult, McpError> {
        let tweet_id = try_tool!(Self::require_tweet_id(&params.tweet_id));

        let (_account, client, me) =
            try_tool!(self.require_me_for(params.account.as_deref()).await);

        Ok(Self::ok_or_err(
            client
                .unbookmark_tweet(&me.id, tweet_id)
                .await
                .map(|bookmarked| format!("Tweet {tweet_id} unbookmarked (bookmarked: {bookmarked})")),
        ))
    }

    #[tool(
        description = "Search recent tweets on X (Twitter) from the last 7 days. Supports operators: from:user, #hashtag, @mention, \"exact phrase\", -exclude, lang:en, etc."
    )]
    async fn search_tweets(
        &self,
        Parameters(params): Parameters<SearchTweetsParams>,
    ) -> Result<CallToolResult, McpError> {
        let query = params.query.trim();
        if query.is_empty() {
            return Ok(CallToolResult::error(vec![Content::text(
                "Search query cannot be empty.",
            )]));
        }

        let (_account, client) = try_tool!(self.require_account(params.account.as_deref()));

        let max_results = params.max_results.unwrap_or(10).clamp(10, 100);

        let result = client
            .search_recent_tweets(
                query,
                max_results,
                params.sort_order.as_deref(),
                params.pagination_token.as_deref(),
            )
            .await;

        Ok(Self::ok_or_err(
            result.map(|r| Self::format_search_results(query, &r.tweets, &r.next_token)),
        ))
    }

    #[tool(
        description = "Get the authenticated user's home timeline on X (Twitter). Returns recent tweets in reverse chronological order. Can exclude replies and/or retweets."
    )]
    async fn get_timeline(
        &self,
        Parameters(params): Parameters<TimelineParams>,
    ) -> Result<CallToolResult, McpError> {
        let (_account, client, me) =
            try_tool!(self.require_me_for(params.account.as_deref()).await);

        let max_results = params.max_results.unwrap_or(20).clamp(1, 100);

        let result = client
            .get_timeline(
                &me.id,
                max_results,
                params.pagination_token.as_deref(),
                params.exclude.as_deref(),
            )
            .await;

        Ok(Self::ok_or_err(
            result.map(|r| Self::format_search_results("timeline", &r.tweets, &r.next_token)),
        ))
    }

    #[tool(
        description = "Get bookmarked tweets for the authenticated user on X (Twitter). Paginated, returns recent bookmarks first."
    )]
    async fn get_bookmarks(
        &self,
        Parameters(params): Parameters<GetBookmarksParams>,
    ) -> Result<CallToolResult, McpError> {
        let (_account, client, me) =
            try_tool!(self.require_me_for(params.account.as_deref()).await);

        let max_results = params.max_results.unwrap_or(20).clamp(1, 100);

        let result = client
            .get_bookmarks(&me.id, max_results, params.pagination_token.as_deref())
            .await;

        Ok(Self::ok_or_err(
            result.map(|r| Self::format_search_results("bookmarks", &r.tweets, &r.next_token)),
        ))
    }

    #[tool(
        description = "Get current trending topics on X (Twitter) for a location by WOEID. Defaults to Worldwide (WOEID 1). Returns trend names and post volumes where available. Use to discover what's happening."
    )]
    async fn get_trends(
        &self,
        Parameters(params): Parameters<GetTrendsParams>,
    ) -> Result<CallToolResult, McpError> {
        let (_account, client) = try_tool!(self.require_account(params.account.as_deref()));

        let woeid = params.woeid.unwrap_or(1);

        let result = client.get_trends(woeid).await;

        Ok(Self::ok_or_err(
            result.map(|r| Self::format_trends(&r.trends, r.woeid)),
        ))
    }

    #[tool(
        description = "Get recent direct messages on X (Twitter). Returns DM events across all conversations with sender IDs and conversation IDs."
    )]
    async fn get_dm_events(
        &self,
        Parameters(params): Parameters<GetDmEventsParams>,
    ) -> Result<CallToolResult, McpError> {
        let (_account, client) = try_tool!(self.require_account(params.account.as_deref()));

        let max_results = params.max_results.unwrap_or(20).clamp(1, 100);

        let result = client
            .get_dm_events(max_results, params.pagination_token.as_deref())
            .await;

        Ok(Self::ok_or_err(
            result.map(|r| Self::format_dm_events(&r.events, &r.next_token)),
        ))
    }

    #[tool(
        description = "Send a direct message on X (Twitter). Requires a conversation ID (get it from get_dm_events) and message text."
    )]
    async fn send_dm(
        &self,
        Parameters(params): Parameters<SendDmParams>,
    ) -> Result<CallToolResult, McpError> {
        let conversation_id = params.conversation_id.trim();
        if conversation_id.is_empty() {
            return Ok(CallToolResult::error(vec![Content::text(
                "Conversation ID cannot be empty.",
            )]));
        }
        let text = params.text.trim();
        if text.is_empty() {
            return Ok(CallToolResult::error(vec![Content::text(
                "Message text cannot be empty.",
            )]));
        }

        let (_account, client) = try_tool!(self.require_account(params.account.as_deref()));

        let result = client.send_dm(conversation_id, text).await;

        Ok(Self::ok_or_err(result.map(|r| {
            format!(
                "DM sent!\nConversation: {}\nEvent ID: {}",
                r.conversation_id, r.event_id
            )
        })))
    }
}

#[tool_handler]
impl ServerHandler for XServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "mcp-server-x",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(&self.instructions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_tweet_id_from_url_variants() {
        assert_eq!(
            XServer::extract_tweet_id("https://x.com/elonmusk/status/1234567890"),
            "1234567890"
        );
        assert_eq!(
            XServer::extract_tweet_id("https://twitter.com/user/status/9876543210?s=20"),
            "9876543210"
        );
        assert_eq!(
            XServer::extract_tweet_id("https://x.com/user/status/555666777#likes"),
            "555666777"
        );
        assert_eq!(XServer::extract_tweet_id("1234567890"), "1234567890");
        assert_eq!(XServer::extract_tweet_id("   42   "), "42");
        assert_eq!(XServer::extract_tweet_id(""), "");
    }

    #[test]
    fn truncate_str_respects_char_boundaries() {
        let s = "héllo world — this is a test";
        let t = XServer::truncate_str(s, 10);
        assert!(t.len() <= 10);
        // Should be valid UTF-8
        assert!(std::str::from_utf8(t.as_bytes()).is_ok());
    }

    fn profile_params(
        name: Option<&str>,
        description: Option<&str>,
        location: Option<&str>,
        url: Option<&str>,
    ) -> UpdateProfileParams {
        UpdateProfileParams {
            account: None,
            name: name.map(str::to_string),
            description: description.map(str::to_string),
            location: location.map(str::to_string),
            url: url.map(str::to_string),
        }
    }

    #[test]
    fn validate_profile_lengths_accepts_valid_and_empty_clears() {
        // In-range values, plus empty strings (used to clear a field) are fine.
        let p = profile_params(Some("New Name"), Some(""), Some(""), Some(""));
        assert!(XServer::validate_profile_lengths(&p).is_none());
    }

    #[test]
    fn validate_profile_lengths_rejects_overlong_bio() {
        let long_bio = "x".repeat(161);
        let p = profile_params(None, Some(&long_bio), None, None);
        let err = XServer::validate_profile_lengths(&p).unwrap();
        assert!(err.contains("description is too long"));
        assert!(err.contains("161"));
    }

    #[test]
    fn validate_profile_lengths_counts_unicode_scalars() {
        // 160 multi-byte chars is within the 160-char limit despite >160 bytes.
        let bio: String = "é".repeat(160);
        let p = profile_params(None, Some(&bio), None, None);
        assert!(XServer::validate_profile_lengths(&p).is_none());
    }

    #[test]
    fn validate_profile_lengths_rejects_blank_name() {
        // A provided-but-empty name would blank the display name — reject it.
        let p = profile_params(Some("   "), None, None, None);
        let err = XServer::validate_profile_lengths(&p).unwrap();
        assert!(err.contains("name cannot be empty"));
    }

    #[test]
    fn format_trends_worldwide_default() {
        let trends = vec![
            Trend {
                name: "#RustLang".to_string(),
                tweet_count: Some(42000),
            },
            Trend {
                name: "Breaking News".to_string(),
                tweet_count: None,
            },
        ];
        let output = XServer::format_trends(&trends, 1);
        assert!(output.contains("Worldwide"));
        assert!(output.contains("WOEID 1"));
        assert!(output.contains("#RustLang (42000 posts)"));
        assert!(output.contains("Breaking News"));
        assert!(!output.contains("No trends"));
    }

    #[test]
    fn format_trends_specific_locations_and_empty() {
        let trends = vec![Trend {
            name: "#Test".to_string(),
            tweet_count: Some(100),
        }];
        let us = XServer::format_trends(&trends, 23424977);
        assert!(us.contains("United States"));
        let jp = XServer::format_trends(&trends, 23424856);
        assert!(jp.contains("Japan"));
        let empty = XServer::format_trends(&[], 1);
        assert!(empty.contains("No trends found"));
    }
}
