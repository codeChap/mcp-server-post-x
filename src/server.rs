macro_rules! try_tool {
    ($expr:expr) => {
        match $expr {
            Ok(v) => v,
            Err(e) => return Ok(e),
        }
    };
}

use crate::api::{
    AppConfig, DmEventResult, MeData, MediaAttachment, PostResult, SearchTweetResult, UserProfile,
    UserSummary, XClient,
};
use crate::params::{
    AccountOnlyParams, FollowsLookupParams, GetDmEventsParams, LookupUserParams,
    PostThreadParams, PostTweetParams, SearchTweetsParams, SendDmParams, TimelineParams,
    TweetIdParams, UploadMediaParams,
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
pub struct PostXServer {
    clients: HashMap<String, Arc<XClient>>,
    default_account: String,
    cached_me: Arc<Mutex<HashMap<String, MeData>>>,
    instructions: String,
    tool_router: ToolRouter<Self>,
}

impl PostXServer {
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
            if let Some(ref desc) = user.description {
                if !desc.is_empty() {
                    let truncated = if desc.len() > 100 {
                        format!("{}...", Self::truncate_str(desc, 97))
                    } else {
                        desc.clone()
                    };
                    output.push_str(&format!("     {truncated}\n"));
                }
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
            if let Some(ref desc) = user.description {
                if !desc.is_empty() {
                    let truncated = if desc.len() > 100 {
                        format!("{}...", Self::truncate_str(desc, 97))
                    } else {
                        desc.clone()
                    };
                    output.push_str(&format!("     {truncated}\n"));
                }
            }
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
            let date = created.split('T').next().unwrap_or(created);
            output.push_str(&format!("  Joined: {date}\n"));
        }
        if let Some(img) = &p.profile_image_url {
            output.push_str(&format!("  Avatar: {img}\n"));
        }

        output
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

    fn append_pagination(output: &mut String, next_token: &Option<String>) {
        if let Some(token) = next_token {
            output.push_str(&format!("\nMore results available. Next page token: {token}"));
        }
    }
}

#[tool_router]
impl PostXServer {
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
                 delete_tweet, search_tweets, get_timeline, get_me, lookup_user, \
                 get_followers, get_following, get_all_followers, get_all_following, \
                 follow_user, unfollow_user, like_tweet, unlike_tweet, retweet, \
                 unretweet, get_dm_events, send_dm, list_accounts.",
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
        description = "Get ALL accounts the authenticated user follows on X (Twitter). Auto-paginates to fetch every account. Returns usernames, display names, follower counts, and bios."
    )]
    async fn get_all_following(
        &self,
        Parameters(params): Parameters<AccountOnlyParams>,
    ) -> Result<CallToolResult, McpError> {
        let (_account, client, me) =
            try_tool!(self.require_me_for(params.account.as_deref()).await);

        let result = client.get_all_following(&me.id).await;

        Ok(Self::ok_or_err(
            result.map(|users| Self::format_all_follows(&users, "following")),
        ))
    }

    #[tool(
        description = "Get ALL followers of the authenticated user on X (Twitter). Auto-paginates to fetch every follower. Returns usernames, display names, follower counts, and bios."
    )]
    async fn get_all_followers(
        &self,
        Parameters(params): Parameters<AccountOnlyParams>,
    ) -> Result<CallToolResult, McpError> {
        let (_account, client, me) =
            try_tool!(self.require_me_for(params.account.as_deref()).await);

        let result = client.get_all_followers(&me.id).await;

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
impl ServerHandler for PostXServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "mcp-server-post-x",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(&self.instructions)
    }
}
