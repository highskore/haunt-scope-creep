//! What a plan names, and whether a file an agent touched falls inside it. Free of host calls.

use std::collections::HashSet;
use std::hash::{BuildHasher, BuildHasherDefault, DefaultHasher};

/// A file or folder a plan names, as written there (relative to the repo or the agent's folder).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Named {
    pub path: String,
    pub folder: bool,
}

/// Plan entries, task paths, and inside and outside touches, each: a runaway agent can't grow the
/// plugin's memory or its per-event work without end.
pub const MAX: usize = 256;

/// Words with a slash that are prose, not paths.
const PROSE: [&str; 12] = ["and/or", "either/or", "input/output", "read/write", "client/server", "true/false", "yes/no", "on/off", "i/o", "n/a", "w/o", "pass/fail"];

/// Files named without an extension.
const BARE: [&str; 6] = ["Makefile", "Dockerfile", "Gemfile", "Pipfile", "Procfile", "LICENSE"];

/// `module.exports`, `process.env`: code a plan backticks, read as a name with an extension otherwise.
const CODE: [&str; 14] = ["module", "process", "console", "window", "document", "globalThis", "this", "self", "os", "sys", "std", "Math", "JSON", "Object"];

/// Bytes of one stored path; longer plan paths are skipped and longer touched ones cut.
pub const PATH_MAX: usize = 256;

/// Bytes of plan paths one scope keeps, and of task paths: a quarter and an eighth of the plugin's
/// 64 KiB across all scopes, so one scope can't fill it alone.
pub const PLAN_PATHS: usize = 16 * 1024;
pub const TASK_PATHS: usize = 8 * 1024;

/// The row suffix Haunt shows, in characters.
const SUFFIX: usize = 64;

/// Folders above the agent's a plan's paths may be relative to (the repo root, usually one or two up).
const UP: usize = 8;

/// The paths and folders `text` names: backticked names, Markdown link targets, words with a `/`,
/// words with a file extension and known extensionless files. A backticked span of one word counts
/// whatever it is. URLs, absolute paths outside `cwd`, `~`, `..`, globs and Windows paths are left
/// out. At most `MAX`.
pub fn extract(text: &str, cwd: &str) -> Vec<Named> {
    let mut out: Vec<Named> = Vec::new();
    let mut seen: HashSet<Named> = HashSet::new();
    for (i, span) in text.split('`').enumerate() {
        let code = i % 2 == 1;
        let lone = code && span.split_whitespace().nth(1).is_none();
        for word in span.split_whitespace() {
            for (k, piece) in word.split("](").enumerate() {
                // `[text](path#anchor)`
                let piece = if k > 0 { piece.split('#').next().unwrap_or(piece) } else { piece };
                if let Some(n) = named(piece, cwd, code, lone)
                    && seen.insert(n.clone())
                {
                    out.push(n);
                    if out.len() == MAX {
                        return out;
                    }
                }
            }
        }
    }
    out
}

fn named(word: &str, cwd: &str, code: bool, lone: bool) -> Option<Named> {
    let punct = |c: char| ",;:!?()[]{}\"'*<>".contains(c);
    let w = word.trim_start_matches(punct).trim_end_matches(|c| punct(c) || c == '.');
    if w.is_empty() || w.contains("://") || w.contains('\\') || w.starts_with(['~', '-']) || w.contains(['$', '*', '{', '}', '|', '=', '@', '#']) {
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
    if path.is_empty() || path.len() > PATH_MAX || path.split('/').any(|s| s.is_empty() || s == "." || s == ".." || !s.chars().all(|c| c.is_alphanumeric() || "._-".contains(c))) {
        return None;
    }
    if !code && PROSE.contains(&path.to_lowercase().as_str()) {
        return None;
    }
    let last = path.rsplit('/').next().unwrap_or(path);
    if !path.contains('/') && path.split_once('.').is_some_and(|(root, _)| CODE.contains(&root)) {
        return None;
    }
    let file = has_extension(last) || BARE.contains(&last);
    match (folder, path.contains('/'), file) {
        (true, _, _) => Some(Named { path: path.into(), folder: true }),
        (false, _, true) => Some(Named { path: path.into(), folder: false }),
        (false, true, false) => Some(Named { path: path.into(), folder: true }),
        // `Justfile`, not `RATE_MS`, `createServer`, `limiter` or `1000`.
        (false, false, false) => (lone && file_like(path)).then(|| Named { path: path.into(), folder: false }),
    }
}

/// A capital, then letters and digits with at least one lowercase: how extensionless files are named.
fn file_like(name: &str) -> bool {
    name.starts_with(|c: char| c.is_ascii_uppercase())
        && name.chars().all(|c| c.is_ascii_alphanumeric())
        && name.chars().any(|c| c.is_ascii_lowercase())
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

const TEST_DIRS: [&str; 4] = ["test", "tests", "__tests__", "spec"];

fn in_test_dir(dir: &str) -> bool {
    dir.split('/').any(|s| TEST_DIRS.contains(&s))
}

/// `foo.test.js`, `foo.spec.ts`, `foo_test.go`, `test_foo.py` -> `foo.js`, `foo.ts`, `foo.go`, `foo.py`.
fn untested(name: &str) -> String {
    let (stem, ext) = name.rsplit_once('.').unwrap_or((name, ""));
    let stem = stem.strip_suffix(".test").or_else(|| stem.strip_suffix(".spec")).or_else(|| stem.strip_suffix("_test")).or_else(|| stem.strip_prefix("test_")).unwrap_or(stem);
    if ext.is_empty() { stem.into() } else { format!("{stem}.{ext}") }
}

fn split(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}

/// `path` with `.`, `..` and empty segments resolved; None unless absolute and staying under `/`.
pub fn normal(path: &str) -> Option<String> {
    let mut segs: Vec<&str> = Vec::new();
    for s in path.strip_prefix('/')?.split('/') {
        match s {
            "" | "." => {}
            ".." => {
                segs.pop()?;
            }
            s => segs.push(s),
        }
    }
    Some(format!("/{}", segs.join("/")))
}

/// `path` relative to `cwd`, when strictly under it.
fn rel_of<'a>(path: &'a str, cwd: &str) -> Option<&'a str> {
    path.strip_prefix(cwd.trim_end_matches('/'))?.strip_prefix('/').filter(|r| !r.is_empty())
}

/// `/Users/me/x` as `~/x`.
fn tilde(path: &str) -> String {
    for home in ["/Users/", "/home/"] {
        if let Some(rest) = path.strip_prefix(home) {
            return match rest.split_once('/') {
                Some((_, r)) => format!("~/{r}"),
                None => "~".into(),
            };
        }
    }
    path.into()
}

/// One named path, split up once per plan so a touch compares without building strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    named: Named,
    dir: String,
    name: String,
    untested: String,
    in_tests: bool,
}

pub fn rules(named: &[Named]) -> Vec<Rule> {
    named
        .iter()
        .map(|n| {
            let (dir, name) = split(&n.path);
            Rule { named: n.clone(), dir: dir.into(), name: name.into(), untested: untested(name), in_tests: in_test_dir(dir) }
        })
        .collect()
}

/// Whether `path` (absolute and `normal`) is inside what `plan` names. Only a file under the agent's
/// folder can be; the plan's paths are read from there or from a folder above it.
pub fn inside(path: &str, cwd: &str, plan: &[Rule]) -> bool {
    let Some(rel) = rel_of(path, cwd) else { return false };
    // `src/a.rs`, then `web/src/a.rs`, … as seen from each folder above.
    let mut seen = vec![rel.to_owned()];
    let mut tail = String::new();
    for seg in cwd.trim_end_matches('/').rsplit('/').filter(|s| !s.is_empty()).take(UP) {
        tail = format!("{seg}/{tail}");
        seen.push(format!("{tail}{rel}"));
    }
    let (rdir, name) = split(rel);
    let (base, tests) = (untested(name), in_test_dir(rdir));
    plan.iter().any(|r| {
        let n = &r.named;
        if n.folder {
            return seen.iter().any(|c| c.strip_prefix(n.path.as_str()).is_some_and(|rest| rest.starts_with('/')));
        }
        if seen.contains(&n.path) {
            return true;
        }
        let same_dir = || seen.iter().any(|c| split(c).0 == r.dir);
        if name != r.name && base == r.untested && (tests || r.in_tests || same_dir()) {
            return true;
        }
        locks(&r.name).contains(&name) && (!rel.contains('/') || same_dir())
    })
}

/// Whether a suffix would read as a row needing you: a looser copy of Haunt's own check, so Haunt
/// never refuses what passes here. Anything non-ASCII counts, as Haunt reads it as a wildcard.
pub fn says_needs_you(text: &str) -> bool {
    if !text.is_ascii() {
        return true;
    }
    let mut flat: Vec<u8> = Vec::new();
    for c in text.bytes().map(|c| c.to_ascii_lowercase()) {
        let c = match c {
            b'0' => b'o',
            b'1' | b'i' | b'|' => b'l',
            b'5' => b's',
            c if c.is_ascii_alphanumeric() => c,
            _ => continue,
        };
        if c == b'v' && flat.last() == Some(&b'v') {
            *flat.last_mut().unwrap() = b'w';
        } else {
            flat.push(c);
        }
    }
    [&b"needsyou"[..], b"waltlngforyou"].iter().any(|p| flat.windows(p.len()).any(|w| w == *p))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Touch {
    /// Relative to the agent's folder when under it, else the full path with `~` for home; at most
    /// `PATH_MAX` bytes.
    pub shown: String,
    pub local: bool,
    pub inside: bool,
}

/// A fresh string, as `truncate` would keep the long one's allocation.
fn cut(s: &str) -> String {
    s[..s.floor_char_boundary(PATH_MAX)].to_owned()
}

/// A path already touched: itself when short, else its hash, so a flood of long ones costs 8 bytes
/// each and only those could ever collide.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Seen {
    Path(String),
    Hash(u64),
}

fn seen(path: &str) -> Seen {
    if path.len() <= PATH_MAX { Seen::Path(path.into()) } else { Seen::Hash(BuildHasherDefault::<DefaultHasher>::default().hash_one(path)) }
}

/// One agent's plan and what it touched since.
#[derive(Debug, Clone, Default)]
pub struct Scope {
    pub cwd: String,
    pub plan: Vec<Named>,
    /// Paths its task subjects named, kept across plans.
    pub tasks: Vec<Named>,
    named: Vec<Named>,
    rules: Vec<Rule>,
    /// Bytes of the paths in `plan` and `tasks`.
    pub bytes: usize,
    seen: HashSet<Seen>,
    pub touched: Vec<Touch>,
    pub inside_n: usize,
    pub outside_n: usize,
    /// More touches came than `MAX` of their kind.
    pub inside_full: bool,
    pub outside_full: bool,
}

impl Scope {
    /// A new plan replaces the old one and starts the touches over.
    pub fn planned(&mut self, cwd: &str, plan: &str) {
        self.cwd = normal(cwd).unwrap_or_else(|| cwd.into());
        let mut used = 0;
        self.plan = extract(plan, &self.cwd)
            .into_iter()
            .take_while(|n| {
                used += n.path.len();
                used <= PLAN_PATHS
            })
            .collect();
        self.seen.clear();
        self.touched.clear();
        (self.inside_n, self.outside_n, self.inside_full, self.outside_full) = (0, 0, false, false);
        self.renamed();
    }

    pub fn tasked(&mut self, cwd: &str, subject: &str) {
        let known: HashSet<Named> = self.tasks.iter().cloned().collect();
        let mut used: usize = self.tasks.iter().map(|n| n.path.len()).sum();
        let before = self.tasks.len();
        for n in extract(subject, cwd).into_iter().filter(|n| !known.contains(n)) {
            if self.tasks.len() >= MAX || used + n.path.len() > TASK_PATHS {
                break;
            }
            used += n.path.len();
            self.tasks.push(n);
        }
        if self.tasks.len() > before {
            self.renamed();
        }
    }

    fn renamed(&mut self) {
        let mut seen: HashSet<&Named> = HashSet::new();
        self.named = self.plan.iter().chain(&self.tasks).filter(|n| seen.insert(n)).cloned().collect();
        self.rules = rules(&self.named);
        self.bytes = self.plan.iter().chain(&self.tasks).map(|n| n.path.len()).sum();
    }

    /// The plan's paths, then the tasks' it doesn't have.
    pub fn named(&self) -> &[Named] {
        &self.named
    }

    /// Records a touch; true when the row or panel changes.
    pub fn touch(&mut self, raw: &str) -> bool {
        if self.plan.is_empty() {
            return false;
        }
        let path = normal(raw).unwrap_or_else(|| raw.into());
        if self.seen.contains(&seen(&path)) {
            return false;
        }
        let inside = inside(&path, &self.cwd, &self.rules);
        let (n, full) = if inside { (&mut self.inside_n, &mut self.inside_full) } else { (&mut self.outside_n, &mut self.outside_full) };
        if *n >= MAX {
            return !std::mem::replace(full, true);
        }
        *n += 1;
        let (shown, local) = match rel_of(&path, &self.cwd) {
            Some(r) => (cut(r), true),
            None => (cut(&tilde(&path)), false),
        };
        self.seen.insert(seen(&path));
        self.touched.push(Touch { shown, local, inside });
        true
    }

    pub fn outside(&self) -> impl Iterator<Item = &Touch> {
        self.touched.iter().filter(|t| !t.inside)
    }

    /// The row: `2 outside plan` and `package.json, .github/`; nothing while all is inside.
    pub fn row(&self) -> Option<(String, String)> {
        if self.outside_n == 0 {
            return None;
        }
        let label = if self.outside_full { format!("{MAX}+ outside plan") } else { format!("{} outside plan", self.outside_n) };
        let mut listed: HashSet<String> = HashSet::new();
        let mut suffix = String::new();
        for t in self.outside() {
            let s = if t.local { short(&t.shown, &self.named) } else { t.shown.clone() };
            if !listed.insert(s.clone()) {
                continue;
            }
            let join = |item: &str| if suffix.is_empty() { item.to_owned() } else { format!("{suffix}, {item}") };
            let mut next = join(&s);
            // A name that reads as a status, or one too long for the row on its own.
            if says_needs_you(&next) || s.chars().count() > SUFFIX {
                next = join("1 file");
            }
            // Whole items only, so Haunt's cut never lands mid-name.
            if next.chars().count() > SUFFIX || says_needs_you(&next) {
                break;
            }
            suffix = next;
        }
        Some((label, suffix))
    }
}

/// How an outside file shows on the row: its top folder (`.github/`) unless the plan names
/// something under that folder, else its path.
pub fn short(rel: &str, plan: &[Named]) -> String {
    match rel.split_once('/') {
        Some((top, _)) if !plan.iter().any(|n| n.path == top || n.path.strip_prefix(top).is_some_and(|r| r.starts_with('/'))) => format!("{top}/"),
        _ => rel.into(),
    }
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
3. Bump `package.json` for jose 5.2, per [the notes](docs/jose.md#upgrade).\n\
4. Wire it into the Makefile and `scripts/ci`.\n\
- Don't touch ../shared/config.ts or C:\\Users\\me\\x.ts or ~/notes.md.\n\
- Read https://example.com/rfc.html and /etc/hosts and/or the Node.js docs, i.e. nothing else.\n\
- Run `npm test`, `cargo test -p core` and `--force` never.";

    fn paths(n: &[Named]) -> Vec<String> {
        n.iter().map(|n| if n.folder { format!("{}/", n.path) } else { n.path.clone() }).collect()
    }

    #[test]
    fn a_real_plan_names_its_files_and_folders_and_nothing_else() {
        let n = extract(PLAN, "/w/shop");
        assert_eq!(
            paths(&n),
            [
                "src/auth/refresh.ts",
                "src/middleware/session.ts",
                "src/auth/",
                "docs/auth.md",
                "src/api/client.ts",
                "package.json",
                "docs/jose.md",
                "Makefile",
                "scripts/ci/",
                "Node.js"
            ],
        );
    }

    #[test]
    fn absolute_paths_count_only_inside_the_agents_folder() {
        assert_eq!(paths(&extract("edit /w/shop/lib/x.rs and /w/other/y.rs", "/w/shop")), ["lib/x.rs"]);
        assert_eq!(paths(&extract("see `./scripts/` and .github/ and .env", "/w")), ["scripts/", ".github/", ".env"]);
        assert!(extract("version 1.2.3, v2, e.g. this", "/w").is_empty());
    }

    #[test]
    fn a_lone_backticked_word_or_a_known_file_is_named() {
        assert_eq!(paths(&extract("Edit `Justfile`, docker/Dockerfile and LICENSE, not `make all` or Justfile", "/w")), ["Justfile", "docker/Dockerfile", "LICENSE"]);
    }

    #[test]
    fn backticked_code_in_a_real_plan_is_not_a_file() {
        let plan = "# Plan: configurable rate limit via `RATE_MS`\n\
The limiter in `src/server.js` is hardcoded (`limiter(1)` in `createServer`), default `1000`.\n\
1. `src/server.js`: read `process.env.RATE_MS`; replace `limiter(1)` with `limiter(1000 / rateMs)`. `limiter` and `module.exports` are unchanged.\n\
2. `README.md`: document `RATE_MS`.\n\
Verification: `npm test` and `RATE_MS=500 node -e \"require('./src/server').createServer().listen(0)\"`.";
        assert_eq!(paths(&extract(plan, "/w/suite")), ["src/server.js", "README.md"]);
    }

    #[test]
    fn a_plan_names_at_most_max_paths() {
        let text: String = (0..MAX * 4).map(|i| format!("`src/f{i}.rs` `src/f{i}.rs` ")).collect();
        let n = extract(&text, "/w");
        assert_eq!((n.len(), n[MAX - 1].path.as_str()), (MAX, format!("src/f{}.rs", MAX - 1).as_str()));
    }

    fn plan(text: &str) -> Scope {
        let mut s = Scope::default();
        s.planned("/w/shop", text);
        s
    }

    fn at(s: &Scope, p: &str) -> bool {
        inside(&format!("/w/shop/{p}"), "/w/shop", &s.rules)
    }

    #[test]
    fn named_files_folders_and_suffixes_are_inside() {
        let s = plan(PLAN);
        assert!(at(&s, "src/auth/refresh.ts") && at(&s, "src/auth/token.ts") && at(&s, "src/auth/deep/x.ts"));
        assert!(at(&s, "package.json") && at(&s, "docs/auth.md") && at(&s, "Makefile"));
        assert!(!at(&s, "src/billing/x.ts") && !at(&s, ".github/workflows/ci.yml") && !at(&s, "docs/other.md"));
        let web = rules(&extract("`web/src/api/client.ts`", "/w/shop/web"));
        assert!(inside("/w/shop/web/src/api/client.ts", "/w/shop/web", &web), "names relative to the repo root match from a subfolder");
    }

    #[test]
    fn nothing_outside_the_agents_folder_is_inside() {
        let s = plan("`src/auth/` `src/auth/refresh.ts` `x.ts` `package.json`");
        let out = |p: &str| !inside(p, "/w/shop", &s.rules);
        assert!(out("/w/other/src/auth/refresh.ts"), "same tail, another repo");
        assert!(out("/w/other/src/auth/token.ts"), "a folder of the same name elsewhere");
        assert!(out("/w/tests/x.test.ts"), "a test folder above the agent's");
        assert!(out("/w/shop/src/x.test.ts"), "a root file's test is next to it or in a test folder, not anywhere");
        assert!(out("/w/shop/deep/x.ts") && out("/w/shop/deep/package-lock.json"), "a root file is at the root");
        assert!(!out("/w/shop/x.test.ts") && !out("/w/shop/tests/x.test.ts") && !out("/w/shop/package-lock.json"));
    }

    #[test]
    fn dot_segments_resolve_before_matching() {
        let mut s = plan("`src/auth/`");
        assert_eq!(normal("/w/shop/src/auth/../../../other/src/auth/x.ts").as_deref(), Some("/w/other/src/auth/x.ts"));
        assert_eq!(normal("/../x"), None);
        s.touch("/w/shop/src/auth/./x.ts");
        s.touch("/w/shop/src/auth/../../../other/src/auth/y.ts");
        s.touch("/w/shop/src//auth/x.ts");
        assert_eq!(s.row(), Some(("1 outside plan".into(), "/w/other/src/auth/y.ts".into())), "once each, by where it lands");
    }

    #[test]
    fn test_siblings_and_lockfiles_follow_what_the_plan_names() {
        let named = rules(&extract("`src/api/client.ts`, `lib/parse.py`, `package.json`, `crates/core/Cargo.toml`", "/w"));
        let at = |p: &str| inside(&format!("/w/{p}"), "/w", &named);
        assert!(at("src/api/client.test.ts") && at("src/api/client.spec.ts") && at("test/client.test.ts") && at("src/api/__tests__/client.test.ts"));
        assert!(at("lib/test_parse.py") && at("tests/test_parse.py"));
        assert!(!at("src/other/client.test.ts"), "a test of another client is not a sibling");
        assert!(at("package-lock.json") && at("yarn.lock") && at("crates/core/Cargo.lock") && at("Cargo.lock"), "the workspace lock sits at the root");
        assert!(!at("src/package-lock.json") && !at("Gemfile.lock"));
        let tests = rules(&extract("`src/foo.test.js`", "/w"));
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
        for p in ["src/auth/refresh.ts", "package.json", ".github/workflows/ci.yml", ".github/dependabot.yml", "src/billing/x.ts", "/Users/me/notes.md"] {
            s.touch(&format!("{}{p}", if p.starts_with('/') { "" } else { "/w/shop/" }));
        }
        assert_eq!(s.row(), Some(("5 outside plan".into(), "package.json, .github/, src/billing/x.ts, ~/notes.md".into())));
    }

    #[test]
    fn inside_edits_cant_crowd_out_outside_ones() {
        let mut s = plan("`src/`");
        for i in 0..MAX + 10 {
            s.touch(&format!("/w/shop/src/f{i}.rs"));
        }
        assert!(s.touch("/w/shop/x.rs"), "still counted");
        assert_eq!((s.inside_n, s.inside_full, s.row().unwrap().0), (MAX, true, "1 outside plan".into()));
        let mut changed = 0;
        for i in 0..MAX + 10 {
            changed += s.touch(&format!("/w/shop/o{i}.rs")) as usize;
        }
        assert_eq!((s.row().unwrap().0, changed), (format!("{MAX}+ outside plan"), MAX), "MAX - 1 more, then one change to say so");
    }

    #[test]
    fn a_file_named_like_a_status_never_reaches_the_row() {
        assert!(says_needs_you("needs you/x.rs") && says_needs_you("NEEDS_Y0U") && says_needs_you("needs, you.rs") && says_needs_you("vvaiting-for-you"));
        assert!(says_needs_you("café.md"), "non-ASCII could be a lookalike");
        assert!(!says_needs_you("package.json, .github/") && !says_needs_you("1 file"));
        let mut s = plan("`src/`");
        for p in ["needs you/x.rs", "a.rs", "needs", "you.rs"] {
            s.touch(&format!("/w/shop/{p}"));
        }
        let (label, suffix) = s.row().unwrap();
        assert_eq!((label.as_str(), suffix.as_str()), ("4 outside plan", "1 file, a.rs, needs, 1 file"));
        assert!(!says_needs_you(&suffix));
    }

    #[test]
    fn the_suffix_holds_whole_names_only() {
        let mut s = plan("`src/`");
        for p in ["aaaaaaaaaaaaaaaaaaaa.rs", "bbbbbbbbbbbbbbbbbbbb.rs", "cccccccccccccccccccc.rs", "d.rs"] {
            s.touch(&format!("/w/shop/{p}"));
        }
        let long = "𝕏".repeat(70);
        s.touch(&format!("/w/shop/{long}"));
        let (_, suffix) = s.row().unwrap();
        assert_eq!(suffix, "aaaaaaaaaaaaaaaaaaaa.rs, bbbbbbbbbbbbbbbbbbbb.rs");
        let mut s = plan("`src/`");
        s.touch(&format!("/w/shop/{long}"));
        s.touch("/w/shop/e.rs");
        assert_eq!(s.row().unwrap().1, "1 file, e.rs", "a name too long for the row");
    }

    #[test]
    fn long_paths_are_cut_or_skipped() {
        let long = "x".repeat(PATH_MAX);
        assert!(extract(&format!("`src/{long}.rs`"), "/w").is_empty());
        let mut s = plan("`src/`");
        s.touch(&format!("/w/shop/{}", "𝕏".repeat(1000)));
        assert!(s.touched[0].shown.len() <= PATH_MAX && s.touched[0].shown.chars().all(|c| c == '𝕏'));
    }

    #[test]
    fn one_scope_keeps_a_bounded_share_of_plan_and_task_paths() {
        let long = |i: usize| format!("`src/{}{i}.rs` ", "p".repeat(220));
        let mut s = plan(&(0..250).map(long).collect::<String>());
        assert!(s.bytes <= PLAN_PATHS && s.plan.len() > 60, "{} paths, {} bytes", s.plan.len(), s.bytes);
        for i in 0..100 {
            s.tasked("/w/shop", &(0..4).map(|j| long(1000 + i * 4 + j)).collect::<String>());
        }
        let tasks: usize = s.tasks.iter().map(|n| n.path.len()).sum();
        assert!(tasks <= TASK_PATHS && s.bytes <= PLAN_PATHS + TASK_PATHS);
    }

    #[test]
    fn a_long_path_is_seen_once_too() {
        let mut s = plan("`src/`");
        let long = format!("/w/shop/src/{}.rs", "q".repeat(400));
        assert!(s.touch(&long) && !s.touch(&long));
        assert!(s.touch("/w/shop/a.rs") && !s.touch("/w/shop/./a.rs"));
    }

    #[test]
    fn task_subjects_widen_the_plan_but_no_plan_means_nothing() {
        let mut s = Scope::default();
        s.tasked("/w/shop", "Write `README.md`");
        assert!(!s.touch("/w/shop/x.rs"), "no plan, no touches");
        s.planned("/w/shop", "Fix `src/a.rs`.");
        s.touch("/w/shop/README.md");
        assert_eq!(s.row(), None);
        for i in 0..MAX * 2 {
            s.tasked("/w/shop", &format!("Write `t{i}.md`"));
        }
        assert_eq!(s.tasks.len(), MAX);
    }
}
