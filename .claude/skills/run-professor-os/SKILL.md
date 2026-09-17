---
name: run-professor-os
description: Launch the Professor OS Tauri app and drive its React UI (chat, sidebar views) to verify a change works. Use when asked to run, start, screenshot, or smoke-test the app on macOS.
---

# Run Professor OS

Tauri 2 + React + TypeScript desktop app. Frontend is served by Vite on
`http://localhost:1420`; the Rust shell loads it into a native WebView.

## 0. Build prerequisites

- Chat needs the Claude Code CLI (`claude`) installed and signed in; there is no on-device model.
- If cargo reports a dependency version that crates.io clearly has as missing
  (e.g. `failed to select a version for h2 = "^0.4.14"`), the local sparse index
  cache is stale: delete `~/.cargo/registry/index/*/.cache/<prefix>/<crate>` and retry.

## 1. Launch the real app

From the project root, in the background (first build takes a few minutes; later ones about 20s):

```bash
npm run tauri dev > "$SCRATCH/tauri-dev.log" 2>&1   # run_in_background
```

Wait for readiness. The log contains ANSI color codes, so match the binary
path, not the literal "Running `...`" string:

```bash
until grep -qE 'target/debug/professor-os|error\[|panicked' "$SCRATCH/tauri-dev.log"; do sleep 1; done
tail -5 "$SCRATCH/tauri-dev.log"; pgrep -fl target/debug/professor-os
```

Also confirm the frontend: `curl -s http://localhost:1420/ | grep -o '<title>.*</title>'`
should print `<title>Professor OS</title>`.

## 2. Drive the UI

The native window usually cannot be captured or clicked from the terminal:
without Screen Recording permission `screencapture` returns only the desktop
wallpaper, and `osascript` fails with "not allowed assistive access (-1719)".
Swift CoreGraphics scripts fail with Command Line Tools only, and Python has no
`Quartz` module.

Instead, drive the same frontend in headless Chrome via the DevTools Protocol.
No npm installs are needed (Node's built-in `fetch`/`WebSocket`):

```bash
"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" --headless=new --disable-gpu \
  --hide-scrollbars --remote-debugging-port=9223 --user-data-dir="$SCRATCH/chrome-profile" \
  --window-size=1100,720 about:blank > "$SCRATCH/chrome.log" 2>&1   # run_in_background

node .claude/skills/run-professor-os/drive.mjs "$SCRATCH"
```

`drive.mjs` walks the people flow in light mode, then dark: a new name in chat
offers a "Who is …?" card (role chips, profile-link fill, missing fields) and a
section; typing an existing name ("Sat…", "Fuz…") shows the mention popover and
Tab picks it (highlighted, sent as `mentions`); ⌘K opens a person's page; the
Students section; Today. It prints a JSON result (including console errors) and
numbered screenshots. **Look at the screenshots.** Restart headless Chrome
between runs: an old session's injected stub can linger. Headless Chrome doesn't
show WebKit-only problems: after a change to fonts or CSS, also check the
`tauri dev` log for CoreText notes (non-standard font weights fell back to Times
in the real window).

Headless Chrome only exercises the frontend. There is no Tauri IPC bridge
there, so `drive.mjs` stubs `window.__TAURI_INTERNALS__.invoke` with a small
in-memory backend and reports the commands used. Keep the stub in sync with
`src/api.ts`. Verify real backend behavior with the Rust tests:

```bash
cd src-tauri && cargo test --lib
```

## 2c. Evaluate chat with Claude

Chat runs
through a long-lived `claude -p` process (`src-tauri/src/claude.rs`) that reaches the graph through
the app's localhost MCP server (`mcp.rs`, tools in `tools.rs`). Google Calendar writes are allowed
only for items confirmed in the UI; `tools::approve` enforces this. To run a multi-turn conversation
with real Claude against an in-memory graph (uses the Claude account):

```bash
cd src-tauri && cargo test --lib claude_ -- --ignored --nocapture --test-threads 1
```

`claude_conversation` covers students, tasks and day planning; `claude_pages_and_focus` covers a
new student's section offer and details form, notes, search, questions asked from the page, and
the page's Obsidian file; `claude_people` covers details given in chat, a new name becoming a person
the app asks about, filling a profile from a pasted link (served locally), and pages picked while
typing; `claude_tasks` covers tasks linked FOR a person, WAITING_ON a person and under a course,
planning from the `tasks` tool, and a task ticked off in the app; `claude_organizations` covers one
page per institution whatever it's called (aliases), departments PART_OF it, positions and dates on
AFFILIATED_WITH/STUDIED_AT links, a move kept as history, and "who do I know at IITG"; `claude_rename`
covers renaming a page's heading; `claude_assigned_tasks` covers a task delegated to two students
(ASSIGNED_TO) kept apart from the professor's own; `claude_project_status` covers projects becoming
in-progress, planned or completed from how they're described, and "which projects are ongoing"; `claude_corrections`
covers a real mix-up: a pasted student must not be tied to an earlier Faculty Advisor
follow-up task, and "this is wrong" must undo the tie (name, notes, links), not just reword it. To
inspect what was said in the real app, copy the DB and run `dump_messages` in graph.rs. `claude_following` covers following a researcher
from pasted links (X/LinkedIn kept, not read) and Claude asking which OpenAlex author is theirs. It
also covers a real first check of OpenAlex and their homepage (baseline, nothing reported), and
Claude rating new papers against the professor's projects, ideas, and research areas. It also uses
the network.

Design (drive.mjs steps 34–42): page and section emoji icons (`ui/IconPicker.tsx`, suggestions in
`ui/emoji.ts`, stored as `info.icon` / `Section.icon`), Favorites (tag `favorite`), the sidebar
New menu, ⌘K commands, Today's quick add, and Settings > Appearance (theme and accent in
localStorage, applied as `data-theme` / `data-accent` on `<html>`). Headless Chrome draws emoji with
a different font than the Mac app.

Following people (`watch.rs`): a background loop in `lib.rs` checks followed people (tag
`following`) every minute. Each source is read at most every 6 hours; "Check now" forces a check.
Relevance is judged by a one-shot `claude -p --json-schema` call (`claude::ask_json`). Updates show
only inside the app, as the sidebar badge, the `updates-arrived` toast, the Updates view, and Today.
There are deliberately no OS notifications. `drive.mjs` exercises these screens; `window.__emit(event,
payload)` in its stub fires backend events such as the toast.

`tauri dev` rebuilds and restarts the app on every Rust change, against the real database and the
real vault in `~/Documents/Professor OS`. Back up both before changing sync or schema code.

Read the transcript: tool calls, replies, and the final graph dump. The calendar steps depend on
the Google Calendar connector being authenticated (`claude`, then `/mcp`); the log prints its
status (`needs-auth` means not connected).

## 2d. Assistant setup and releases

- First run shows `src/onboarding/Setup.tsx` until Claude Code or Codex is installed, signed in and
  chosen (`settings.json` in the app data dir). `drive.mjs` walks it with `?setup=fresh`.
- Real sign-in plumbing, without touching the real login:
  `cargo test --lib real_claude_sign_in -- --ignored --nocapture` (empty CLAUDE_CONFIG_DIR).
- Codex flags against the real CLI, signed out:
  `CODEX_BIN=/path/to/codex cargo test --lib real_codex_accepts -- --ignored --nocapture`
  (install one in the scratchpad with `npm i @openai/codex`).
- Release build on macOS: `OPENSSL_DIR="$(scripts/static-openssl.sh)" npm run tauri build`, then
  `otool -L` on `Contents/MacOS/professor-os` must show no `/opt/homebrew` paths. Smoke test with
  `env -i HOME=<empty dir> PATH=/usr/bin:/bin "<app>/Contents/MacOS/professor-os"`: it prints
  `mcp: tools…` and stays running.
- Releases: push a `v*` tag; `.github/workflows/release.yml` builds macOS arm64/x64 and Linux
  x86_64/arm64 into a draft release. Test Linux packages in Docker (`ubuntu:24.04`) before
  publishing. `git push` may pick up another account's credentials; use
  `git -c credential.helper= -c 'credential.helper=!gh auth git-credential' push`.

## 3. Clean up

```bash
pkill -f 'remote-debugging-port=9223'
```

Stopping the app: kill the background `tauri dev` task (or `pkill -f target/debug/professor-os; pkill -f node_modules/.bin/vite`).
Exit code 144 on a background task you killed is expected.
