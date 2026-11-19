#!/usr/bin/env bash
# Unified launcher for Tailscale Remote (builds if needed, supports autostart).
set -e

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"
cd "$DIR"

if [ "${1:-}" = "--install-startup" ]; then
    AUTOSTART_DIR="$HOME/.config/autostart"
    mkdir -p "$AUTOSTART_DIR"
    cat <<EOF > "$AUTOSTART_DIR/tailscale-remote.desktop"
[Desktop Entry]
Type=Application
Name=Tailscale Remote
Exec="$DIR/run-remote.sh" --no-browser
Hidden=false
NoDisplay=false
X-GNOME-Autostart-enabled=true
Comment=Tailscale Phone Trackpad, Keyboard & Media Remote
EOF
    echo "[OK] Registered Tailscale Remote in ~/.config/autostart/tailscale-remote.desktop"
    exit 0
fi

if [ "${1:-}" = "--uninstall-startup" ]; then
    rm -f "$HOME/.config/autostart/tailscale-remote.desktop"
    echo "[OK] Removed Tailscale Remote from autostart."
    exit 0
fi

BIN="$DIR/target/release/tailscale-remote"

NEEDS_BUILD=false
if [ ! -x "$BIN" ]; then
    NEEDS_BUILD=true
else
    LATEST_SRC=$(find "$DIR/src" "$DIR/static" "$DIR/Cargo.toml" -type f -printf '%T@\n' 2>/dev/null | sort -n | tail -n 1 | cut -d. -f1)
    BIN_TS=$(stat -c %Y "$BIN" 2>/dev/null || stat -f %m "$BIN" 2>/dev/null || echo 0)
    if [ -n "$LATEST_SRC" ] && [ "$BIN_TS" -lt "$LATEST_SRC" ]; then
        echo "[*] Source updated since last build. Recompiling..."
        NEEDS_BUILD=true
    fi
fi

if [ "$NEEDS_BUILD" = true ]; then
    "$DIR/build.sh"
fi

exec "$BIN" "$@"
