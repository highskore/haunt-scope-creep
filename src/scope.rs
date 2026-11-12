//! What a plan names, and whether a file an agent touched falls inside it. Free of host calls.

/// A file or folder a plan names, as written there (relative to the repo or the agent's folder).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Named {
    pub path: String,
    pub folder: bool,
}

/// Words with a slash that are prose, not paths.
const PROSE: [&str; 12] = ["and/or", "either/or", "input/output", "read/write", "client/server", "true/false", "yes/no", "on/off", "i/o", "n/a", "w/o", "pass/fail"];

/// The paths and folders `text` names: backticked names, words with a `/`, and words with a file
/// extension. URLs, absolute paths outside `cwd`, `~`, `..`, globs and Windows paths are left out.
pub fn extract(text: &str, cwd: &str) -> Vec<Named> {
    let mut out: Vec<Named> = Vec::new();
    for (i, span) in text.split('`').enumerate() {
        let code = i % 2 == 1;
        for word in span.split_whitespace() {
            if let Some(n) = named(word, cwd, code)
                && !out.contains(&n)
            {
                out.push(n);
            }
        }
    }
    out
}

fn named(word: &str, cwd: &str, code: bool) -> Option<Named> {
    let punct = |c: char| ",;:!?()[]{}\"'*<>".contains(c);
    let w = word.trim_start_matches(punct).trim_end_matches(|c| punct(c) || c == '.');
    if w.is_empty() || w.contains("://") || w.contains('\\') || w.starts_with('~') || w.contains(['$', '*', '{', '}', '|', '=', '@', '#']) {
        return None;
    }
    if w.len() > 2 && w.as_bytes()[1] == b':' {
        return None; // C:/…
    }
    let w = match w.strip_prefix('/') {
        Some(_) => w.strip_prefix(cwd.trim_end_matches('/'))?.strip_prefix('/')?,
        None => w.strip_prefix("./").unwrap_or(w),
    };
    let folder = w.ends_with('/');
    let path = w.trim_end_matches('/');
    if path.is_empty() || path.split('/').any(|s| s.is_empty() || s == "." || s == ".." || !s.chars().all(|c| c.is_alphanumeric() || "._-".contains(c))) {
        return None;
    }
    if !code && PROSE.contains(&path.to_lowercase().as_str()) {
        return None;
    }
    let last = path.rsplit('/').next().unwrap_or(path);
    let file = has_extension(last);
    match (folder, path.contains('/'), file) {
        (true, _, _) => Some(Named { path: path.into(), folder: true }),
        (false, _, true) => Some(Named { path: path.into(), folder: false }),
        (false, true, false) => Some(Named { path: path.into(), folder: true }),
        (false, false, false) => None,
    }
}

/// `main.rs`, `.env`, `Cargo.toml`; not `e.g`, `1.2` or `v2`.
fn has_extension(name: &str) -> bool {
    if let Some(rest) = name.strip_prefix('.') {
        return !rest.is_empty() && !rest.contains('.') && rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    }
    let Some((stem, ext)) = name.rsplit_once('.') else { return false };
    !stem.is_empty()
        && (1..=8).contains(&ext.len())
        && ext.chars().all(|c| c.is_ascii_alphanumeric())
        && ext.chars().any(|c| c.is_ascii_alphabetic())
        && !(stem.len() == 1 && ext.len() == 1)
        && !stem.chars().all(|c| c.is_ascii_digit() || c == '.')
}

/// Lockfiles that follow a manifest.
fn locks(manifest: &str) -> &'static [&'static str] {
    match manifest {
        "package.json" => &["package-lock.json", "npm-shrinkwrap.json", "yarn.lock", "pnpm-lock.yaml", "bun.lockb", "bun.lock"],
        "Cargo.toml" => &["Cargo.lock"],
        "pyproject.toml" => &["poetry.lock", "uv.lock", "pdm.lock"],
        "Pipfile" => &["Pipfile.lock"],
        "Gemfile" => &["Gemfile.lock"],
        "go.mod" => &["go.sum"],
        "composer.json" => &["composer.lock"],
        "mix.exs" => &["mix.lock"],
        _ => &[],
    }
}

/// Per plan, so a runaway agent can't grow the plugin's memory without end.
const MAX: usize = 256;

const TEST_DIRS: [&str; 4] = ["test", "tests", "__tests__", "spec"];

/// `foo.test.js`, `foo.spec.ts`, `foo_test.go`, `test_foo.py` -> `foo.js`, `foo.ts`, `foo.go`, `foo.py`.
fn untested(name: &str) -> String {
    let (stem, ext) = name.rsplit_once('.').unwrap_or((name, ""));
    let stem = stem.strip_suffix(".test").or_else(|| stem.strip_suffix(".spec")).or_else(|| stem.strip_suffix("_test")).or_else(|| stem.strip_prefix("test_")).unwrap_or(stem);
    if ext.is_empty() { stem.into() } else { format!("{stem}.{ext}") }
}

fn split(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}

/// `dir` (absolute) is `named` (a plan's relative folder), or ends in it.
fn dir_is(dir: &str, named: &str) -> bool {
    named.is_empty() || dir == named || dir.ends_with(&format!("/{named}"))
}

/// Whether `path` (absolute, as the file event gave it) is inside what `plan` names.
pub fn inside(path: &str, cwd: &str, plan: &[Named]) -> bool {
    let rel = path.strip_prefix(cwd.trim_end_matches('/')).and_then(|r| r.strip_prefix('/'));
    let (dir, name) = split(path);
    plan.iter().any(|n| {
        if n.folder {
            return rel.is_some_and(|r| r.starts_with(&format!("{}/", n.path))) || path.contains(&format!("/{}/", n.path));
        }
        if rel == Some(n.path.as_str()) || path.ends_with(&format!("/{}", n.path)) {
            return true;
        }
        let (ndir, nname) = split(&n.path);
        let at_root = rel.is_some_and(|r| !r.contains('/'));
        let near = if ndir.is_empty() { at_root } else { dir_is(dir, ndir) };
        let test_dir = |d: &str| d.split('/').any(|s| TEST_DIRS.contains(&s));
        let sibling = name != nname && untested(name) == untested(nname) && (dir_is(dir, ndir) || test_dir(dir) || test_dir(ndir));
        sibling || locks(nname).contains(&name) && (near || at_root)
    })
}

/// How an outside file shows on the row: its top folder (`.github/`) unless the plan names
/// something under that folder, else its path.
pub fn short(rel: &str, plan: &[Named]) -> String {
    match rel.split_once('/') {
        Some((top, _)) if !plan.iter().any(|n| n.path == top || n.path.starts_with(&format!("{top}/"))) => format!("{top}/"),
        _ => rel.into(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Touch {
    /// As the event gave it.
    pub path: String,
    /// Relative to the agent's folder when inside it.
    pub shown: String,
    pub inside: bool,
}

/// One agent's plan and what it touched since.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scope {
    pub cwd: String,
    pub plan: Vec<Named>,
    /// Paths its task subjects named, kept across plans.
    pub tasks: Vec<Named>,
    pub touched: Vec<Touch>,
}

impl Scope {
    /// A new plan replaces the old one and starts the touches over.
    pub fn planned(&mut self, cwd: &str, plan: &str) {
        self.cwd = cwd.into();
        self.plan = extract(plan, cwd);
        self.touched.clear();
    }

    pub fn tasked(&mut self, cwd: &str, subject: &str) {
        for n in extract(subject, cwd) {
            if self.tasks.len() < MAX && !self.tasks.contains(&n) {
                self.tasks.push(n);
            }
        }
    }

    pub fn named(&self) -> Vec<Named> {
        self.plan.iter().chain(self.tasks.iter().filter(|t| !self.plan.contains(t))).cloned().collect()
    }

    /// Records a touch; true when it is new.
    pub fn touch(&mut self, path: &str) -> bool {
        if self.plan.is_empty() || self.touched.len() >= MAX || self.touched.iter().any(|t| t.path == path) {
            return false;
        }
        let shown = path.strip_prefix(self.cwd.trim_end_matches('/')).and_then(|r| r.strip_prefix('/')).unwrap_or(path).to_owned();
        let inside = inside(path, &self.cwd, &self.named());
        self.touched.push(Touch { path: path.into(), shown, inside });
        true
    }

    pub fn outside(&self) -> impl Iterator<Item = &Touch> {
        self.touched.iter().filter(|t| !t.inside)
    }

    /// The row: `2 outside plan` and `package.json, .github/`; nothing while all is inside.
    pub fn row(&self) -> Option<(String, String)> {
        let n = self.outside().count();
        if n == 0 {
            return None;
        }
        let named = self.named();
        let mut short: Vec<String> = Vec::new();
        for t in self.outside() {
            let s = short_of(&t.shown, &named);
            if !short.contains(&s) {
                short.push(s);
            }
        }
        Some((format!("{n} outside plan"), short.join(", ")))
    }
}

fn short_of(shown: &str, plan: &[Named]) -> String {
    if shown.starts_with('/') {
        // Outside the agent's folder: just the name.
        return shown.rsplit('/').next().unwrap_or(shown).into();
    }
    short(shown, plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAN: &str = "# Plan: refresh tokens\n\
\n\
The session expires mid-request. I'll fix the refresh flow in `src/auth/refresh.ts` and the\n\
middleware in src/middleware/session.ts, e.g. by retrying once.\n\
\n\
1. Update `src/auth/` to keep the old token for 5 s (see docs/auth.md).\n\
2. Add a test next to the client: `src/api/client.ts`.\n\
3. Bump `package.json` for jose 5.2.\n\
- Don't touch ../shared/config.ts or C:\\Users\\me\\x.ts or ~/notes.md.\n\
- Read https://example.com/rfc.html and /etc/hosts and/or the Node.js docs, i.e. nothing else.\n\
- Run `npm test` and `cargo test -p core`.";

    fn paths(n: &[Named]) -> Vec<String> {
        n.iter().map(|n| if n.folder { format!("{}/", n.path) } else { n.path.clone() }).collect()
    }

    #[test]
    fn a_real_plan_names_its_files_and_folders_and_nothing_else() {
        let n = extract(PLAN, "/w/shop");
        assert_eq!(
            paths(&n),
            ["src/auth/refresh.ts", "src/middleware/session.ts", "src/auth/", "docs/auth.md", "src/api/client.ts", "package.json", "Node.js"],
        );
    }

    #[test]
    fn absolute_paths_count_only_inside_the_agents_folder() {
        assert_eq!(paths(&extract("edit /w/shop/lib/x.rs and /w/other/y.rs", "/w/shop")), ["lib/x.rs"]);
        assert_eq!(paths(&extract("see `./scripts/` and .github/ and .env", "/w")), ["scripts/", ".github/", ".env"]);
        assert!(extract("version 1.2.3, v2, e.g. this", "/w").is_empty());
    }

    fn plan(text: &str) -> Scope {
        let mut s = Scope::default();
        s.planned("/w/shop", text);
        s
    }

    #[test]
    fn named_files_folders_and_suffixes_are_inside() {
        let s = plan(PLAN);
        let named = s.named();
        let at = |p: &str| inside(&format!("/w/shop/{p}"), "/w/shop", &named);
        assert!(at("src/auth/refresh.ts") && at("src/auth/token.ts") && at("src/auth/deep/x.ts"));
        assert!(at("package.json") && at("docs/auth.md"));
        assert!(!at("src/billing/x.ts") && !at(".github/workflows/ci.yml") && !at("docs/other.md"));
        assert!(inside("/w/shop/web/src/api/client.ts", "/w/shop/web", &named), "names relative to the repo root match from a subfolder");
    }

    #[test]
    fn test_siblings_and_lockfiles_follow_what_the_plan_names() {
        let named = extract("`src/api/client.ts`, `lib/parse.py`, `package.json`, `crates/core/Cargo.toml`", "/w");
        let at = |p: &str| inside(&format!("/w/{p}"), "/w", &named);
        assert!(at("src/api/client.test.ts") && at("src/api/client.spec.ts") && at("test/client.test.ts") && at("src/api/__tests__/client.test.ts"));
        assert!(at("lib/test_parse.py") && at("tests/test_parse.py"));
        assert!(!at("src/other/client.test.ts"), "a test of another client is not a sibling");
        assert!(at("package-lock.json") && at("yarn.lock") && at("crates/core/Cargo.lock") && at("Cargo.lock"), "the workspace lock sits at the root");
        assert!(!at("src/package-lock.json") && !at("Gemfile.lock"));
        let tests = extract("`src/foo.test.js`", "/w");
        assert!(inside("/w/src/foo.js", "/w", &tests), "and the other way round");
    }

    #[test]
    fn a_new_plan_replaces_the_old_one_and_starts_over() {
        let mut s = plan("Change `src/a.rs`.");
        assert!(s.touch("/w/shop/src/b.rs"));
        assert!(!s.touch("/w/shop/src/b.rs"), "once per file");
        assert_eq!(s.row(), Some(("1 outside plan".into(), "src/b.rs".into())), "src/ is in the plan, so the full path");
        s.planned("/w/shop", "Now `src/b.rs` and `src/c.rs`.");
        assert_eq!((paths(&s.plan), s.touched.len()), (vec!["src/b.rs".to_owned(), "src/c.rs".to_owned()], 0));
        s.touch("/w/shop/src/b.rs");
        assert_eq!(s.row(), None, "inside the new plan");
    }

    #[test]
    fn the_row_groups_outside_files_by_top_folder() {
        let mut s = plan("Fix `src/auth/refresh.ts`.");
        for p in ["src/auth/refresh.ts", "package.json", ".github/workflows/ci.yml", ".github/dependabot.yml", "src/billing/x.ts", "/w/other/notes.md"] {
            s.touch(&format!("{}{p}", if p.starts_with('/') { "" } else { "/w/shop/" }));
        }
        assert_eq!(s.row(), Some(("5 outside plan".into(), "package.json, .github/, src/billing/x.ts, notes.md".into())));
    }

    #[test]
    fn task_subjects_widen_the_plan_but_no_plan_means_nothing() {
        let mut s = Scope::default();
        s.tasked("/w/shop", "Write `README.md`");
        assert!(!s.touch("/w/shop/x.rs"), "no plan, no touches");
        s.planned("/w/shop", "Fix `src/a.rs`.");
        s.touch("/w/shop/README.md");
        assert_eq!(s.row(), None);
    }
}
