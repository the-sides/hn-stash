#!/bin/sh
set -u
LABEL="dev.hnstash.app"
launchctl bootout "gui/$(id -u)/$LABEL" 2>/dev/null
rm -f "$HOME/Library/LaunchAgents/$LABEL.plist"
rm -rf "$HOME/Applications/HN Stash.app"
echo "Removed. Saved stories remain in ~/Library/Application Support/hn-stash/"
