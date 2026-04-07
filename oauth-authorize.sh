#!/usr/bin/env bash
# 3-legged OAuth 1.0a PIN-based flow for X (Twitter)
# Authorizes a user account against an existing app.
set -euo pipefail

API_KEY="AL2pOHlmB6HR2CmEnFYE1Hs3Y"
API_KEY_SECRET="rRsIXAdkwJDJv6mDFsYghdxHBuVWN51jzKTqDqBU5aSPqh5az1"

REQUEST_TOKEN_URL="https://api.twitter.com/oauth/request_token"
AUTHORIZE_URL="https://api.twitter.com/oauth/authorize"
ACCESS_TOKEN_URL="https://api.twitter.com/oauth/access_token"
CALLBACK="oob"

# --- helpers ---

nonce() { openssl rand -hex 16; }
timestamp() { date +%s; }

pct_encode() {
    python3 -c "import urllib.parse, sys; print(urllib.parse.quote(sys.argv[1], safe=''))" "$1"
}

sign() {
    local method="$1" url="$2" params="$3" token_secret="${4:-}"
    local base="${method}&$(pct_encode "$url")&$(pct_encode "$params")"
    local key="$(pct_encode "$API_KEY_SECRET")&$(pct_encode "$token_secret")"
    echo -n "$base" | openssl dgst -sha1 -hmac "$key" -binary | base64
}

# --- Step 1: get request token ---

echo "=== Step 1: Requesting temporary token..."

NONCE=$(nonce)
TS=$(timestamp)

PARAMS="oauth_callback=$(pct_encode "$CALLBACK")&oauth_consumer_key=${API_KEY}&oauth_nonce=${NONCE}&oauth_signature_method=HMAC-SHA1&oauth_timestamp=${TS}&oauth_version=1.0"
SIG=$(sign "POST" "$REQUEST_TOKEN_URL" "$PARAMS" "")
SIG_ENC=$(pct_encode "$SIG")

RESPONSE=$(curl -s -X POST "$REQUEST_TOKEN_URL" \
    -H "Authorization: OAuth oauth_callback=\"$(pct_encode "$CALLBACK")\", oauth_consumer_key=\"${API_KEY}\", oauth_nonce=\"${NONCE}\", oauth_signature=\"${SIG_ENC}\", oauth_signature_method=\"HMAC-SHA1\", oauth_timestamp=\"${TS}\", oauth_version=\"1.0\"")

if echo "$RESPONSE" | grep -q "oauth_token="; then
    OAUTH_TOKEN=$(echo "$RESPONSE" | tr '&' '\n' | grep oauth_token= | head -1 | cut -d= -f2)
    OAUTH_TOKEN_SECRET=$(echo "$RESPONSE" | tr '&' '\n' | grep oauth_token_secret= | cut -d= -f2)
else
    echo "ERROR: Failed to get request token:"
    echo "$RESPONSE"
    exit 1
fi

# --- Step 2: user authorizes ---

echo ""
echo "=== Step 2: Open this URL in a browser where you're logged in as @securechap:"
echo ""
echo "  ${AUTHORIZE_URL}?oauth_token=${OAUTH_TOKEN}"
echo ""
read -rp "Enter the PIN from X: " PIN

# --- Step 3: exchange for access token ---

echo ""
echo "=== Step 3: Exchanging PIN for access token..."

NONCE=$(nonce)
TS=$(timestamp)

PARAMS="oauth_consumer_key=${API_KEY}&oauth_nonce=${NONCE}&oauth_signature_method=HMAC-SHA1&oauth_timestamp=${TS}&oauth_token=${OAUTH_TOKEN}&oauth_verifier=${PIN}&oauth_version=1.0"
SIG=$(sign "POST" "$ACCESS_TOKEN_URL" "$PARAMS" "$OAUTH_TOKEN_SECRET")
SIG_ENC=$(pct_encode "$SIG")

RESPONSE=$(curl -s -X POST "$ACCESS_TOKEN_URL" \
    -H "Authorization: OAuth oauth_consumer_key=\"${API_KEY}\", oauth_nonce=\"${NONCE}\", oauth_signature=\"${SIG_ENC}\", oauth_signature_method=\"HMAC-SHA1\", oauth_timestamp=\"${TS}\", oauth_token=\"${OAUTH_TOKEN}\", oauth_version=\"1.0\"" \
    -d "oauth_verifier=${PIN}")

if echo "$RESPONSE" | grep -q "oauth_token="; then
    ACCESS_TOKEN=$(echo "$RESPONSE" | tr '&' '\n' | grep oauth_token= | head -1 | cut -d= -f2)
    ACCESS_TOKEN_SECRET=$(echo "$RESPONSE" | tr '&' '\n' | grep oauth_token_secret= | cut -d= -f2)
    SCREEN_NAME=$(echo "$RESPONSE" | tr '&' '\n' | grep screen_name= | cut -d= -f2)

    echo ""
    echo "=== Success! Authorized as @${SCREEN_NAME}"
    echo ""
    echo "Add this to ~/.config/mcp-server-post-x/config.toml:"
    echo ""
    echo "[accounts.${SCREEN_NAME}]"
    echo "api_key = \"${API_KEY}\""
    echo "api_key_secret = \"${API_KEY_SECRET}\""
    echo "access_token = \"${ACCESS_TOKEN}\""
    echo "access_token_secret = \"${ACCESS_TOKEN_SECRET}\""
else
    echo "ERROR: Failed to get access token:"
    echo "$RESPONSE"
    exit 1
fi
