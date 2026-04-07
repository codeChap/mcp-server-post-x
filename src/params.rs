use crate::api::MediaAttachment;
use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MediaAttachmentParam {
    #[schemars(description = "Local file path to the media file (jpeg, png, gif, webp, mp4)")]
    pub path: String,
    #[schemars(
        description = "Alt text for the media (images and GIFs only, not supported for video)"
    )]
    pub alt_text: Option<String>,
}

impl From<MediaAttachmentParam> for MediaAttachment {
    fn from(p: MediaAttachmentParam) -> Self {
        Self {
            path: p.path,
            alt_text: p.alt_text,
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UploadMediaParams {
    #[schemars(
        description = "Local file path to media file. Supported: jpeg/png/webp (max 5MB), gif (max 15MB), mp4 (max 512MB)"
    )]
    pub path: String,
    #[schemars(
        description = "Alt text for the media (images and GIFs only, not supported for video)"
    )]
    pub alt_text: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PostTweetParams {
    #[schemars(description = "The tweet text")]
    pub text: String,
    #[schemars(
        description = "Media attachments to upload and attach (max 4 images, or 1 video, or 1 GIF). Cannot be used with media_ids."
    )]
    pub media: Option<Vec<MediaAttachmentParam>>,
    #[schemars(
        description = "Pre-uploaded media IDs to attach (max 4). Cannot be used with media. Type validation is skipped for pre-uploaded IDs."
    )]
    pub media_ids: Option<Vec<String>>,
    #[schemars(
        description = "Tweet ID to reply to (e.g. '123456'). When set, the tweet is posted as a reply to the specified tweet."
    )]
    pub reply_to: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ThreadTweet {
    #[schemars(description = "The tweet text")]
    pub text: String,
    #[schemars(description = "Media attachments (max 4 images, or 1 video, or 1 GIF)")]
    pub media: Option<Vec<MediaAttachmentParam>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PostThreadParams {
    #[schemars(
        description = "Array of tweets to post as a thread (max 25). Each tweet has 'text' and optional 'media'."
    )]
    pub tweets: Vec<ThreadTweet>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TimelineParams {
    #[schemars(description = "Maximum results to return (1-100, default 20)")]
    pub max_results: Option<u32>,
    #[schemars(description = "Exclude 'replies', 'retweets', or both comma-separated")]
    pub exclude: Option<String>,
    #[schemars(description = "Pagination token from a previous response to get the next page")]
    pub pagination_token: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetDmEventsParams {
    #[schemars(description = "Maximum results to return (1-100, default 20)")]
    pub max_results: Option<u32>,
    #[schemars(description = "Pagination token from a previous response to get the next page")]
    pub pagination_token: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SendDmParams {
    #[schemars(
        description = "The DM conversation ID (e.g. '123456-789012' for 1-on-1 conversations)"
    )]
    pub conversation_id: String,
    #[schemars(description = "The message text to send")]
    pub text: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchTweetsParams {
    #[schemars(
        description = "Search query. Supports Twitter operators like from:user, #hashtag, @mention, \"exact phrase\", -exclude, lang:en, etc."
    )]
    pub query: String,
    #[schemars(description = "Maximum results to return (10-100, default 10)")]
    pub max_results: Option<u32>,
    #[schemars(
        description = "Sort order: 'recency' (newest first) or 'relevancy' (most relevant first)"
    )]
    pub sort_order: Option<String>,
    #[schemars(description = "Pagination token from a previous response to get the next page")]
    pub pagination_token: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TweetIdParams {
    #[schemars(
        description = "The tweet ID or tweet URL (e.g. '123456' or 'https://x.com/user/status/123456')"
    )]
    pub tweet_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LookupUserParams {
    #[schemars(description = "Username (with or without @) or numeric user ID")]
    pub user: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FollowsLookupParams {
    #[schemars(description = "Maximum number of results to return (1-100, default 20)")]
    pub max_results: Option<u32>,
    #[schemars(description = "Pagination token from a previous response to get the next page")]
    pub pagination_token: Option<String>,
}
