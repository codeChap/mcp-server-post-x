# mcp-server-post-x

An MCP (Model Context Protocol) server for X (Twitter). Built in Rust using OAuth 1.0a and the X API v2. Supports multiple accounts.

Communicates via stdio using JSON-RPC 2.0.

## Tools

| Tool | Description |
|------|-------------|
| `list_accounts` | List available accounts and which is the default |
| `post_tweet` | Post a tweet with optional media (up to 4 images, 1 video, or 1 GIF) |
| `post_thread` | Post a thread of up to 25 tweets, each with optional media |
| `delete_tweet` | Delete a tweet by ID or URL |
| `upload_media` | Upload media for later attachment (returns a media_id) |
| `search_tweets` | Search recent tweets (last 7 days) with Twitter operators |
| `get_timeline` | Get your home timeline in reverse chronological order |
| `get_me` | Get the authenticated user's profile |
| `lookup_user` | Look up any user by @username or numeric ID |
| `get_followers` | List your followers (paginated) |
| `get_following` | List who you follow (paginated) |
| `get_all_followers` | Fetch ALL your followers in a single call (auto-paginates) |
| `get_all_following` | Fetch ALL accounts you follow in a single call (auto-paginates) |
| `like_tweet` | Like a tweet by ID or URL |
| `unlike_tweet` | Unlike a tweet by ID or URL |
| `retweet` | Retweet a tweet by ID or URL |
| `unretweet` | Undo a retweet by ID or URL |
| `get_dm_events` | Get recent direct messages across all conversations |
| `send_dm` | Send a direct message to a conversation |
| `follow_user` | Follow a user by username or ID |
| `unfollow_user` | Unfollow a user by username or ID |

All tools accept an optional `account` parameter to select which X account to use. Omit it to use the default account.

## Quick Start

### 1. Build

```bash
cargo build --release
```

Produces `target/release/post-x` (optimized with LTO, stripped).

### 2. Configure credentials

Create the config file:

```bash
mkdir -p ~/.config/mcp-server-post-x
```

Create `~/.config/mcp-server-post-x/config.toml`:

**Single account** (no `default_account` needed):

```toml
[accounts.myaccount]
api_key = "your-api-key"
api_key_secret = "your-api-key-secret"
access_token = "your-access-token"
access_token_secret = "your-access-token-secret"
```

**Multiple accounts:**

```toml
default_account = "myaccount"

[accounts.myaccount]
api_key = "your-api-key"
api_key_secret = "your-api-key-secret"
access_token = "your-access-token"
access_token_secret = "your-access-token-secret"

[accounts.otheraccount]
api_key = "your-api-key"
api_key_secret = "your-api-key-secret"
access_token = "other-access-token"
access_token_secret = "other-access-token-secret"
```

Notes:
- Account keys are X usernames (e.g. `[accounts.codechap]`)
- If you have multiple accounts, `default_account` is required
- If you have one account, `default_account` is optional (auto-detected)
- Multiple accounts can share the same `api_key`/`api_key_secret` (same X app). Only the `access_token`/`access_token_secret` differ per account.

Secure it:

```bash
chmod 700 ~/.config/mcp-server-post-x
chmod 600 ~/.config/mcp-server-post-x/config.toml
```

See [Getting credentials](#getting-credentials) below for how to obtain these.

### 3. Add to your MCP client

Claude Code (`~/.claude.json`):

```json
{
  "mcpServers": {
    "post-x": {
      "command": "/path/to/post-x"
    }
  }
}
```

Then ask Claude things like:

- "Post a tweet saying hello world"
- "Post a tweet as securechap saying hello world"
- "Search for tweets about Rust"
- "Show me my timeline"
- "Like this tweet: https://x.com/someone/status/123456"
- "Who are my followers?"
- "Look up @elonmusk"
- "List my accounts"

## Tool Reference

### list_accounts

No required parameters. Returns available account names, which is the default, and cached usernames.

### post_tweet

| Param | Type | Required | Description |
|-------|------|----------|-------------|
| `account` | string | no | Account to use (omit for default) |
| `text` | string | yes | Tweet text (max 280 characters) |
| `media` | array | no | Media to upload and attach. Each item: `{ path, alt_text? }`. Max 4 images, or 1 video, or 1 GIF. |
| `media_ids` | array | no | Pre-uploaded media IDs to attach (max 4). Mutually exclusive with `media`. |
| `reply_to` | string | no | Tweet ID to reply to |

### post_thread

| Param | Type | Required | Description |
|-------|------|----------|-------------|
| `account` | string | no | Account to use (omit for default) |
| `tweets` | array | yes | Array of tweets (max 25). Each: `{ text, media? }` |

### delete_tweet / like_tweet / unlike_tweet / retweet / unretweet

| Param | Type | Required | Description |
|-------|------|----------|-------------|
| `account` | string | no | Account to use (omit for default) |
| `tweet_id` | string | yes | Tweet ID or full tweet URL |

All accept URLs like `https://x.com/user/status/123456` — the ID is extracted automatically.

### upload_media

| Param | Type | Required | Description |
|-------|------|----------|-------------|
| `account` | string | no | Account to use (omit for default) |
| `path` | string | yes | Local file path. Supported: jpeg/png/webp (max 5MB), gif (max 15MB), mp4 (max 512MB) |
| `alt_text` | string | no | Alt text (images and GIFs only, not video) |

Returns a `media_id` to use with `post_tweet`'s `media_ids` param.

### search_tweets

| Param | Type | Required | Description |
|-------|------|----------|-------------|
| `account` | string | no | Account to use (omit for default) |
| `query` | string | yes | Search query. Supports: `from:user`, `#hashtag`, `@mention`, `"exact phrase"`, `-exclude`, `lang:en` |
| `max_results` | integer | no | 10-100 (default 10) |
| `sort_order` | string | no | `recency` or `relevancy` |
| `pagination_token` | string | no | Next page token from previous response |

### get_timeline

| Param | Type | Required | Description |
|-------|------|----------|-------------|
| `account` | string | no | Account to use (omit for default) |
| `max_results` | integer | no | 1-100 (default 20) |
| `exclude` | string | no | `replies`, `retweets`, or both comma-separated |
| `pagination_token` | string | no | Next page token |

### lookup_user / follow_user / unfollow_user

| Param | Type | Required | Description |
|-------|------|----------|-------------|
| `account` | string | no | Account to use (omit for default) |
| `user` | string | yes | Username (with or without `@`) or numeric user ID |

### get_followers / get_following

| Param | Type | Required | Description |
|-------|------|----------|-------------|
| `account` | string | no | Account to use (omit for default) |
| `max_results` | integer | no | 1-100 (default 20) |
| `pagination_token` | string | no | Next page token |

### get_all_followers / get_all_following

| Param | Type | Required | Description |
|-------|------|----------|-------------|
| `account` | string | no | Account to use (omit for default) |

Auto-paginates through all results (100 per page) and returns the complete list in a single response. Includes a 200ms delay between pages to respect rate limits.

### get_dm_events

| Param | Type | Required | Description |
|-------|------|----------|-------------|
| `account` | string | no | Account to use (omit for default) |
| `max_results` | integer | no | 1-100 (default 20) |
| `pagination_token` | string | no | Next page token |

### send_dm

| Param | Type | Required | Description |
|-------|------|----------|-------------|
| `account` | string | no | Account to use (omit for default) |
| `conversation_id` | string | yes | DM conversation ID (get from `get_dm_events`) |
| `text` | string | yes | Message text |

### get_me

| Param | Type | Required | Description |
|-------|------|----------|-------------|
| `account` | string | no | Account to use (omit for default) |

Returns your user ID, display name, and @username.

## Adding Additional Accounts

To add another X account to an existing app without a separate developer account, use the included OAuth authorization script:

```bash
./oauth-authorize.sh
```

This runs the 3-legged OAuth 1.0a PIN-based flow:
1. Opens a URL where the new account authorizes your app
2. You paste the PIN back into the terminal
3. It outputs the `[accounts.username]` config block to add to your `config.toml`

All accounts share the same app and billing credits.

## Getting Credentials

1. Go to [developer.x.com](https://developer.x.com/) and sign up for a developer account
2. Create a Project and an App in the Developer Console
3. In your App settings, set up **User authentication**:
   - App permissions: **Read and write** (and **Direct Messages** if you want DM support)
   - Type: **Web App, Automated App or Bot**
   - Callback URL: `https://example.com` (not used, but required)
   - Website URL: any valid URL
4. Go to **Keys and tokens** and generate:
   - **API Key** and **API Key Secret** (under Consumer Keys)
   - **Access Token** and **Access Token Secret** (under Authentication Tokens)
5. Copy all four values into your `config.toml` under `[accounts.yourusername]`

The server validates credentials at startup. If you get persistent 401 errors, regenerate your tokens at [developer.x.com](https://developer.x.com/).

## Development

```bash
cargo build              # debug build
cargo run                # run in dev mode
RUST_LOG=debug cargo run # debug logging (credentials are redacted)
```

## Technical Details

- **Auth:** OAuth 1.0a with HMAC-SHA1 signatures (RFC 5849, RFC 3986 percent-encoding)
- **Multi-account:** Multiple X accounts per server instance, selectable per tool call
- **Tweet API:** X API v2 (`api.x.com/2/`)
- **Media upload:** v1.1 chunked upload (`upload.twitter.com/1.1/media/upload.json`) — INIT/APPEND/FINALIZE/STATUS flow for video/GIF, simple multipart for images
- **Media limits:** JPEG/PNG/WebP up to 5MB, GIF up to 15MB, MP4 up to 512MB
- **Media validation:** Max 4 images OR 1 video OR 1 GIF per tweet (no mixing)
- **Thread posting:** 500ms delay between tweets, chained via `in_reply_to_tweet_id`
- **Retry logic:** Automatic retry with exponential backoff on 503 errors
- **Rate limits:** 429 responses include reset timestamp in error message (no auto-retry — the caller decides)

## Project Structure

```
src/
  main.rs    — entry point, config loading, tracing, stdio transport
  server.rs  — MCP tool handlers, response formatting, multi-account routing
  api.rs     — X API client: OAuth signing, tweet/media/user/DM endpoints
  params.rs  — tool parameter types (serde + JSON Schema)
```
