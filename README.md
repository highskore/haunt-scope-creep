# Scope creep

Mark an agent's Haunt row when it edits files its own plan didn't name.

When an agent leaves plan mode, this plugin reads the plan for the files and folders it names. From then on it compares every file the agent edits against that list. Once an edit falls outside, the row shows `2 outside plan` on the right and the files after the meta line (`package.json, .github/`). A row with no plan, or with every edit inside the plan, shows nothing.

## What counts as named

- Backticked names, Markdown link targets (`[notes](docs/auth.md)`), and words with a `/` or a file extension (`src/auth/refresh.ts`, `docs/`, `package.json`). Prose words are skipped.
- `Makefile`, `Dockerfile`, `Gemfile`, `Pipfile`, `Procfile` and `LICENSE`, and a single backticked word named like them (`` `Justfile` ``, `` `README` ``, `` `CODEOWNERS` ``). Backticked code such as `` `RATE_MS` ``, `` `createServer` ``, `` `module.exports` `` or `` `process.env.PORT` `` is not a file, while `` `module.rs` `` or `` `console.log` `` is.
- Paths are read relative to the agent's folder, or a folder above it such as the repo root. An absolute path inside it counts. Windows paths, `../`, `~`, URLs and globs are skipped.
- A plan counts its first 256 paths, and tasks add up to 256 more.
- Paths in the subjects of the agent's tasks count too, for as long as the session runs.

An edit is inside the plan only when it lands in the agent's folder (after resolving `.` and `..`), and it is:

- a named file;
- anywhere in a named folder;
- a test next to a named file: `foo.js` covers `foo.test.js`, `foo.spec.js`, `foo_test.go`, `test_foo.py`, and the same name under `test/`, `tests/`, `__tests__/` or `spec/`;
- the lockfile of a named manifest: `package.json` covers `package-lock.json`, `yarn.lock`, `pnpm-lock.yaml` and `bun.lock`, and `Cargo.toml` covers `Cargo.lock`. The same goes for `pyproject.toml`, `Pipfile`, `Gemfile`, `go.mod`, `composer.json` and `mix.exs`.

An edit outside the agent's folder always counts as outside, shown by its full path (`~/notes.md`). The row lists whole names, as many as fit in Haunt's 64 characters; a name that reads like a Haunt status, or one too long for the row, shows as `1 file`. After 256 outside edits the row reads `256+ outside plan`.

A new plan replaces the old one, and the count starts over. When a tab starts a new session, the old session's plans go. Memory is bounded across all agents: 64 plans (32 of them task-only), 4,096 touched files and 64 KB of plan paths in total, each path cut to 256 bytes. One plan keeps at most 16 KB of paths, and its tasks 8 KB. Past the totals it drops task-only scopes, then plans no tab is showing, oldest first; a plan a tab shows, or its session's own plan behind a subagent's, goes only past the 64-plan count. Each subagent with its own plan is checked against that plan. A subagent without one is checked against its session's plan.

## What it can't see

- **Files written by shell commands.** It only sees the agent's Edit, Write, MultiEdit and NotebookEdit tools. `sed -i`, `echo >`, a code generator or a `git checkout` run through Bash don't show up.
- **Plans not made in plan mode.** It reads the plan the agent leaves plan mode with. It doesn't read a plan written into chat or into a file.
- **What the plan meant.** A file the plan describes without naming it ("the config") counts as outside.
- **Two plans in one tab.** A tab shows the plan it heard last. If two agents plan in the same tab at once, the row follows whichever planned or edited most recently.
- **Words that only look like paths.** `TCP/IP`, `Node.js` or `v1.2/beta` in the plan's prose count as named. That only widens the plan, so an edit there reads as inside.
- **Unverified events.** Events from a tab without its nonce are ignored, so another process can't fake a plan or an edit.

## The Scope panel

Type `scope ` in ⌘K to list the agents that have a plan, or right-click an agent's row and choose **Scope**. The panel opens on the right and shows the paths the plan names, then each file the agent touched. The outside ones are orange. A long list stops at 200 rows, with the outside files first. While the panel is open it updates as the agent works.

## Settings

```toml
[plugins."dev.haunt.scope-creep"]
row = true   # mark the row with "2 outside plan" and the files; the panel works either way
```

## Capabilities

| capability | why |
|---|---|
| `events` (plan exit, task, file edited) | when a plan is set and which files the agent edits |
| `event_details` | the plan's text, task subjects and the edited file's path |
| `decorate` | the count and files on the row |
| `panel` | the Scope panel |

It has no file, command, terminal or network access. It reads nothing from disk, and the plan and paths stay in the plugin's memory.

## Build and test

```sh
cargo build --release --target wasm32-wasip2   # or ./build.sh
cargo test                                     # unit tests
haunt plugin install .
```

`e2e/scope_creep.rs` runs the built plugin against a real Haunt core: copy it into a Haunt checkout's `cli/tests/` and run `HAUNT_PLUGINS_DIR=<this repo's parent> cargo test -p haunt-cli --features plugins --test scope_creep`.

Needs Haunt with plugin API 0.5 (`api = "0.5"`).

## Licence

MIT, see `LICENSE`.
