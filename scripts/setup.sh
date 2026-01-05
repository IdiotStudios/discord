#!/usr/bin/env bash
set -euo pipefail

# scripts/setup.sh
# Attempts to auto-download recommended binaries into .bin/ (yt-dlp is already handled by the Rust code).
# This script tries to download a prebuilt `librespot-wrapper` helper for your platform.

BIN_DIR=".bin"
WRAPPER_BIN="librespot-wrapper"
WRAPPER_PATH="$BIN_DIR/$WRAPPER_BIN"

# Optional overrides
: "${SPOTIFY_WRAPPER_URL:=}"
: "${FORCE:=0}"

mkdir -p "$BIN_DIR"

# Detect OS/ARCH
OS=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m)

# Normalize arch
case "$ARCH" in
  x86_64|amd64) ARCH=x86_64 ;;
  aarch64|arm64) ARCH=arm64 ;;
  armv7l|armv7) ARCH=armv7 ;;
  *) ARCH=$ARCH ;;
esac

echo "Detected platform: $OS/$ARCH"

if [ "$FORCE" != "0" ] || [ ! -f "$WRAPPER_PATH" ]; then
  # 1) If SPOTIFY_WRAPPER_URL provided, try downloading it
  if [ -n "${SPOTIFY_WRAPPER_URL:-}" ]; then
    echo "Downloading spotify helper from SPOTIFY_WRAPPER_URL..."
    if curl -fsSL "$SPOTIFY_WRAPPER_URL" -o "$WRAPPER_PATH"; then
      chmod +x "$WRAPPER_PATH" || true
      echo "Downloaded helper to $WRAPPER_PATH"
      exit 0
    else
      echo "Failed to download from SPOTIFY_WRAPPER_URL: $SPOTIFY_WRAPPER_URL" >&2
    fi
  fi

  # 2) Try a list of candidate URLs for prebuilt releases (common patterns)
  BASES=(
    "https://github.com/librespot-org/librespot/releases/latest/download"
    "https://github.com/Spotifyd/spotifyd/releases/latest/download"
    "https://github.com/your-org/spotify-helper/releases/latest/download"
  )

  CANDIDATES=()

  # Common candidate filenames we attempt (ordered)
  CANDIDATES+=("librespot-${OS}-${ARCH}")
  CANDIDATES+=("librespot-${OS}-${ARCH}.tar.gz")
  CANDIDATES+=("spotifyd-${OS}-${ARCH}")
  CANDIDATES+=("spotifyd-${OS}-${ARCH}.tar.gz")
  CANDIDATES+=("librespot-wrapper-${OS}-${ARCH}")
  CANDIDATES+=("librespot-wrapper-${OS}-${ARCH}.tar.gz")

  for base in "${BASES[@]}"; do
    for cand in "${CANDIDATES[@]}"; do
      url="$base/$cand"
      echo "Trying $url"
      if curl -fsSL "$url" -o "$WRAPPER_PATH"; then
        echo "Downloaded $url -> $WRAPPER_PATH"
        chmod +x "$WRAPPER_PATH" || true
        # If we downloaded a tarball, try to extract the contained binary and place it
        if file "$WRAPPER_PATH" | grep -q 'gzip compressed data'; then
          tmpdir=$(mktemp -d)
          tar -xzf "$WRAPPER_PATH" -C "$tmpdir" || true
          # Find an executable candidate
          found=$(find "$tmpdir" -maxdepth 2 -type f -perm -111 | head -n 1 || true)
          if [ -n "$found" ]; then
            mv "$found" "$WRAPPER_PATH"
            chmod +x "$WRAPPER_PATH" || true
            rm -rf "$tmpdir"
            echo "Extracted helper to $WRAPPER_PATH"
            exit 0
          else
            echo "Downloaded tarball but couldn't find executable inside" >&2
            rm -rf "$tmpdir"
          fi
        else
          exit 0
        fi
      fi
    done
  done

  # 3) As a fallback, try to build a helper from source (librespot or spotifyd) if cargo is available
  if [ ! -f "$WRAPPER_PATH" ]; then
    if command -v cargo >/dev/null 2>&1; then
      echo "No prebuilt helper found; attempting to build librespot from source (requires Rust toolchain)..."
      tmpdir=$(mktemp -d)
      echo "Cloning librespot into $tmpdir"
      if git clone --depth 1 https://github.com/librespot-org/librespot.git "$tmpdir/librespot"; then
        (cd "$tmpdir/librespot" && cargo build --release) || true
        if [ -f "$tmpdir/librespot/target/release/librespot" ]; then
          mv "$tmpdir/librespot/target/release/librespot" "$WRAPPER_PATH"
          chmod +x "$WRAPPER_PATH" || true
          echo "Built librespot and installed wrapper to $WRAPPER_PATH"
          rm -rf "$tmpdir"
          exit 0
        else
          echo "librespot build failed or binary not found; attempting spotifyd fallback..." >&2
        fi
      else
        echo "Failed to clone librespot" >&2
      fi

      # Try building our bundled wrapper (tools/librespot-wrapper)
      echo "Attempting to build the in-repo librespot-wrapper helper..."
      if [ -d "tools/librespot-wrapper" ]; then
        if cargo build --manifest-path tools/librespot-wrapper/Cargo.toml --release; then
          mkdir -p "$BIN_DIR"
          cp target/release/librespot-wrapper "$WRAPPER_PATH" || true
          chmod +x "$WRAPPER_PATH" || true
          echo "Built and installed tools/librespot-wrapper to $WRAPPER_PATH"
          rm -rf "$tmpdir"
          exit 0
        else
          echo "Failed to build tools/librespot-wrapper" >&2
        fi
      fi
      # Try spotifyd
      echo "Attempting to build spotifyd from source..."
      if git clone --depth 1 https://github.com/Spotifyd/spotifyd.git "$tmpdir/spotifyd"; then
        (cd "$tmpdir/spotifyd" && cargo build --release) || true
        if [ -f "$tmpdir/spotifyd/target/release/spotifyd" ]; then
          mv "$tmpdir/spotifyd/target/release/spotifyd" "$WRAPPER_PATH"
          chmod +x "$WRAPPER_PATH" || true
          echo "Built spotifyd and installed wrapper to $WRAPPER_PATH"
          rm -rf "$tmpdir"
          exit 0
        else
          echo "spotifyd build failed or binary not found" >&2
        fi
      else
        echo "Failed to clone spotifyd" >&2
      fi

      rm -rf "$tmpdir"
    fi

    # 4) If all else fails, write an example wrapper
    echo "No prebuilt helper downloaded and build-from-source failed. Writing example wrapper to ${WRAPPER_PATH}.example"
    cp .bin/librespot-wrapper.example "$WRAPPER_PATH.example" || true
    exit 0
  fi
else
  echo "$WRAPPER_PATH already exists (use FORCE=1 to re-download)."
fi

exit 0
