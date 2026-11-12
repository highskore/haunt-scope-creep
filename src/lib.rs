//! Scope creep: marks an agent's row when it edits files its own plan didn't name, and shows the
//! plan beside what it touched. Built only from the agent's plan, task subjects and file events.
// wit-bindgen's flattened `notify` import has 14 params.
#![allow(clippy::too_many_arguments)]

wit_bindgen::generate!({ path: "wit/haunt-plugin.wit", world: "plugin" });

pub mod scope;

use std::collections::BTreeMap;
use std::sync::Mutex;

use haunt::plugin::log::log;
use haunt::plugin::palette;
use haunt::plugin::panel::{self, Node, PanelNode, Placement, RowNode, TextNode, Tone as PTone};
use haunt::plugin::rows::{self, Decoration, Tone};
use haunt::plugin::types::LogLevel;
use scope::Scope;
use serde::Deserialize;

const MODE: &str = "scope";
const PANEL: &str = "scope";
/// The tools whose `file_path` is a file the agent wrote.
const EDITS: [&str; 4] = ["Edit", "Write", "MultiEdit", "NotebookEdit"];

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The row's "2 outside plan".
    pub row: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { row: true }
    }
}

/// A session, or one of its subagents.
type Key = (String, Option<String>);

/// Scopes kept. Agent ids come from the agent, so one tab can make many: these bound the plugin's
/// memory as a whole, well under Haunt's 64 MB.
const SCOPES: usize = 64;
/// Of them, scopes with tasks but no plan.
const PLANLESS: usize = 32;
/// Touches across all scopes.
const TOUCHES: usize = 4096;
/// Bytes of plan and task paths across all scopes; one scope holds at most `scope::PLAN_PATHS` +
/// `scope::TASK_PATHS` of it.
const PLAN_BYTES: usize = 64 * 1024;
/// Rows per panel list, well under Haunt's 512 nodes.
const ROWS: usize = 200;

#[derive(Default)]
pub struct Book {
    /// Each with when it was last active.
    scopes: BTreeMap<Key, (u64, Scope)>,
    /// Each tab's latest session with a plan.
    tabs: BTreeMap<String, Key>,
    seq: u64,
}

impl Book {
    fn get(&self, surface: &str) -> Option<&Scope> {
        self.tabs.get(surface).and_then(|k| self.scopes.get(k)).map(|(_, s)| s)
    }

    fn entry(&mut self, k: Key) -> &mut Scope {
        self.seq += 1;
        let e = self.scopes.entry(k).or_default();
        e.0 = self.seq;
        &mut e.1
    }

    /// Drops scopes until all fit, never `keep` (the one just changed): ones without a plan first,
    /// then ones no tab shows, then the rest, oldest first. A tab's scope, and its session's own plan
    /// when a subagent's is shown, only ever go for the count: one tab's flood can't push out another's.
    fn evict(&mut self, keep: &Key) -> Vec<String> {
        loop {
            let planless = self.scopes.values().filter(|(_, s)| s.plan.is_empty()).count();
            let touches: usize = self.scopes.values().map(|(_, s)| s.touched.len()).sum();
            let bytes: usize = self.scopes.values().map(|(_, s)| s.bytes).sum();
            let count = self.scopes.len() > SCOPES || planless > PLANLESS;
            if !count && touches <= TOUCHES && bytes <= PLAN_BYTES {
                break;
            }
            let shown = |k: &Key| self.tabs.values().any(|t| t == k || (t.0 == k.0 && k.1.is_none()));
            let Some(victim) = self
                .scopes
                .iter()
                .filter(|(k, _)| *k != keep && (count || !shown(k)))
                .min_by_key(|(k, (at, s))| ((!s.plan.is_empty(), shown(k)), *at))
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            self.scopes.remove(&victim);
        }
        let gone: Vec<String> = self.tabs.iter().filter(|(_, k)| !self.scopes.contains_key(*k)).map(|(t, _)| t.clone()).collect();
        self.tabs.retain(|_, k| self.scopes.contains_key(k));
        gone
    }
}

#[derive(Default)]
struct Live {
    settings: Settings,
    book: Book,
    /// The tab whose panel is open.
    open: Option<String>,
}

static LIVE: Mutex<Option<Live>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Live) -> R) -> R {
    f(LIVE.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Live::default))
}

/// `shop` for `/Users/me/code/shop`.
fn name_of(cwd: &str) -> &str {
    cwd.trim_end_matches('/').rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or("agent")
}

/// What one event does to the scopes; returns the tabs to redraw.
pub fn apply(b: &mut Book, e: &AgentEvent) -> Vec<String> {
    let (Some(surface), Some(session), Some(cwd)) = (e.surface.as_deref(), e.session_id.clone(), e.cwd.as_deref()) else { return Vec::new() };
    if !e.verified {
        return Vec::new();
    }
    let own: Key = (session.clone(), e.agent_id.clone());
    let changed = match (e.kind.as_str(), e.summary.as_deref()) {
        ("plan_exit", Some(summary)) => {
            // The tab moved on to a new session: the old one's plans are done.
            if let Some(old) = b.tabs.get(surface).filter(|k| k.0 != session).map(|k| k.0.clone()) {
                b.scopes.retain(|k, _| k.0 != old);
            }
            b.entry(own.clone()).planned(cwd, summary);
            b.tabs.insert(surface.into(), own.clone());
            own
        }
        ("task_created", Some(subject)) => {
            b.entry(own.clone()).tasked(cwd, subject);
            return b.evict(&own);
        }
        ("file_edited", _) if e.tool.as_deref().is_some_and(|t| EDITS.contains(&t)) => {
            let Some(path) = e.file_path.as_deref() else { return Vec::new() };
            // A subagent without a plan of its own works on its session's.
            let Some(key) = [own, (session, None)].into_iter().find(|k| b.scopes.get(k).is_some_and(|(_, s)| !s.plan.is_empty())) else {
                return Vec::new();
            };
            if !b.entry(key.clone()).touch(path) {
                return Vec::new();
            }
            b.tabs.insert(surface.into(), key.clone());
            key
        }
        _ => return Vec::new(),
    };
    let mut out = b.evict(&changed);
    out.push(surface.into());
    out
}

fn decoration(s: &Scope) -> Option<Decoration> {
    let (label, suffix) = s.row()?;
    Some(Decoration { label: Some(label), label_tone: Tone::Warn, suffix: Some(suffix), suffix_tone: Tone::Warn, scroll: false, bar: None, spark: None })
}

/// The panel: what the plan names, then what the agent touched, the outside ones in orange.
pub fn nodes(s: &Scope) -> Vec<PanelNode> {
    let node = |node| PanelNode { parent: None, node };
    let text = |text: &str| node(Node::Text(TextNode { text: text.into(), tone: PTone::Dim, mono: false, bold: true }));
    let row = |label: String, right: Option<&str>, tone| node(Node::Row(RowNode { label, sub: None, right: right.map(Into::into), tone }));
    let more = |n: usize| row(format!("+{n} more"), None, PTone::Faint);
    let mut out = vec![text("The plan names")];
    for n in s.named().iter().take(ROWS) {
        out.push(row(if n.folder { format!("{}/", n.path) } else { n.path.clone() }, None, PTone::Text));
    }
    if s.named().len() > ROWS {
        out.push(more(s.named().len() - ROWS));
    }
    out.push(node(Node::Divider));
    out.push(text("It touched"));
    if s.touched.is_empty() {
        out.push(row("nothing yet".into(), None, PTone::Faint));
    }
    // Too many to list: the outside ones first, as they're what matters.
    let mut touched: Vec<&scope::Touch> = s.touched.iter().collect();
    if touched.len() > ROWS {
        touched.sort_by_key(|t| t.inside);
    }
    for t in touched.iter().take(ROWS) {
        out.push(if t.inside { row(t.shown.clone(), None, PTone::Text) } else { row(t.shown.clone(), Some("outside plan"), PTone::Warn) });
    }
    let hidden = s.touched.len().saturating_sub(ROWS) + usize::from(s.inside_full) + usize::from(s.outside_full);
    if hidden > 0 {
        out.push(if s.inside_full || s.outside_full { row("more not counted".into(), None, PTone::Faint) } else { more(hidden) });
    }
    out
}

fn show(s: &Scope) {
    if let Err(e) = panel::show(PANEL, Placement::Right, &format!("Scope · {}", name_of(&s.cwd)), &nodes(s)) {
        log(LogLevel::Warn, &format!("scope-creep: not shown: {e}"));
    }
}

fn redraw(l: &Live, surface: &str) {
    let s = l.book.get(surface);
    if l.settings.row {
        let d = s.and_then(decoration);
        if let Err(e) = rows::decorate(surface, d.as_ref()) {
            log(LogLevel::Debug, &format!("scope-creep: row not marked: {e}"));
            // The count alone, rather than a row stuck on an older one.
            if let Some(d) = d.map(|d| Decoration { suffix: None, ..d })
                && let Err(e) = rows::decorate(surface, Some(&d))
            {
                log(LogLevel::Debug, &format!("scope-creep: count not shown: {e}"));
            }
        }
    }
    if l.open.as_deref() == Some(surface)
        && let Some(s) = s
    {
        show(s);
    }
}

fn open(l: &mut Live, surface: &str) {
    match l.book.get(surface) {
        Some(s) => {
            show(s);
            l.open = Some(surface.into());
        }
        None => log(LogLevel::Info, "scope-creep: that agent has no plan yet"),
    }
}

struct Plugin;

impl Guest for Plugin {
    fn init(json: String) {
        with(|l| l.settings = serde_json::from_str(&json).unwrap_or_default());
        palette::mode(MODE, "An agent's plan beside the files it touched");
    }

    fn on_settings_changed(json: String) {
        with(|l| {
            l.settings = serde_json::from_str(&json).unwrap_or_default();
            if !l.settings.row {
                rows::clear_all();
                return;
            }
            let tabs: Vec<String> = l.book.tabs.keys().cloned().collect();
            for t in tabs {
                redraw(l, &t);
            }
        });
    }

    fn on_event(event: AgentEvent) {
        with(|l| {
            for surface in apply(&mut l.book, &event) {
                redraw(l, &surface);
            }
        });
    }

    fn palette_items(mode: String, query: String) -> Vec<PaletteItem> {
        if mode != MODE {
            return Vec::new();
        }
        let q = query.trim().to_lowercase();
        with(|l| {
            l.book
                .tabs
                .keys()
                .filter_map(|surface| Some((surface, l.book.get(surface)?)))
                .filter(|(_, s)| name_of(&s.cwd).to_lowercase().contains(&q))
                .map(|(surface, s)| {
                    let tail = match (s.outside_n, s.outside_full) {
                        (0, _) => String::new(),
                        (_, true) => format!(" · {}+ outside plan", scope::MAX),
                        (n, false) => format!(" · {n} outside plan"),
                    };
                    let sub = format!("{} named · {} touched{tail}", s.named().len(), s.touched.len());
                    PaletteItem { id: format!("open:{surface}"), title: name_of(&s.cwd).into(), subtitle: Some(sub), at_ms: None, mark: None, surface: None }
                })
                .collect()
        })
    }

    fn on_action(action_id: String, payload: String) {
        with(|l| {
            if let Some(surface) = action_id.strip_prefix("open:") {
                open(l, surface);
            } else if action_id == "scope" {
                open(l, &payload);
            } else if action_id.is_empty() && payload == format!("panel:{PANEL}") {
                l.open = None;
            }
        });
    }

    fn row_menu(surface: String) -> Vec<MenuItem> {
        let planned = with(|l| l.book.get(&surface).is_some());
        planned.then(|| MenuItem { id: "scope".into(), title: "Scope".into(), checked: false, parent: None }).into_iter().collect()
    }

    fn on_row_moved(_surface: String, _group: String, _before: Option<String>) {}

    fn on_theme_changed() {}

    fn on_tick(_now_ms: u64) {}

    fn on_exec(_job: u64, _outcome: Result<haunt::plugin::exec::Output, String>) {}

    fn on_away_changed(_away: bool, _away_ms: u64) {}

    fn on_hover_hold(_held: bool, _surface: String) {}

    fn on_fs_tree(_job: u64, _outcome: Result<haunt::plugin::fs_list::TreeResult, String>) {}
}

export!(Plugin);

#[cfg(test)]
mod tests {
    use super::*;

    fn event(kind: &str, agent: Option<&str>, summary: Option<&str>, file: Option<&str>, verified: bool) -> AgentEvent {
        AgentEvent {
            kind: kind.into(),
            session_id: Some("s1".into()),
            agent_id: agent.map(Into::into),
            cwd: Some("/w/shop".into()),
            summary: summary.map(Into::into),
            verified,
            surface: Some("t".into()),
            tool: file.map(|_| "Edit".into()),
            command: None,
            file_path: file.map(Into::into),
            task_id: None,
            request_key: None,
        }
    }

    #[test]
    fn verified_plans_and_edits_mark_the_row() {
        let mut b = Book::default();
        let mut go = |e: AgentEvent| apply(&mut b, &e);
        assert!(go(event("file_edited", None, None, Some("/w/shop/x.rs"), true)).is_empty(), "no plan yet");
        assert!(go(event("plan_exit", None, Some("Fix `src/a.rs`"), None, false)).is_empty(), "a forged plan");
        assert!(go(event("plan_exit", None, None, None, true)).is_empty(), "a plan without its text");
        assert_eq!(go(event("plan_exit", None, Some("Fix `src/a.rs`"), None, true)), ["t"]);
        go(event("file_edited", None, None, Some("/w/shop/src/a.rs"), true));
        go(event("file_edited", Some("sub-1"), None, Some("/w/shop/package.json"), true));
        assert!(go(event("file_edited", None, None, Some("/w/shop/.env"), false)).is_empty(), "a forged edit");
        let s = b.get("t").unwrap();
        assert_eq!(s.row(), Some(("1 outside plan".into(), "package.json".into())), "the subagent works on its session's plan");
        assert!(matches!(decoration(s).unwrap().label_tone, Tone::Warn));
        assert_eq!(b.scopes.len(), 1, "no scope for the forged or empty plans");
    }

    #[test]
    fn a_new_session_in_a_tab_drops_the_old_ones_and_old_scopes_go() {
        let mut b = Book::default();
        apply(&mut b, &event("plan_exit", None, Some("`a.rs`"), None, true));
        apply(&mut b, &event("plan_exit", Some("sub"), Some("`b.rs`"), None, true));
        let mut next = event("plan_exit", None, Some("`c.rs`"), None, true);
        next.session_id = Some("s2".into());
        apply(&mut b, &next);
        assert_eq!(b.scopes.keys().map(|k| k.0.as_str()).collect::<Vec<_>>(), ["s2"]);
        for i in 0..SCOPES + 5 {
            let mut e = event("plan_exit", None, Some("`a.rs`"), None, true);
            (e.session_id, e.surface) = (Some(format!("x{i}")), Some(format!("t{i}")));
            let redraw = apply(&mut b, &e);
            assert_eq!(redraw.len(), if i >= SCOPES - 1 { 2 } else { 1 }, "the evicted tab is cleared too");
        }
        assert_eq!((b.scopes.len(), b.tabs.len()), (SCOPES, SCOPES));
        assert!(b.get("t").is_none() && b.get(&format!("t{}", SCOPES + 4)).is_some(), "the oldest went");
    }

    use std::alloc::{GlobalAlloc, Layout, System};
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

    /// Counts live heap bytes, so a test can bound the plugin's peak memory. Natively, so pointers
    /// are twice wasm's and the bound is on the safe side.
    struct Counting;
    static LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);
    static PEAK: AtomicUsize = AtomicUsize::new(0);

    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, l: Layout) -> *mut u8 {
            let p = unsafe { System.alloc(l) };
            if !p.is_null() {
                PEAK.fetch_max(LIVE_BYTES.fetch_add(l.size(), Relaxed) + l.size(), Relaxed);
            }
            p
        }
        unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
            unsafe { System.dealloc(p, l) };
            LIVE_BYTES.fetch_sub(l.size(), Relaxed);
        }
    }

    #[global_allocator]
    static ALLOC: Counting = Counting;

    fn at(kind: &str, session: &str, agent: Option<&str>, surface: &str, summary: Option<&str>, file: Option<&str>) -> AgentEvent {
        let mut e = event(kind, agent, summary, file, true);
        (e.session_id, e.surface) = (Some(session.into()), Some(surface.into()));
        e
    }

    #[test]
    fn a_flood_of_subagent_writes_stays_small_and_leaves_other_tabs_alone() {
        let mut b = Book::default();
        apply(&mut b, &at("plan_exit", "sa", None, "a", Some("Fix `src/a.rs`"), None));
        let base = LIVE_BYTES.load(Relaxed);
        PEAK.store(base, Relaxed);
        let long = "𝕏".repeat(1000);
        for i in 0..50 {
            let agent = format!("sub{i}");
            apply(&mut b, &at("plan_exit", "sb", Some(&agent), "b", Some("Only `src/b.rs`"), None));
            for j in 0..512 {
                apply(&mut b, &at("file_edited", "sb", Some(&agent), "b", None, Some(&format!("/w/shop/{long}{j}"))));
            }
        }
        let peak = PEAK.load(Relaxed) - base;
        assert!(peak < 4 << 20, "peak {peak} bytes, about 1.3 MB when written");
        assert!(b.scopes.values().map(|(_, s)| s.touched.len()).sum::<usize>() <= TOUCHES);
        assert_eq!(apply(&mut b, &at("file_edited", "sa", None, "a", None, Some("/w/shop/c.rs"))), ["a"]);
        assert_eq!(b.get("a").unwrap().row().unwrap().0, "1 outside plan");
    }

    #[test]
    fn one_tabs_big_plan_and_tasks_cant_push_out_another_tabs_plan() {
        let mut b = Book::default();
        apply(&mut b, &at("plan_exit", "sa", None, "a", Some("Fix `src/a.rs`"), None));
        let path = |i: usize| format!("`src/{}{i}.rs` ", "p".repeat(225));
        let big: String = (0..250).map(path).collect();
        assert!(big.len() > 59_000);
        apply(&mut b, &at("plan_exit", "sb", None, "b", Some(&big), None));
        for i in 0..70 {
            let subject: String = (0..4).map(|j| path(1000 + i * 4 + j)).collect();
            apply(&mut b, &at("task_created", "sb", None, "b", Some(&subject), None));
        }
        assert_eq!(apply(&mut b, &at("file_edited", "sa", None, "a", None, Some("/w/shop/c.rs"))), ["a"]);
        assert_eq!(b.get("a").unwrap().row().unwrap().0, "1 outside plan");
    }

    #[test]
    fn a_shown_subagent_keeps_its_sessions_plan() {
        let mut b = Book::default();
        apply(&mut b, &at("plan_exit", "sa", None, "a", Some("Fix `src/a.rs`"), None));
        apply(&mut b, &at("plan_exit", "sa", Some("sub"), "a", Some("Fix `src/b.rs`"), None));
        let path = |i: usize| format!("`src/{}{i}.rs` ", "p".repeat(220));
        for k in 0..8 {
            let plan: String = (0..70).map(|i| path(k * 100 + i)).collect();
            apply(&mut b, &at("plan_exit", "sb", Some(&format!("s{k}")), "b", Some(&plan), None));
        }
        assert!(b.scopes.contains_key(&("sa".to_owned(), None)), "tab a shows its subagent, and its session's plan stays");
        assert!(b.scopes.values().map(|(_, s)| s.bytes).sum::<usize>() <= PLAN_BYTES + scope::PLAN_PATHS);
    }

    #[test]
    fn task_only_scopes_go_before_a_plan() {
        let mut b = Book::default();
        apply(&mut b, &at("plan_exit", "sa", None, "a", Some("Fix `src/a.rs`"), None));
        for i in 0..70 {
            apply(&mut b, &at("task_created", "sb", Some(&format!("sub{i}")), "b", Some("Write `x.rs`"), None));
        }
        assert_eq!(b.scopes.values().filter(|(_, s)| s.plan.is_empty()).count(), PLANLESS);
        assert!(b.get("a").is_some_and(|s| !s.plan.is_empty()), "tab a's plan stays");
    }

    #[test]
    fn a_long_panel_is_cut_with_the_outside_files_first() {
        let mut s = Scope::default();
        let names: String = (1..scope::MAX).map(|i| format!("`lib/f{i}.rs` ")).collect();
        let names = format!("`src/` {names}");
        s.planned("/w", &names);
        for i in 0..scope::MAX + 3 {
            s.touch(&format!("/w/src/f{i}.rs"));
        }
        s.touch("/w/x.rs");
        let n = nodes(&s);
        assert!(n.len() < 512, "{}", n.len());
        let labels: Vec<String> = n.iter().filter_map(|n| if let Node::Row(r) = &n.node { Some(r.label.clone()) } else { None }).collect();
        assert!(labels.contains(&"+56 more".to_owned()) && labels.contains(&"x.rs".to_owned()) && labels.last().unwrap() == "more not counted", "{:?}", &labels[labels.len() - 3..]);
    }

    #[test]
    fn the_panel_lists_the_plan_then_the_touches() {
        let mut s = Scope::default();
        s.planned("/w/shop", "Fix `src/a.rs` in `src/`.");
        s.touch("/w/shop/src/a.rs");
        s.touch("/w/shop/package.json");
        let rows: Vec<(String, Option<String>)> = nodes(&s)
            .into_iter()
            .filter_map(|n| match n.node {
                Node::Row(r) => Some((r.label, r.right)),
                Node::Text(t) => Some((t.text, None)),
                _ => None,
            })
            .collect();
        let want: Vec<(String, Option<String>)> = vec![
            ("The plan names".into(), None),
            ("src/a.rs".into(), None),
            ("src/".into(), None),
            ("It touched".into(), None),
            ("src/a.rs".into(), None),
            ("package.json".into(), Some("outside plan".into())),
        ];
        assert_eq!(rows, want);
    }

    #[test]
    fn manifest_asks_for_what_it_uses_and_defaults_match() {
        let m = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/haunt-plugin.toml")).unwrap();
        let t: toml::Table = toml::from_str(&m).unwrap();
        let caps = t["capabilities"].as_table().unwrap();
        assert_eq!(caps.keys().collect::<Vec<_>>(), ["decorate", "event_details", "events", "panel"]);
        let defaults: serde_json::Map<String, serde_json::Value> =
            t["settings"].as_table().unwrap().iter().map(|(k, v)| (k.clone(), serde_json::to_value(&v["default"]).unwrap())).collect();
        assert_eq!(serde_json::from_value::<Settings>(serde_json::Value::Object(defaults)).unwrap(), Settings::default());
    }
}
