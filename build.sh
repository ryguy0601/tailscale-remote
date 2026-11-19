#!/usr/bin/env bash
set -e

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"
cd "$DIR"

echo "=== Building Tailscale Remote ==="
cargo build --release

echo ""
echo "Build complete! Binary available at:"
echo " - target/release/tailscale-remote"
