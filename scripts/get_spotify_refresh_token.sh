#!/usr/bin/env bash
set -euo pipefail

CLIENT_ID=${SPOTIFY_CLIENT_ID:-}
CLIENT_SECRET=${SPOTIFY_CLIENT_SECRET:-}
REDIRECT_URI="http://127.0.0.1:8888"
PORT=8888

# Recommended scopes for playback control
SCOPES="user-read-playback-state user-modify-playback-state user-read-currently-playing"

if [ -z "$CLIENT_ID" ]; then
  read -rp "Spotify Client ID: " CLIENT_ID
fi
if [ -z "$CLIENT_SECRET" ]; then
  read -rsp "Spotify Client Secret: " CLIENT_SECRET
  echo
fi

# URL-encode function for POSIX shells
urlencode() {
  python3 -c "import urllib.parse, sys; print(urllib.parse.quote(sys.stdin.read().strip(), safe=''))"
}

ENC_SCOPE=$(printf "%s" "$SCOPES" | urlencode)
ENC_REDIRECT=$(printf "%s" "$REDIRECT_URI" | urlencode)
STATE=$(date +%s%N)

AUTH_URL="https://accounts.spotify.com/authorize?response_type=code&client_id=${CLIENT_ID}&scope=${ENC_SCOPE}&redirect_uri=${ENC_REDIRECT}&state=${STATE}&show_dialog=true"

echo "Open this URL in your browser (or it should open automatically):"
echo
echo "$AUTH_URL"
echo

# Try to open browser
if command -v xdg-open >/dev/null 2>&1; then
  xdg-open "$AUTH_URL" >/dev/null 2>&1 || true
elif command -v open >/dev/null 2>&1; then
  open "$AUTH_URL" >/dev/null 2>&1 || true
fi

TMP_CODE_FILE="/tmp/spotify_auth_code_${STATE}.txt"

# Python tiny HTTP server to capture the /callback?code=... request
python3 - <<PYTHON > /dev/null &
import http.server, socketserver, urllib.parse, sys, pathlib
PORT = ${PORT}
OUT = pathlib.Path('${TMP_CODE_FILE}')
class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        parsed = urllib.parse.urlparse(self.path)
        qs = urllib.parse.parse_qs(parsed.query)
        code = qs.get('code', [''])[0]
        state = qs.get('state', [''])[0]
        self.send_response(200)
        self.send_header('Content-type', 'text/html')
        self.end_headers()
        if code:
            self.wfile.write(b"<html><body><h1>Authorization received</h1><p>You can close this window.</p></body></html>")
            OUT.write_text(code)
        else:
            self.wfile.write(b"<html><body><h1>No code found</h1></body></html>")
        # stop the server after one request
        return

with socketserver.TCPServer(("", PORT), Handler) as httpd:
    httpd.handle_request()
PYTHON

echo "Waiting for browser authorization..."
for i in {0..120}; do
  if [ -f "$TMP_CODE_FILE" ]; then
    CODE=$(cat "$TMP_CODE_FILE")
    rm -f "$TMP_CODE_FILE"
    break
  fi
  sleep 1
done

if [ -z "${CODE:-}" ]; then
  echo "Timed out waiting for authorization code." >&2
  exit 1
fi

# Exchange code for tokens
res=$(curl -s -X POST -u "$CLIENT_ID:$CLIENT_SECRET" \
  -d grant_type=authorization_code \
  -d code="$CODE" \
  -d redirect_uri="$REDIRECT_URI" \
  https://accounts.spotify.com/api/token)

if echo "$res" | grep -q "refresh_token"; then
  REFRESH=$(echo "$res" | python3 -c "import sys, json; print(json.load(sys.stdin)['refresh_token'])")
  ACCESS=$(echo "$res" | python3 -c "import sys, json; print(json.load(sys.stdin)['access_token'])")
  EXPIRES=$(echo "$res" | python3 -c "import sys, json; print(json.load(sys.stdin)['expires_in'])")
  echo
  echo "SUCCESS!" 
  echo "Refresh token (add to your .env as SPOTIFY_REFRESH_TOKEN):"
  echo
  echo "$REFRESH"
  echo
  echo "Access token (expires_in=${EXPIRES}): $ACCESS" >/dev/stderr
  echo
  exit 0
else
  echo "Failed to exchange code for tokens. Response:" >&2
  echo "$res" >&2
  exit 1
fi
