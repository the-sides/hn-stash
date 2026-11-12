//! GitHub-backed, one-way backup. Credentials stay in GitHub CLI's credential store.
use crate::Story;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const DESCRIPTION: &str = "HN Stash — saved stories and reading history";
const DATA_FILE: &str = "hn-stash.json";

/// Debounce edits, serialize uploads, and retry independently of menu activity.
/// A periodic check also repairs a cloud copy that was changed externally.
pub struct SyncSchedule {
    due: Instant,
    dirty: bool,
    running: bool,
    failures: u32,
}

impl Default for SyncSchedule {
    fn default() -> Self {
        Self::new(Instant::now())
    }
}

impl SyncSchedule {
    pub fn new(now: Instant) -> Self {
        Self {
            due: now,
            dirty: true,
            running: false,
            failures: 0,
        }
    }

    pub fn changed(&mut self, now: Instant) {
        self.dirty = true;
        // Keep an offline retry deadline instead of retrying after every click.
        if self.failures == 0 {
            self.due = now + Duration::from_secs(2);
        }
    }

    pub fn begin(&mut self, now: Instant, connect: bool) -> bool {
        if self.running || (!connect && now < self.due) {
            return false;
        }
        self.running = true;
        self.dirty = false;
        true
    }

    pub fn finish(&mut self, now: Instant, success: bool) {
        self.running = false;
        if success {
            self.failures = 0;
            if !self.dirty {
                self.due = now + Duration::from_secs(300);
            }
        } else {
            self.dirty = true;
            self.failures = (self.failures + 1).min(7);
            self.due = now + Duration::from_secs((5 * (1 << (self.failures - 1))).min(300));
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Connection {
    pub login: String,
    pub gist_id: String,
}

impl Connection {
    pub fn load() -> Option<Self> {
        serde_json::from_slice(&std::fs::read(crate::data_path("github.json")).ok()?).ok()
    }
    pub fn save(&self) -> Result<(), String> {
        crate::write_json("github.json", self)
    }
    pub fn url(&self) -> String {
        format!("https://gist.github.com/{}", self.gist_id)
    }
}

fn gh() -> Result<Command, String> {
    // LaunchAgents do not inherit the interactive shell's Homebrew PATH.
    for path in ["/opt/homebrew/bin/gh", "/usr/local/bin/gh"] {
        if std::path::Path::new(path).exists() {
            return Ok(Command::new(path));
        }
    }
    if Command::new("gh").arg("--version").output().is_ok() {
        Ok(Command::new("gh"))
    } else {
        Err("Install GitHub CLI (brew install gh), then connect again".into())
    }
}

fn api(method: &str, endpoint: &str, body: Option<Value>) -> Result<Value, String> {
    let mut command = gh()?;
    command.args([
        "api",
        "--hostname",
        "github.com",
        "--method",
        method,
        endpoint,
    ]);
    command
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_HTTP_TIMEOUT", "20");
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    if body.is_some() {
        command.args(["--input", "-"]).stdin(Stdio::piped());
    }
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    if let Some(body) = body {
        let bytes = serde_json::to_vec(&body).map_err(|e| e.to_string())?;
        child
            .stdin
            .take()
            .ok_or("No GitHub input pipe")?
            .write_all(&bytes)
            .map_err(|e| e.to_string())?;
    }
    let stdout = child.stdout.take().ok_or("Missing GitHub output pipe")?;
    let stderr = child.stderr.take().ok_or("Missing GitHub error pipe")?;
    let output = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(16 * 1024 * 1024)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let errors = std::thread::spawn(move || {
        // Drain diagnostics without retaining or exposing credential details.
        std::io::copy(&mut { stderr }, &mut std::io::sink())
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            result => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(match result {
                    Err(error) => error.to_string(),
                    _ => "GitHub request timed out; will retry automatically".into(),
                });
            }
        }
    };
    let bytes = output
        .join()
        .map_err(|_| "GitHub output reader failed")?
        .map_err(|e| e.to_string())?;
    let _ = errors.join();
    if !status?.success() {
        // Do not expose CLI diagnostics (which may contain account or auth details).
        return Err("GitHub request failed. Sign in with gist permission, then retry".into());
    }
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

pub fn account() -> Result<String, String> {
    api("GET", "user", None)?["login"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "GitHub did not return an account".into())
}

fn markdown(saved: &[Story], history: &[Story]) -> String {
    let mut text = String::from(
        "# HN Stash\n\nYour Mac backs up this reading list automatically. Open this Gist on any device to read.\n\nSecret Gists are unlisted, not private: anyone with the link can read them.\n",
    );
    for (heading, stories) in [("Saved", saved), ("History", history)] {
        text.push_str(&format!("\n## {heading}\n\n"));
        if stories.is_empty() {
            text.push_str("Nothing here yet.\n");
        }
        for story in stories {
            // HTML-escape the title and encode URL delimiters before Markdown rendering.
            let title = story
                .title
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
                .replace('[', "\\[")
                .replace(']', "\\]")
                .replace(['\n', '\r'], " ");
            let url = if story.url.starts_with("https://") || story.url.starts_with("http://") {
                story.url.replace(['<', '>', '\n', '\r', ' '], "")
            } else {
                story.discussion_url()
            };
            text.push_str(&format!(
                "- [{title}](<{url}>) · [comments]({})\n",
                story.discussion_url()
            ));
        }
    }
    text
}

pub fn backup(
    connection: Option<Connection>,
    saved: &[Story],
    history: &[Story],
) -> Result<Connection, String> {
    let login = account()?;
    if connection.as_ref().is_some_and(|c| c.login != login) {
        return Err("GitHub account changed. Disconnect before connecting the new account".into());
    }
    let mut connection = connection.unwrap_or(Connection {
        login,
        gist_id: String::new(),
    });
    if connection.gist_id.is_empty() {
        // Discover existing backups so reconnecting does not create duplicates.
        for page in 1..=10 {
            let list = api("GET", &format!("gists?per_page=100&page={page}"), None)?;
            let gists = list.as_array().ok_or("Invalid Gist list")?;
            if let Some(gist) = gists
                .iter()
                .find(|g| g["description"] == DESCRIPTION && g["files"][DATA_FILE].is_object())
            {
                connection.gist_id = gist["id"].as_str().ok_or("Missing Gist ID")?.to_owned();
                break;
            }
            if gists.len() < 100 {
                break;
            }
        }
    }
    let files = json!({
        DATA_FILE: {"content": serde_json::to_string_pretty(&json!({"version": 1, "saved": saved, "history": history})).map_err(|e| e.to_string())?},
        "README.md": {"content": markdown(saved, history)}
    });
    if connection.gist_id.is_empty() {
        let gist = api(
            "POST",
            "gists",
            Some(json!({"description": DESCRIPTION, "public": false, "files": files})),
        )?;
        connection.gist_id = gist["id"].as_str().ok_or("Missing Gist ID")?.to_owned();
        // Save the ID immediately so a later failure cannot create a duplicate.
        connection.save()?;
    } else {
        let gist = api("GET", &format!("gists/{}", connection.gist_id), None)?;
        if gist["owner"]["login"] != connection.login || gist["public"] != false {
            return Err("Backup must be a secret Gist owned by your connected account".into());
        }
        if gist["files"][DATA_FILE]["content"] != files[DATA_FILE]["content"]
            || gist["files"]["README.md"]["content"] != files["README.md"]["content"]
        {
            api(
                "PATCH",
                &format!("gists/{}", connection.gist_id),
                Some(json!({"files": files})),
            )?;
        }
    }
    connection.save()?;
    Ok(connection)
}

pub fn sign_in() -> Result<(), String> {
    gh()?;
    // GitHub CLI owns the OAuth browser/device flow and credential storage.
    let script = "tell application \"Terminal\"\nactivate\ndo script \"/usr/bin/env PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin gh auth login --hostname github.com --git-protocol ssh --web --scopes gist\"\nend tell";
    let status = Command::new("osascript")
        .args(["-e", script])
        .status()
        .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err("Could not open GitHub sign-in in Terminal".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn multiple_clicks_upload_only_after_the_last_edit_settles() {
        let now = Instant::now();
        let mut sync = SyncSchedule::new(now);
        sync.changed(now);
        sync.changed(now + Duration::from_secs(1));
        assert!(!sync.begin(now + Duration::from_secs(2), false));
        assert!(sync.begin(now + Duration::from_secs(3), false));
        assert!(!sync.begin(now + Duration::from_secs(4), false));
    }

    #[test]
    fn edits_during_upload_are_sent_after_completion() {
        let now = Instant::now();
        let mut sync = SyncSchedule::new(now);
        assert!(sync.begin(now, false));
        sync.changed(now + Duration::from_secs(1));
        sync.finish(now + Duration::from_secs(4), true);
        assert!(sync.begin(now + Duration::from_secs(4), false));
    }

    #[test]
    fn offline_retries_without_menu_activity_and_caps_backoff() {
        let mut now = Instant::now();
        let mut sync = SyncSchedule::new(now);
        for delay in [5, 10, 20, 40, 80, 160, 300, 300] {
            assert!(sync.begin(now, false));
            sync.finish(now, false);
            sync.changed(now); // Editing offline must preserve the backoff.
            assert!(!sync.begin(now + Duration::from_secs(delay - 1), false));
            now += Duration::from_secs(delay);
        }
        assert!(sync.begin(now, false));
        sync.finish(now, true);
        sync.changed(now);
        assert!(sync.begin(now + Duration::from_secs(2), false));
    }

    #[test]
    fn periodic_check_runs_without_editing_or_opening_the_menu() {
        let now = Instant::now();
        let mut sync = SyncSchedule::new(now);
        assert!(sync.begin(now, false));
        sync.finish(now, true);
        assert!(!sync.begin(now + Duration::from_secs(299), false));
        assert!(sync.begin(now + Duration::from_secs(300), false));
    }

    #[test]
    fn readable_backup_escapes_untrusted_titles_and_urls() {
        let story = Story {
            id: 1,
            title: "<script>[click]\nnext".into(),
            url: "javascript:alert(1)".into(),
            score: 0,
            comments: 0,
        };
        let output = markdown(&[story], &[]);
        assert!(output.contains("&lt;script&gt;\\[click\\] next"));
        assert!(!output.contains("javascript:"));
        assert!(output.contains("https://news.ycombinator.com/item?id=1"));
    }
}
