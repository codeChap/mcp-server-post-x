use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PostTweetParams {
    #[schemars(description = "The tweet text (max 280 characters)")]
    pub text: String,
    #[schemars(description = "Optional local file path to an image to attach (jpeg, png, gif, webp; max 5MB)")]
    pub image: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ThreadTweet {
    #[schemars(description = "The tweet text (max 280 characters)")]
    pub text: String,
    #[schemars(description = "Optional local file path to an image to attach (jpeg, png, gif, webp; max 5MB)")]
    pub image: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PostThreadParams {
    #[schemars(description = "Array of tweets to post as a thread (max 25). Each tweet has 'text' and optional 'image'.")]
    pub tweets: Vec<ThreadTweet>,
}
