# Rename mcp-server-post-x → mcp-server-x

The server is a full X API MCP (post, search, timeline, DMs, follows, profile), not a poster. Sibling servers are named after the service.

## Names

| Surface | From | To |
|---------|------|----|
| Folder | `mcp-server-post-x` | `mcp-server-x` |
| GitHub | `codeChap/mcp-server-post-x` | `codeChap/mcp-server-x` |
| Crate | `mcp-server-post-x` | `mcp-server-x` |
| Binary | `post-x` | `mcp-server-x` |
| MCP client key | `post-x` | `x` |
| ServerInfo | `mcp-server-post-x` | `mcp-server-x` |
| Rust type | `PostXServer` | `XServer` |
| Config dir | `~/.config/mcp-server-post-x/` | `~/.config/mcp-server-x/` |
| Env prefix | `POST_X_*` | `X_*` |

Tool names stay (`post_tweet`, `get_timeline`, …). They describe actions.

Binary is `mcp-server-x`, not `x` (PATH collision).

## Compatibility

- Config: prefer `mcp-server-x/config.toml`; if missing, use `mcp-server-post-x/config.toml`; if neither exists, error with the new path.
- Env: `X_API_KEY` (etc.) first, then `POST_X_*`. Same for `X_ACCOUNT_NAME` / `POST_X_ACCOUNT_NAME`.
- Do not copy secrets. Do not auto-migrate the config file.

## Out of repo (this machine)

- `~/.grok/config.toml` MCP key + command path
- `~/.claude.json` MCP key + command path
- `~/.grok/skills/tweet-playbook/` server name `x`
- `/media/codechap/4TB/develop/mcps/CLAUDE.md`

## Non-goals

- Renaming MCP tools
- Rewriting session transcripts
- Auto-moving `~/.config/mcp-server-post-x`
