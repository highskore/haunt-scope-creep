# Scope creep

Mark an agent's Haunt row when it edits files its own plan didn't name.

When an agent leaves plan mode, this plugin reads the plan for the files and folders it names. From then on it compares every file the agent edits against that list. Once an edit falls outside, the row shows `2 outside plan` on the right and the files after the meta line (`package.json, .github/`). A row with no plan, or with every edit inside the plan, shows nothing.

## What counts as named

- Backticked names, and words with a `/` or a file extension (`src/auth/refresh.ts`, `docs/`, `package.json`). Prose words are skipped.
- Paths are read relative to the agent's folder. An absolute path inside it counts. Windows paths, `../`, `~`, URLs and globs are skipped.
- Paths in the subjects of the agent's tasks count too, for as long as the session runs.

An edit is inside the plan when it is:

- a named file;
- anywhere in a named folder;
- a test next to a named file: `foo.js` covers `foo.test.js`, `foo.spec.js`, `foo_test.go`, `test_foo.py`, and the same name under `test/`, `tests/`, `__tests__/` or `spec/`;
- the lockfile of a named manifest: `package.json` covers `package-lock.json`, `yarn.lock`, `pnpm-lock.yaml` and `bun.lock`, and `Cargo.toml` covers `Cargo.lock`. The same goes for `pyproject.toml`, `Pipfile`, `Gemfile`, `go.mod`, `composer.json` and `mix.exs`.

A new plan replaces the old one, and the count starts over. Each subagent with its own plan is checked against that plan. A subagent without one is checked against its session's plan.

## What it can't see

- **Files written by shell commands.** It only sees the agent's Edit, Write, MultiEdit and NotebookEdit tools. `sed -i`, `echo >`, a code generator or a `git checkout` run through Bash don't show up.
- **Plans not made in plan mode.** It reads the plan the agent leaves plan mode with. It doesn't read a plan written into chat or into a file.
- **What the plan meant.** A file the plan describes without naming it ("the config") counts as outside.
- **Unverified events.** Events from a tab without its nonce are ignored, so another process can't fake a plan or an edit.

## The Scope panel

Type `scope ` in ⌘K to list the agents that have a plan, or right-click an agent's row and choose **Scope**. The panel opens on the right and shows the paths the plan names, then each file the agent touched. The outside ones are orange. While the panel is open it updates as the agent works.

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
