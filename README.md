# HN Stash

A tiny macOS menu bar app (Rust, native AppKit) for stashing Hacker News stories to read later.

- Click the newspaper icon: the current top 15 stories are fetched.
- Click a story: it's saved to the **Saved** submenu (checkmark shows it's saved; click again to unsave).
- Option-click a story: open it now (also recorded in History).
- In **Saved**, click a story to open it and remove it from the list. Option-click opens the HN comments.

Saved stories and history live in `~/Library/Application Support/hn-stash/` (`saved.json`, `history.json`).

## Install

    ./install.sh     # builds, installs to ~/Applications, starts at login
    ./uninstall.sh
