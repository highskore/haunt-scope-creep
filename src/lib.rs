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

#[derive(Default)]
struct Live {
    settings: Settings,
    scopes: BTreeMap<Key, Scope>,
    /// Each tab's latest session with a plan.
    tabs: BTreeMap<String, Key>,
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

/// What one event does to the scopes; returns the tab to redraw.
pub fn apply(scopes: &mut BTreeMap<Key, Scope>, tabs: &mut BTreeMap<String, Key>, e: &AgentEvent) -> Option<String> {
    if !e.verified {
        return None;
    }
    let (surface, session, cwd) = (e.surface.as_deref()?, e.session_id.clone()?, e.cwd.as_deref()?);
    let own: Key = (session.clone(), e.agent_id.clone());
    match e.kind.as_str() {
        "plan_exit" => {
            scopes.entry(own.clone()).or_default().planned(cwd, e.summary.as_deref()?);
            tabs.insert(surface.into(), own);
        }
        "task_created" => {
            scopes.entry(own).or_default().tasked(cwd, e.summary.as_deref()?);
            return None;
        }
        "file_edited" if e.tool.as_deref().is_some_and(|t| EDITS.contains(&t)) => {
            let path = e.file_path.as_deref()?;
            // A subagent without a plan of its own works on its session's.
            let key = [own, (session, None)].into_iter().find(|k| scopes.get(k).is_some_and(|s| !s.plan.is_empty()))?;
            if !scopes.get_mut(&key)?.touch(path) {
                return None;
            }
            tabs.insert(surface.into(), key);
        }
        _ => return None,
    }
    Some(surface.into())
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
    let mut out = vec![text("The plan names")];
    for n in s.named() {
        out.push(row(if n.folder { format!("{}/", n.path) } else { n.path }, None, PTone::Text));
    }
    out.push(node(Node::Divider));
    out.push(text("It touched"));
    if s.touched.is_empty() {
        out.push(row("nothing yet".into(), None, PTone::Faint));
    }
    for t in &s.touched {
        out.push(if t.inside { row(t.shown.clone(), None, PTone::Text) } else { row(t.shown.clone(), Some("outside plan"), PTone::Warn) });
    }
    out
}

fn show(s: &Scope) {
    if let Err(e) = panel::show(PANEL, Placement::Right, &format!("Scope · {}", name_of(&s.cwd)), &nodes(s)) {
        log(LogLevel::Warn, &format!("scope-creep: not shown: {e}"));
    }
}

fn redraw(l: &Live, surface: &str) {
    let s = l.tabs.get(surface).and_then(|k| l.scopes.get(k));
    if l.settings.row
        && let Err(e) = rows::decorate(surface, s.and_then(decoration).as_ref())
    {
        log(LogLevel::Debug, &format!("scope-creep: row not marked: {e}"));
    }
    if l.open.as_deref() == Some(surface)
        && let Some(s) = s
    {
        show(s);
    }
}

fn open(l: &mut Live, surface: &str) {
    match l.tabs.get(surface).and_then(|k| l.scopes.get(k)) {
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
            let tabs: Vec<String> = l.tabs.keys().cloned().collect();
            for t in tabs {
                redraw(l, &t);
            }
        });
    }

    fn on_event(event: AgentEvent) {
        with(|l| {
            if let Some(surface) = apply(&mut l.scopes, &mut l.tabs, &event) {
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
            l.tabs
                .iter()
                .filter_map(|(surface, k)| Some((surface, l.scopes.get(k)?)))
                .filter(|(_, s)| name_of(&s.cwd).to_lowercase().contains(&q))
                .map(|(surface, s)| {
                    let out = s.outside().count();
                    let tail = if out > 0 { format!(" · {out} outside plan") } else { String::new() };
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
        let planned = with(|l| l.tabs.contains_key(&surface));
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
        let (mut scopes, mut tabs) = (BTreeMap::new(), BTreeMap::new());
        let mut go = |e: AgentEvent| apply(&mut scopes, &mut tabs, &e);
        assert_eq!(go(event("file_edited", None, None, Some("/w/shop/x.rs"), true)), None, "no plan yet");
        assert_eq!(go(event("plan_exit", None, Some("Fix `src/a.rs`"), None, false)), None, "a forged plan");
        assert_eq!(go(event("plan_exit", None, Some("Fix `src/a.rs`"), None, true)).as_deref(), Some("t"));
        go(event("file_edited", None, None, Some("/w/shop/src/a.rs"), true));
        go(event("file_edited", Some("sub-1"), None, Some("/w/shop/package.json"), true));
        assert_eq!(go(event("file_edited", None, None, Some("/w/shop/.env"), false)), None, "a forged edit");
        let s = &scopes[&tabs["t"]];
        assert_eq!(s.row(), Some(("1 outside plan".into(), "package.json".into())), "the subagent works on its session's plan");
        assert!(matches!(decoration(s).unwrap().label_tone, Tone::Warn));
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
