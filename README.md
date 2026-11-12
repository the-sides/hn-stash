# HN Stash

A tiny macOS menu bar app (Rust, native AppKit) for stashing Hacker News stories to read later.

- Click the newspaper icon: the current top 15 stories are fetched.
- Click a story: save it to **Saved** (checkmark shows it's saved; click again to unsave).
- Story clicks keep the menu open so you can select multiple items. Click outside or press Escape to close it.
- Right-click or Option-click a top story: open it now (also recorded in History).
- Middle-click a story in the top stories, **Saved**, or **History**: open its Hacker News comments without changing Saved or History.
- In **Saved**, click a story to open it and remove it from the list. Option-click opens the HN comments.
- Links opened while the menu is visible open in the background so you can keep picking stories.
- Saved rows already opened show a “read” marker until you close the menu; the lists refresh when reopened.

Saved stories and history live in `~/Library/Application Support/hn-stash/` (`saved.json`, `history.json`).

## GitHub backup

1. Install GitHub CLI (`brew install gh`) if needed.
2. Choose **GitHub sign-in…** in the menu. GitHub CLI opens in Terminal and handles GitHub's browser/device authorization with the `gist` scope. If already signed in with that scope, skip this step.
3. Choose **Connect GitHub**. This creates (or reuses) a secret Gist in your GitHub account.
4. Choose **Open cloud stash** to read your Saved and History links. Bookmark that URL on your phone or another device.

The Gist contains a readable `README.md` and an `hn-stash.json` backup. **Secret Gists are unlisted, not private: anyone with the link can read them.**

This is a one-way backup of this Mac's lists, not an editable web app or multi-Mac sync. Changes made directly to the Gist are not imported. Reconnecting another Mac to the same Gist replaces the backup with that Mac's lists.

After the one-time connection, the cloud copy stays up to date automatically while the app is running. Changes upload in the background about two seconds after your last edit; edits made during an upload are sent next. There is no manual backup action or routine progress indicator. The app also checks the cloud copy on startup and every five minutes, without needing to open the menu. Identical content is not rewritten.

Local saving works offline. Failed requests time out and retry automatically after 5 seconds, increasing to a maximum five-minute interval. Pending changes are picked up from local storage on the next startup if the app quits before uploading. Connection errors are available under **GitHub**, alongside sign-in and disconnect controls; normal syncing stays invisible. GitHub history retains previous revisions, including previously removed stories.

Credentials stay in GitHub CLI's credential store; HN Stash stores only the account and Gist ID in `github.json` alongside the local lists. Switching GitHub accounts requires disconnecting first. **GitHub → Disconnect GitHub** stops uploads without deleting the Gist or signing out other tools. It is unavailable while a backup is in progress.

For a manual backup/connection from a terminal: `cargo run -- --connect`.

## Install

    ./install.sh     # builds, installs to ~/Applications, starts at login
    ./uninstall.sh

## Checks

    cargo test
    cargo clippy --all-targets -- -D warnings
