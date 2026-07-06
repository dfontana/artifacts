#! /bin/bash
set -euo pipefail

# Fetch and cache https://api.artifactsmmo.com/openapi.json into ../spec.json,
# only re-fetching when the API's version has changed. The API doesn't send
# an ETag or Last-Modified header, but it does send `x-app-version`, so we use
# that as the freshness signal: a cheap HEAD request first, compared against
# the version we last cached.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SPEC_URL="https://api.artifactsmmo.com/openapi.json"
SPEC_FILE="$SCRIPT_DIR/../spec.json"
VERSION_FILE="$SCRIPT_DIR/../spec.version"

remote_version="$(curl -sI "$SPEC_URL" | tr -d '\r' | grep -i '^x-app-version:' | cut -d' ' -f2- || true)"

if [[ -f "$SPEC_FILE" && -f "$VERSION_FILE" && -n "$remote_version" ]]; then
  cached_version="$(cat "$VERSION_FILE")"
  if [[ "$remote_version" == "$cached_version" ]]; then
    echo "spec.json is up to date (version $cached_version)"
    exit 0
  fi
fi

echo "Fetching openapi spec (version ${remote_version:-unknown})..."
curl -sf "$SPEC_URL" -o "$SPEC_FILE"

if [[ -n "$remote_version" ]]; then
  echo "$remote_version" > "$VERSION_FILE"
else
  rm -f "$VERSION_FILE"
fi

echo "Cached spec.json"
