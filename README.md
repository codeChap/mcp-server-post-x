# mcp-server-post-x

An MCP (Model Context Protocol) server for posting to X (Twitter). Built in Rust using OAuth 1.0a authentication and the X API v2.

Communicates via stdio using JSON-RPC 2.0, like all MCP servers.

**API target date:** February 2026. The X API has been volatile — endpoints may change and require software updates.

## Tools

| Tool | Description |
|------|-------------|
| `post_tweet` | Post a single tweet, optionally with an image |
| `post_thread` | Post a thread of up to 25 tweets, each with an optional image |
| `get_me` | Get the authenticated user's profile (id, name, username) |

### post_tweet

Post a single tweet to X, optionally with an image attachment.

**Parameters:**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `text` | string | yes | Tweet text (max 280 characters) |
| `image` | string | no | Local file path to an image (jpeg, png, gif, webp; max 5MB) |

**Returns:** Tweet ID and URL on success.

### post_thread

Post a thread of tweets. Each tweet is posted as a reply to the previous one.

**Parameters:**

| Name | Type | Required | Description |
|------|------|----------|-------------|
| `tweets` | array | yes | Array of tweet objects (max 25) |
| `tweets[].text` | string | yes | Tweet text (max 280 characters) |
| `tweets[].image` | string | no | Local file path to an image |

**Returns:** List of tweet IDs and URLs for each posted tweet. If a tweet fails mid-thread, the response includes which tweets succeeded and the error for the one that failed.

### get_me

Get the authenticated user's profile. Useful for verifying that credentials are working.

**Parameters:** None.

**Returns:** User ID, display name, and @username.

## Prerequisites

1. An X developer account at [developer.x.com](https://developer.x.com/)
2. A project/app with OAuth 1.0a credentials (the free tier works)
3. All four tokens generated: API Key, API Key Secret, Access Token, Access Token Secret

## Setup

Create the config directory and file:

```bash
mkdir -p ~/.config/mcp-server-post-x
chmod 700 ~/.config/mcp-server-post-x
```

Create `~/.config/mcp-server-post-x/config.toml`:

```toml
api_key = "your-api-key"
api_key_secret = "your-api-key-secret"
access_token = "your-access-token"
access_token_secret = "your-access-token-secret"
```

Secure the file (it contains secrets):

```bash
chmod 600 ~/.config/mcp-server-post-x/config.toml
```

The server validates the config at startup and will fail fast with an actionable error if any field is missing or empty.

## Build

```bash
cargo build --release
```

This produces `target/release/post-x` (optimized with LTO and stripped).

For development:

```bash
cargo build              # debug build
cargo run                # run in dev mode
RUST_LOG=debug cargo run # run with debug logging (credentials are redacted in logs)
```

## Claude Code MCP Configuration

Add to your Claude Code MCP settings (`~/.claude/claude_desktop_config.json` or similar):

```json
{
  "mcpServers": {
    "post-x": {
      "command": "/path/to/post-x"
    }
  }
}
```

Then you can ask Claude to post tweets, e.g.:

- "Post a tweet saying hello world"
- "Post a thread with three points about Rust"
- "Post this screenshot to X" (with an image path)

## Rate Limits (Free Tier)

- ~17 tweets per 24 hours for posting
- Media uploads have separate rate limits
- The server surfaces rate limit reset times in error messages — it does not auto-retry (the caller decides when to retry)

## Technical Details

- **Authentication:** OAuth 1.0a with HMAC-SHA1 signatures and RFC 3986 percent-encoding
- **Tweet API:** X API v2 (`POST https://api.x.com/2/tweets`)
- **Media upload:** Legacy v1.1 API (`POST https://upload.twitter.com/1.1/media/upload.json`) — no v2 equivalent exists yet
- **Image validation:** File must exist, be ≤5MB, and have a jpeg/png/gif/webp extension
- **Thread posting:** 500ms delay between tweets to avoid rate limits, chained via `in_reply_to_tweet_id`
- **Credential errors:** Persistent 401 errors are logged with guidance to regenerate credentials at [developer.x.com](https://developer.x.com/)
- **Logging:** Uses `tracing` with `RUST_LOG` env filter; all credentials are redacted in debug output

## Project Structure

```
src/
  main.rs    — entry point, config loading and validation, tracing setup, stdio serve
  server.rs  — MCP tool definitions (post_tweet, post_thread, get_me) with username caching
  api.rs     — X API client: OAuth 1.0a signing, tweet posting, media upload, error handling
  params.rs  — tool parameter types with serde and JSON Schema derives
```
