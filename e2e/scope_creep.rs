//! End to end against a real haunt core. Copy into haunt's `cli/tests/` and run
//! `cargo test -p haunt-cli --features plugins --test scope_creep` (see README).
#![cfg(feature = "plugins")]

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use haunt_core::plugin_panel::{Panel, PanelNodeKind, PanelPlacement};
use haunt_core::plugin_settings::PluginConfig;
use haunt_core::plugin_store::{PluginSet, Store, attach};
use haunt_core::plugins::{HostRequest, PluginHost};
use haunt_core::{Core, SurfaceEnv};

const ID: &str = "dev.haunt.scope-creep";

fn repo() -> PathBuf {
    std::env::var_os("HAUNT_PLUGINS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../haunt-plugins"))
        .join("haunt-scope-creep")
}

fn until(what: &str, f: impl Fn() -> bool) {
    let t = Instant::now();
    while !f() {
        // Generous: events reach the plugin on another thread, slow on a busy machine.
        assert!(t.elapsed() < Duration::from_secs(20), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn send(sock: &mut UnixStream, env: &SurfaceEnv, nonce: bool, cwd: &str, rest: &str) {
    let nonce = if nonce { format!(r#","nonce":"{}""#, env.nonce) } else { String::new() };
    let line = format!(r#"{{"v":1,"agent":"claude","surface_id":"{}"{nonce},"session_id":"s1","cwd":"{cwd}",{rest}}}"#, env.surface_id);
    sock.write_all(format!("{line}\n").as_bytes()).unwrap();
}

fn edit(tool: &str, path: &Path) -> String {
    format!(r#""event":"file_edited","tool_name":"{tool}","tool_input":{{"file_path":"{}"}}"#, path.display())
}

fn plan(summary: &str) -> String {
    format!(r#""event":"plan_exit","summary":{}"#, serde_json::to_string(summary).unwrap())
}

fn shown(set: &Mutex<PluginSet>) -> Option<Panel> {
    set.lock().unwrap().take_requests().into_iter().rev().find_map(|r| match r {
        HostRequest::ShowPanel { plugin_id, panel, .. } if plugin_id == ID => Some(panel),
        _ => None,
    })
}

fn lines(p: &Panel) -> Vec<String> {
    p.nodes
        .iter()
        .filter_map(|n| match &n.kind {
            PanelNodeKind::Text { text, .. } => Some(text.clone()),
            PanelNodeKind::Row { label, right: Some(r), .. } => Some(format!("{label} · {r}")),
            PanelNodeKind::Row { label, .. } => Some(label.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn edits_outside_the_plan_mark_the_row() {
    let dir = repo();
    if !dir.join("haunt-plugin.toml").is_file() {
        eprintln!("skipped: haunt-scope-creep not found; set HAUNT_PLUGINS_DIR");
        return;
    }
    let tmp = PathBuf::from(format!("/tmp/haunt-fp-scope-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let (home, project) = (tmp.join("home"), tmp.join("shop"));
    std::fs::create_dir_all(&project).unwrap();
    let project = project.canonicalize().unwrap();
    let status = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["build", "--release", "--target", "wasm32-wasip2"])
        .current_dir(&dir)
        .env_remove("CARGO_TARGET_DIR")
        .status()
        .unwrap();
    assert!(status.success());
    let o = Command::new(env!("CARGO_BIN_EXE_haunt")).args(["plugin", "install", dir.to_str().unwrap(), "--yes"]).env("HAUNT_HOME", &home).output().unwrap();
    assert!(o.status.success(), "{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));

    let core = Core::new(&home);
    core.start().unwrap();
    let p = project.display().to_string();
    let (env, other) = (core.register_surface(&p).unwrap(), core.register_surface(&p).unwrap());
    let set = Store::open(&home).unwrap().load_all(&PluginHost::new().unwrap(), Some(&core.handle())).unwrap();
    assert!(set.errors().is_empty(), "{:?}", set.errors());
    let set = Arc::new(Mutex::new(set));
    attach(&core, &set);
    let decoration = |s: &str| core.snapshot().groups.iter().flat_map(|g| &g.rows).find(|r| r.surface_id == s).and_then(|r| r.decoration.clone());
    let label = |s: &str| decoration(s).and_then(|d| d.label);

    let mut sock = UnixStream::connect(&env.socket_path).unwrap();
    // Before any plan, and from a tab that can't prove it's the agent, nothing counts.
    send(&mut sock, &env, true, &p, &edit("Write", &project.join("early.rs")));
    send(&mut sock, &other, false, &p, &plan("Touch `src/forged.rs` only."));
    send(&mut sock, &other, false, &p, &edit("Edit", &project.join("README.md")));
    let summary = "Fix the session refresh.\n\n1. Update `src/auth/refresh.ts` to retry once.\n2. Add a test.\n3. Bump the timeout in src/config.ts and note it in docs/auth.md.";
    send(&mut sock, &env, true, &p, &plan(summary));
    send(&mut sock, &env, true, &p, &edit("Edit", &project.join("src/auth/refresh.ts")));
    send(&mut sock, &env, true, &p, &edit("Write", &project.join("src/auth/refresh.test.ts")));
    send(&mut sock, &env, true, &p, &edit("MultiEdit", &project.join("package.json")));
    send(&mut sock, &env, true, &p, &edit("Write", &project.join(".github/workflows/ci.yml")));
    until("the row", || label(&env.surface_id).as_deref() == Some("2 outside plan"));
    let d = decoration(&env.surface_id).unwrap();
    assert_eq!(d.suffix.as_deref(), Some("package.json, .github/"));
    assert_eq!(decoration(&other.surface_id), None, "a forged plan or edit marks nothing");

    let items = set.lock().unwrap().palette_items(Some("scope"), "");
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!((items[0].title.as_str(), items[0].subtitle.as_deref()), ("shop", Some("3 named · 4 touched · 2 outside plan")));
    set.lock().unwrap().take_requests();
    set.lock().unwrap().invoke(ID, &items[0].id, true).unwrap();
    let panel = shown(&set).expect("the scope panel");
    assert_eq!((panel.title.as_str(), panel.placement), ("Scope · shop", PanelPlacement::Right));
    assert_eq!(
        lines(&panel),
        [
            "The plan names",
            "src/auth/refresh.ts",
            "src/config.ts",
            "docs/auth.md",
            "It touched",
            "src/auth/refresh.ts",
            "src/auth/refresh.test.ts",
            "package.json · outside plan",
            ".github/workflows/ci.yml · outside plan",
        ]
    );

    // While open, the panel follows; a new plan replaces the old one.
    send(&mut sock, &env, true, &p, &plan("Now only `package.json`."));
    until("the new plan", || label(&env.surface_id).is_none());
    let panel = shown(&set).expect("the panel redrawn");
    assert_eq!(lines(&panel), ["The plan names", "package.json", "It touched", "nothing yet"]);
    set.lock().unwrap().panel_action(ID, "scope", panel.generation, "").unwrap();

    let menu = set.lock().unwrap().row_menu(&env.surface_id);
    assert_eq!(menu.iter().map(|m| m.title.as_str()).collect::<Vec<_>>(), ["Scope"]);
    assert!(set.lock().unwrap().row_menu(&other.surface_id).is_empty(), "no plan, no menu");

    send(&mut sock, &env, true, &p, &edit("Edit", &project.join("Makefile")));
    until("the row again", || label(&env.surface_id).as_deref() == Some("1 outside plan"));
    set.lock().unwrap().set_config(&PluginConfig::parse(&format!("[plugins.\"{ID}\"]\nrow = false\n")));
    until("the row cleared", || decoration(&env.surface_id).is_none());

    drop(core);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn a_huge_plan_or_a_hostile_file_name_doesnt_blind_it() {
    let dir = repo();
    if !dir.join("haunt-plugin.toml").is_file() {
        eprintln!("skipped: haunt-scope-creep not found; set HAUNT_PLUGINS_DIR");
        return;
    }
    let tmp = PathBuf::from(format!("/tmp/haunt-fp-scope-big-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let (home, project) = (tmp.join("home"), tmp.join("shop"));
    std::fs::create_dir_all(&project).unwrap();
    let project = project.canonicalize().unwrap();
    let status = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["build", "--release", "--target", "wasm32-wasip2"])
        .current_dir(&dir)
        .env_remove("CARGO_TARGET_DIR")
        .status()
        .unwrap();
    assert!(status.success());
    let o = Command::new(env!("CARGO_BIN_EXE_haunt")).args(["plugin", "install", dir.to_str().unwrap(), "--yes"]).env("HAUNT_HOME", &home).output().unwrap();
    assert!(o.status.success(), "{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));

    let core = Core::new(&home);
    core.start().unwrap();
    let p = project.display().to_string();
    let (env, other) = (core.register_surface(&p).unwrap(), core.register_surface(&p).unwrap());
    let set = Store::open(&home).unwrap().load_all(&PluginHost::new().unwrap(), Some(&core.handle())).unwrap();
    assert!(set.errors().is_empty(), "{:?}", set.errors());
    let set = Arc::new(Mutex::new(set));
    attach(&core, &set);
    let decoration = |s: &str| core.snapshot().groups.iter().flat_map(|g| &g.rows).find(|r| r.surface_id == s).and_then(|r| r.decoration.clone());

    // As big as a plan line gets in: 200 lines, every word a path, most of them repeats.
    let line = |i: usize| -> String { (0..19).map(|j| format!("`src/m{}/f{j}.rs` ", i % 40)).collect() };
    let huge: String = (0..200).map(line).collect::<Vec<_>>().join("\n");
    let huge = plan(&huge);
    // Room for the rest of the line under intake's 64 KiB.
    assert!(huge.len() > 60_000 && huge.len() < 64 * 1024 - 512, "{}", huge.len());
    let mut sock = UnixStream::connect(&env.socket_path).unwrap();
    send(&mut sock, &env, true, &p, &huge);
    send(&mut sock, &env, true, &p, &edit("Edit", &project.join("src/m1/f1.rs")));
    send(&mut sock, &env, true, &p, &edit("Write", &project.join("needs you/x.rs")));
    send(&mut sock, &env, true, &p, &edit("Write", &project.join("notes.md")));
    until("the row", || decoration(&env.surface_id).and_then(|d| d.label).as_deref() == Some("2 outside plan"));
    assert_eq!(decoration(&env.surface_id).unwrap().suffix.as_deref(), Some("1 file, notes.md"), "a status-like name is never sent");

    let mut sock2 = UnixStream::connect(&other.socket_path).unwrap();
    let mut send2 = |rest: &str| {
        let line = format!(r#"{{"v":1,"agent":"claude","surface_id":"{}","nonce":"{}","session_id":"s2","cwd":"{p}",{rest}}}"#, other.surface_id, other.nonce);
        sock2.write_all(format!("{line}\n").as_bytes()).unwrap();
    };
    send2(&plan("Fix `src/a.rs`."));
    send2(&edit("Edit", &project.join("b.rs")));
    until("the other tab", || decoration(&other.surface_id).and_then(|d| d.label).as_deref() == Some("1 outside plan"));

    drop(core);
    let _ = std::fs::remove_dir_all(&tmp);
}
