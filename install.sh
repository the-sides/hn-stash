#!/bin/sh
# Build HN Stash, install it to ~/Applications, and start it at login.
set -eu
cd "$(dirname "$0")"

APP="$HOME/Applications/HN Stash.app"
LABEL="dev.hnstash.app"
AGENT="$HOME/Library/LaunchAgents/$LABEL.plist"

cargo build --release
strip target/release/hn-stash

mkdir -p "$APP/Contents/MacOS"
cp packaging/Info.plist "$APP/Contents/Info.plist"
cp target/release/hn-stash "$APP/Contents/MacOS/hn-stash"
codesign --force --sign - "$APP" >/dev/null 2>&1 || true

mkdir -p "$(dirname "$AGENT")"
cat > "$AGENT" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>$LABEL</string>
  <key>ProgramArguments</key><array><string>$APP/Contents/MacOS/hn-stash</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
  <key>ProcessType</key><string>Interactive</string>
</dict>
</plist>
PLIST

launchctl bootout "gui/$(id -u)/$LABEL" 2>/dev/null || true
launchctl bootstrap "gui/$(id -u)" "$AGENT"
echo "Installed: $APP (starts at login)"
