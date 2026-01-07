#!/bin/bash
# Quick launcher for the voice client tool

cd "$(dirname "$0")/../tools/voice-client" || exit 1

if [ "$1" == "--release" ]; then
    echo "Building in release mode..."
    cargo build --release
    echo ""
    echo "Starting voice client..."
    ./target/release/voice-client
else
    echo "Starting voice client (dev mode)..."
    cargo run
fi
