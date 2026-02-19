use crate::api::{Config, PostResult, XClient};
use crate::params::{PostThreadParams, PostTweetParams};
use rmcp::{
    ErrorData as McpError, ServerHandler, handler::server::tool::ToolRouter,
    handler::server::wrapper::Parameters, model::*, tool, tool_handler, tool_router,
};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct PostXServer {
    client: Arc<XClient>,
    cached_username: Arc<Mutex<Option<String>>>,
    tool_router: ToolRouter<Self>,
}

impl PostXServer {
    async fn ensure_username(&self) -> Result<String, String> {
        {
            let cached = self.cached_username.lock().await;
            if let Some(ref username) = *cached {
                return Ok(username.clone());
            }
        }

        let me = self.client.get_me().await?;
        let username = me.username.clone();
        {
            let mut cached = self.cached_username.lock().await;
            *cached = Some(username.clone());
        }
        Ok(username)
    }

    fn format_post_result(result: &PostResult) -> String {
        format!("Tweet posted!\nID: {}\nURL: {}", result.tweet_id, result.url)
    }
}

#[tool_router]
impl PostXServer {
    pub fn new(config: Config) -> Self {
        Self {
            client: Arc::new(XClient::new(config)),
            cached_username: Arc::new(Mutex::new(None)),
            tool_router: Self::tool_router(),
        }
    }

    #[tool(description = "Post a single tweet to X (Twitter), optionally with an image attachment")]
    async fn post_tweet(
        &self,
        Parameters(params): Parameters<PostTweetParams>,
    ) -> Result<CallToolResult, McpError> {
        let username = match self.ensure_username().await {
            Ok(u) => u,
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(e)])),
        };

        match self
            .client
            .post_tweet(&params.text, params.image.as_deref(), None, &username)
            .await
        {
            Ok(result) => Ok(CallToolResult::success(vec![Content::text(
                Self::format_post_result(&result),
            )])),
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e)])),
        }
    }

    #[tool(
        description = "Post a thread of tweets to X (Twitter). Each tweet can optionally include an image. Max 25 tweets per thread."
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

        let username = match self.ensure_username().await {
            Ok(u) => u,
            Err(e) => return Ok(CallToolResult::error(vec![Content::text(e)])),
        };

        let tweets: Vec<(String, Option<String>)> = params
            .tweets
            .into_iter()
            .map(|t| (t.text, t.image))
            .collect();

        let result = self.client.post_thread(&tweets, &username).await;

        let mut output = String::new();
        if !result.posted.is_empty() {
            output.push_str(&format!(
                "Posted {}/{} tweets:\n",
                result.posted.len(),
                tweets.len()
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
            // Partial failure or full failure — always report as error
            if result.posted.is_empty() {
                return Ok(CallToolResult::error(vec![Content::text(output)]));
            }
            // Partial success — still report as error so caller knows it's incomplete
            return Ok(CallToolResult::error(vec![Content::text(output)]));
        }

        Ok(CallToolResult::success(vec![Content::text(output)]))
    }

    #[tool(description = "Get the authenticated X (Twitter) user's profile (id, name, username). Useful for verifying credentials.")]
    async fn get_me(&self) -> Result<CallToolResult, McpError> {
        match self.client.get_me().await {
            Ok(me) => {
                // Update cached username
                {
                    let mut cached = self.cached_username.lock().await;
                    *cached = Some(me.username.clone());
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
                "X (Twitter) posting server. Use post_tweet to post a single tweet, \
                 post_thread to post a thread, or get_me to verify credentials."
                    .to_string(),
            ),
        }
    }
}
