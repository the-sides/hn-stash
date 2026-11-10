//! HN Stash: a tiny macOS menu bar app.
//!
//! Click the icon to fetch the current Hacker News top stories. Click a story
//! to stash it in "Saved" for later (right-click or Option-click opens it right away).
//! Middle-click a story in any list to open its Hacker News comments.
//! Clicking a saved story opens it in the browser and moves it to "History".

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{AnyThread, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSControlStateValueOff, NSControlStateValueOn,
    NSEventModifierFlags, NSEventType, NSImage, NSMenu, NSMenuDelegate, NSMenuItem, NSStatusBar, NSStatusItem,
    NSVariableStatusItemLength, NSWorkspace,
};
use objc2_foundation::{MainThreadMarker, NSData, NSObject, NSSize, NSObjectProtocol, NSString, NSURL, ns_string};
use serde::{Deserialize, Serialize};

const STORY_COUNT: usize = 15;
const HISTORY_LIMIT: usize = 100;
const ICON_SVG: &[u8] = include_bytes!("../packaging/icon.svg");
const API: &str = "https://hacker-news.firebaseio.com/v0";

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Story {
    id: u64,
    title: String,
    url: String,
    #[serde(default)]
    score: u64,
    #[serde(default)]
    comments: u64,
}

impl Story {
    fn discussion_url(&self) -> String {
        format!("https://news.ycombinator.com/item?id={}", self.id)
    }
}

#[derive(Deserialize)]
struct Item {
    id: u64,
    title: Option<String>,
    url: Option<String>,
    score: Option<u64>,
    descendants: Option<u64>,
}

#[derive(Default)]
struct State {
    stories: Vec<Story>,
    saved: Vec<Story>,
    history: Vec<Story>,
    error: Option<String>,
    menu: Option<Retained<NSMenu>>,
    target: Option<Retained<Handler>>,
    _status_item: Option<Retained<NSStatusItem>>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

static FETCHING: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------- storage

fn data_path(name: &str) -> PathBuf {
    let home = std::env::var_os("HOME").unwrap_or_default();
    PathBuf::from(home).join("Library/Application Support/hn-stash").join(name)
}

fn load_list(name: &str) -> Vec<Story> {
    std::fs::read(data_path(name))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn store_list(name: &str, list: &[Story]) {
    let path = data_path(name);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_vec_pretty(list) {
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(tmp, path);
        }
    }
}

// ---------------------------------------------------------------- network

fn fetch_top() -> Result<Vec<Story>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .build()
        .into();
    let ids: Vec<u64> = agent
        .get(&format!("{API}/topstories.json"))
        .call()
        .and_then(|mut r| r.body_mut().read_json())
        .map_err(|e| e.to_string())?;

    let handles: Vec<_> = ids
        .into_iter()
        .take(STORY_COUNT)
        .map(|id| {
            let agent = agent.clone();
            std::thread::spawn(move || -> Option<Story> {
                let item: Item = agent
                    .get(&format!("{API}/item/{id}.json"))
                    .call()
                    .ok()?
                    .body_mut()
                    .read_json()
                    .ok()?;
                let mut story = Story {
                    id: item.id,
                    title: item.title?,
                    url: String::new(),
                    score: item.score.unwrap_or(0),
                    comments: item.descendants.unwrap_or(0),
                };
                story.url = item.url.unwrap_or_else(|| story.discussion_url());
                Some(story)
            })
        })
        .collect();

    let stories: Vec<Story> = handles
        .into_iter()
        .filter_map(|h| h.join().ok().flatten())
        .collect();
    if stories.is_empty() {
        Err("No stories returned".into())
    } else {
        Ok(stories)
    }
}

fn start_fetch() {
    if FETCHING.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| {
        let result = fetch_top();
        DispatchQueue::main().exec_async(move || {
            FETCHING.store(false, Ordering::SeqCst);
            STATE.with_borrow_mut(|s| match result {
                Ok(stories) => {
                    s.stories = stories;
                    s.error = None;
                }
                Err(e) => s.error = Some(e),
            });
            rebuild_menu();
        });
    });
}

// ---------------------------------------------------------------- menu

fn short(title: &str, max: usize) -> String {
    if title.chars().count() <= max {
        title.to_string()
    } else {
        let mut s: String = title.chars().take(max - 1).collect();
        s.push('…');
        s
    }
}

fn domain(url: &str) -> &str {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let host = rest.split('/').next().unwrap_or(rest);
    host.strip_prefix("www.").unwrap_or(host)
}

fn item(
    mtm: MainThreadMarker,
    title: &str,
    action: Option<Sel>,
    tag: isize,
    target: &Handler,
) -> Retained<NSMenuItem> {
    let it = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            action,
            ns_string!(""),
        )
    };
    if action.is_some() {
        let t: &AnyObject = target;
        unsafe { it.setTarget(Some(t)) };
    }
    it.setTag(tag);
    it
}

fn info(mtm: MainThreadMarker, title: &str, target: &Handler) -> Retained<NSMenuItem> {
    let it = item(mtm, title, None, 0, target);
    it.setEnabled(false);
    it
}

fn rebuild_menu() {
    let mtm = MainThreadMarker::new().expect("main thread");
    STATE.with_borrow(|s| {
        let (Some(menu), Some(target)) = (&s.menu, &s.target) else {
            return;
        };
        menu.removeAllItems();

        // Saved submenu
        let saved_title = if s.saved.is_empty() {
            "Saved".to_string()
        } else {
            format!("Saved ({})", s.saved.len())
        };
        let saved_item = item(mtm, &saved_title, None, 0, target);
        let sub = NSMenu::new(mtm);
        if s.saved.is_empty() {
            sub.addItem(&info(mtm, "Nothing saved yet", target));
        } else {
            for (i, st) in s.saved.iter().enumerate() {
                let it = item(mtm, &short(&st.title, 70), Some(sel!(openSaved:)), i as isize, target);
                it.setToolTip(Some(&NSString::from_str(&format!(
                    "{}\nClick to open (moves to History) · Middle-click or Option-click to open comments",
                    domain(&st.url)
                ))));
                sub.addItem(&it);
                let alt = item(
                    mtm,
                    &format!("Comments: {}", short(&st.title, 60)),
                    Some(sel!(openSavedComments:)),
                    i as isize,
                    target,
                );
                alt.setKeyEquivalentModifierMask(NSEventModifierFlags::Option);
                alt.setAlternate(true);
                sub.addItem(&alt);
            }
            sub.addItem(&NSMenuItem::separatorItem(mtm));
            sub.addItem(&item(mtm, "Open All", Some(sel!(openAllSaved:)), 0, target));
            sub.addItem(&item(mtm, "Clear All", Some(sel!(clearSaved:)), 0, target));
        }
        saved_item.setSubmenu(Some(&sub));
        menu.addItem(&saved_item);

        // History submenu
        let hist_item = item(mtm, "History", None, 0, target);
        let hsub = NSMenu::new(mtm);
        if s.history.is_empty() {
            hsub.addItem(&info(mtm, "Nothing read yet", target));
        } else {
            for (i, st) in s.history.iter().enumerate() {
                let it = item(mtm, &short(&st.title, 70), Some(sel!(openHistory:)), i as isize, target);
                it.setToolTip(Some(&NSString::from_str(&format!(
                    "{}\nClick to open again · Middle-click or Option-click to open comments",
                    domain(&st.url)
                ))));
                hsub.addItem(&it);
                let alt = item(
                    mtm,
                    &format!("Comments: {}", short(&st.title, 60)),
                    Some(sel!(openHistoryComments:)),
                    i as isize,
                    target,
                );
                alt.setKeyEquivalentModifierMask(NSEventModifierFlags::Option);
                alt.setAlternate(true);
                hsub.addItem(&alt);
            }
            hsub.addItem(&NSMenuItem::separatorItem(mtm));
            hsub.addItem(&item(mtm, "Clear History", Some(sel!(clearHistory:)), 0, target));
        }
        hist_item.setSubmenu(Some(&hsub));
        menu.addItem(&hist_item);
        menu.addItem(&NSMenuItem::separatorItem(mtm));

        // Top stories
        if FETCHING.load(Ordering::SeqCst) {
            menu.addItem(&info(mtm, "Refreshing…", target));
        }
        if let Some(err) = &s.error {
            menu.addItem(&info(mtm, &format!("Couldn't load: {}", short(err, 50)), target));
        }
        for (i, st) in s.stories.iter().enumerate() {
            let it = item(mtm, &short(&st.title, 70), Some(sel!(toggleSave:)), i as isize, target);
            let is_saved = s.saved.iter().any(|x| x.id == st.id);
            it.setState(if is_saved { NSControlStateValueOn } else { NSControlStateValueOff });
            it.setToolTip(Some(&NSString::from_str(&format!(
                "{} · {} points · {} comments\nClick to save for later · Right-click or Option-click to open now · Middle-click for comments",
                domain(&st.url),
                st.score,
                st.comments
            ))));
            menu.addItem(&it);
            let alt = item(
                mtm,
                &format!("Open now: {}", short(&st.title, 60)),
                Some(sel!(openStory:)),
                i as isize,
                target,
            );
            alt.setKeyEquivalentModifierMask(NSEventModifierFlags::Option);
            alt.setAlternate(true);
            menu.addItem(&alt);
        }

        menu.addItem(&NSMenuItem::separatorItem(mtm));
        menu.addItem(&item(mtm, "Open Hacker News", Some(sel!(openHN:)), 0, target));
        menu.addItem(&item(mtm, "Quit HN Stash", Some(sel!(quit:)), 0, target));
    });
}

fn open_url(url: &str) {
    if let Some(u) = NSURL::URLWithString(&NSString::from_str(url)) {
        NSWorkspace::sharedWorkspace().openURL(&u);
    }
}

struct Lists<'a> {
    saved: &'a mut Vec<Story>,
    history: &'a mut Vec<Story>,
    stories: &'a [Story],
}

impl Lists<'_> {
    /// Open a story and record it at the top of History.
    fn read(&mut self, st: Story) {
        open_url(&st.url);
        self.history.retain(|x| x.id != st.id);
        self.history.insert(0, st);
        self.history.truncate(HISTORY_LIMIT);
    }
}

fn with_lists(f: impl FnOnce(&mut Lists)) {
    STATE.with_borrow_mut(|s| {
        let stories = std::mem::take(&mut s.stories);
        f(&mut Lists { saved: &mut s.saved, history: &mut s.history, stories: &stories });
        s.stories = stories;
        store_list("saved.json", &s.saved);
        store_list("history.json", &s.history);
    });
    rebuild_menu();
}

// ---------------------------------------------------------------- handler

fn is_middle_click(mtm: MainThreadMarker) -> bool {
    NSApplication::sharedApplication(mtm).currentEvent().is_some_and(|event| {
        matches!(event.r#type(), NSEventType::OtherMouseDown | NSEventType::OtherMouseUp)
            && event.buttonNumber() == 2
    })
}

define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    struct Handler;

    unsafe impl NSObjectProtocol for Handler {}

    unsafe impl NSMenuDelegate for Handler {
        #[unsafe(method(menuWillOpen:))]
        fn menu_will_open(&self, _menu: &NSMenu) {
            start_fetch();
            rebuild_menu();
        }
    }

    impl Handler {
        #[unsafe(method(toggleSave:))]
        fn toggle_save(&self, sender: &NSMenuItem) {
            if is_middle_click(self.mtm()) {
                self.open_story_comments(sel!(openStoryComments:), sender);
                return;
            }
            // Menu tracking delivers secondary clicks to the item's normal action.
            // Inspect that event here, rather than using an event monitor (which
            // does not receive events consumed by AppKit's menu tracking loop).
            let app = NSApplication::sharedApplication(self.mtm());
            if app.currentEvent().is_some_and(|event| {
                matches!(event.r#type(), NSEventType::RightMouseDown | NSEventType::RightMouseUp)
            }) {
                self.open_story(sel!(openStory:), sender);
                return;
            }
            let i = sender.tag() as usize;
            with_lists(|l| {
                if let Some(st) = l.stories.get(i) {
                    if let Some(pos) = l.saved.iter().position(|x| x.id == st.id) {
                        l.saved.remove(pos);
                    } else {
                        l.saved.push(st.clone());
                    }
                }
            });
        }

        #[unsafe(method(openStory:))]
        fn open_story(&self, sender: &NSMenuItem) {
            if is_middle_click(self.mtm()) {
                self.open_story_comments(sel!(openStoryComments:), sender);
                return;
            }
            let i = sender.tag() as usize;
            with_lists(|l| {
                if let Some(st) = l.stories.get(i).cloned() {
                    l.saved.retain(|x| x.id != st.id);
                    l.read(st);
                }
            });
        }

        #[unsafe(method(openStoryComments:))]
        fn open_story_comments(&self, sender: &NSMenuItem) {
            let i = sender.tag() as usize;
            let url = STATE.with_borrow(|s| s.stories.get(i).map(|x| x.discussion_url()));
            if let Some(url) = url {
                open_url(&url);
            }
        }

        #[unsafe(method(openSaved:))]
        fn open_saved(&self, sender: &NSMenuItem) {
            if is_middle_click(self.mtm()) {
                self.open_saved_comments(sel!(openSavedComments:), sender);
                return;
            }
            let i = sender.tag() as usize;
            with_lists(|l| {
                if i < l.saved.len() {
                    let st = l.saved.remove(i);
                    l.read(st);
                }
            });
        }

        #[unsafe(method(openSavedComments:))]
        fn open_saved_comments(&self, sender: &NSMenuItem) {
            let i = sender.tag() as usize;
            let url = STATE.with_borrow(|s| s.saved.get(i).map(|x| x.discussion_url()));
            if let Some(url) = url {
                open_url(&url);
            }
        }

        #[unsafe(method(openAllSaved:))]
        fn open_all_saved(&self, _sender: &NSMenuItem) {
            with_lists(|l| {
                for st in std::mem::take(l.saved) {
                    l.read(st);
                }
            });
        }

        #[unsafe(method(clearSaved:))]
        fn clear_saved(&self, _sender: &NSMenuItem) {
            with_lists(|l| l.saved.clear());
        }

        #[unsafe(method(openHistory:))]
        fn open_history(&self, sender: &NSMenuItem) {
            if is_middle_click(self.mtm()) {
                self.open_history_comments(sel!(openHistoryComments:), sender);
                return;
            }
            let i = sender.tag() as usize;
            with_lists(|l| {
                if let Some(st) = l.history.get(i).cloned() {
                    l.read(st);
                }
            });
        }

        #[unsafe(method(openHistoryComments:))]
        fn open_history_comments(&self, sender: &NSMenuItem) {
            let i = sender.tag() as usize;
            let url = STATE.with_borrow(|s| s.history.get(i).map(|x| x.discussion_url()));
            if let Some(url) = url {
                open_url(&url);
            }
        }

        #[unsafe(method(clearHistory:))]
        fn clear_history(&self, _sender: &NSMenuItem) {
            with_lists(|l| l.history.clear());
        }

        #[unsafe(method(openHN:))]
        fn open_hn(&self, _sender: &NSMenuItem) {
            open_url("https://news.ycombinator.com/");
        }

        #[unsafe(method(quit:))]
        fn quit(&self, _sender: &NSMenuItem) {
            NSApplication::sharedApplication(self.mtm()).terminate(None);
        }
    }
);

impl Handler {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

fn main() {
    let mtm = MainThreadMarker::new().expect("must run on main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let handler = Handler::new(mtm);
    let menu = NSMenu::new(mtm);
    menu.setDelegate(Some(ProtocolObject::from_ref(&*handler)));

    let status_item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
    if let Some(button) = status_item.button(mtm) {
        match NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(ICON_SVG)) {
            Some(img) => {
                img.setSize(NSSize::new(18.0, 18.0));
                button.setImage(Some(&img));
            }
            None => button.setTitle(ns_string!("Y")),
        }
    }
    status_item.setMenu(Some(&menu));

    STATE.with_borrow_mut(|s| {
        s.saved = load_list("saved.json");
        s.history = load_list("history.json");
        s.menu = Some(menu);
        s.target = Some(handler);
        s._status_item = Some(status_item);
    });
    rebuild_menu();
    start_fetch();

    app.run();
}
