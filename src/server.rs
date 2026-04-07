macro_rules! try_tool {
    ($expr:expr) => {
        match $expr {
            Ok(v) => v,
            Err(e) => return Ok(e),
        }
    };
}

use crate::api::{
    Config, DmEventResult, MeData, MediaAttachment, PostResult, SearchTweetResult, UserProfile,
    UserSummary, XClient,
};
use crate::params::{
    FollowsLookupParams, GetDmEventsParams, LookupUserParams, PostThreadParams, PostTweetParams,
    SearchTweetsParams, SendDmParams, TimelineParams, TweetIdParams, UploadMediaParams,
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

    fn require_me(result: Result<MeData, String>) -> Result<MeData, CallToolResult> {
        result.map_err(|e| CallToolResult::error(vec![Content::text(e)]))
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

    fn format_post_result(result: &PostResult) -> String {
        format!("Tweet posted!\nID: {}\nURL: {}", result.tweet_id, result.url)
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
        let has_media = params.media.as_ref().is_some_and(|m| !m.is_empty());
        let has_media_ids = params.media_ids.as_ref().is_some_and(|ids| !ids.is_empty());
        if has_media && has_media_ids {
            return Ok(CallToolResult::error(vec![Content::text(
                "'media' and 'media_ids' are mutually exclusive. Use one or the other.",
            )]));
        }

        let me = try_tool!(Self::require_me(self.ensure_me().await));

        let media_attachments: Vec<MediaAttachment> = params
            .media
            .unwrap_or_default()
            .into_iter()
            .map(Into::into)
            .collect();

        let result = self
            .client
            .post_tweet(
                &params.text,
                &media_attachments,
                params.media_ids.as_deref(),
                params.reply_to.as_deref(),
                &me.username,
            )
            .await;

        Ok(Self::ok_or_err(result.map(|r| Self::format_post_result(&r))))
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

        let me = try_tool!(Self::require_me(self.ensure_me().await));

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
        let result = self
            .client
            .upload_media(&params.path, params.alt_text.as_deref())
            .await;

        Ok(Self::ok_or_err(result.map(|r| {
            format!(
                "Media uploaded!\nMedia ID: {}\nType: {}\nState: {}",
                r.media_id, r.media_type, r.state
            )
        })))
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
        let me = try_tool!(Self::require_me(self.ensure_me().await));

        let max_results = params.max_results.unwrap_or(20).clamp(1, 100);

        let result = self
            .client
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
        let me = try_tool!(Self::require_me(self.ensure_me().await));

        let max_results = params.max_results.unwrap_or(20).clamp(1, 100);

        let result = self
            .client
            .get_following(&me.id, max_results, params.pagination_token.as_deref())
            .await;

        Ok(Self::ok_or_err(
            result.map(|r| Self::format_follows(&r.users, &r.next_token, "following")),
        ))
    }

    #[tool(
        description = "Get ALL accounts the authenticated user follows on X (Twitter). Auto-paginates to fetch every account. Returns usernames, display names, follower counts, and bios."
    )]
    async fn get_all_following(&self) -> Result<CallToolResult, McpError> {
        let me = try_tool!(Self::require_me(self.ensure_me().await));

        let result = self.client.get_all_following(&me.id).await;

        Ok(Self::ok_or_err(
            result.map(|users| Self::format_all_follows(&users, "following")),
        ))
    }

    #[tool(
        description = "Get ALL followers of the authenticated user on X (Twitter). Auto-paginates to fetch every follower. Returns usernames, display names, follower counts, and bios."
    )]
    async fn get_all_followers(&self) -> Result<CallToolResult, McpError> {
        let me = try_tool!(Self::require_me(self.ensure_me().await));

        let result = self.client.get_all_followers(&me.id).await;

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
        let me = try_tool!(Self::require_me(self.ensure_me().await));

        let target_id = match self.client.resolve_user_id(&params.user).await {
            Ok(id) => id,
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(e)])),
        };

        Ok(Self::ok_or_err(
            self.client
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
        let me = try_tool!(Self::require_me(self.ensure_me().await));

        let target_id = match self.client.resolve_user_id(&params.user).await {
            Ok(id) => id,
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(e)])),
        };

        Ok(Self::ok_or_err(
            self.client
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
            self.client.lookup_user_by_id(user).await
        } else {
            self.client.lookup_user_by_username(user).await
        };

        Ok(Self::ok_or_err(result.map(|p| Self::format_user_profile(&p))))
    }

    #[tool(description = "Like a tweet on X (Twitter). Accepts a tweet ID or tweet URL.")]
    async fn like_tweet(
        &self,
        Parameters(params): Parameters<TweetIdParams>,
    ) -> Result<CallToolResult, McpError> {
        let tweet_id = try_tool!(Self::require_tweet_id(&params.tweet_id));

        let me = try_tool!(Self::require_me(self.ensure_me().await));

        Ok(Self::ok_or_err(
            self.client
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

        let me = try_tool!(Self::require_me(self.ensure_me().await));

        Ok(Self::ok_or_err(
            self.client
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

        Ok(Self::ok_or_err(
            self.client
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

        let me = try_tool!(Self::require_me(self.ensure_me().await));

        Ok(Self::ok_or_err(
            self.client
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

        let me = try_tool!(Self::require_me(self.ensure_me().await));

        Ok(Self::ok_or_err(
            self.client
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

        let max_results = params.max_results.unwrap_or(10).clamp(10, 100);

        let result = self
            .client
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
        let me = try_tool!(Self::require_me(self.ensure_me().await));

        let max_results = params.max_results.unwrap_or(20).clamp(1, 100);

        let result = self
            .client
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
        let max_results = params.max_results.unwrap_or(20).clamp(1, 100);

        let result = self
            .client
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

        let result = self.client.send_dm(conversation_id, text).await;

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
            .with_instructions(
                "X (Twitter) server. Tools: post_tweet, post_thread, upload_media, \
                 delete_tweet, search_tweets, get_timeline, get_me, lookup_user, \
                 get_followers, get_following, get_all_followers, get_all_following, \
                 follow_user, unfollow_user, like_tweet, unlike_tweet, retweet, \
                 unretweet, get_dm_events, send_dm.",
            )
    }
}
