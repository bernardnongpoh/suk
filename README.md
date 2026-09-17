<p align="center">
  <img src="src-tauri/icons/icon.png" width="96" alt="">
</p>

<h1 align="center">Professor OS</h1>

<p align="center">A calm place for a professor's students, research, teaching and admin.<br>
Tell it what's going on in plain words; it keeps everything organized.</p>

<p align="center">
  <img src="docs/screenshots/today.png" width="820" alt="Today: a greeting, quick add, and tasks grouped by when they're due">
</p>

## Install

**macOS or Linux**, in a terminal:

```sh
curl -fsSL https://raw.githubusercontent.com/bernardnongpoh/professor-os/main/install.sh | sh
```

On macOS this puts Professor OS in Applications. On Ubuntu and Debian it installs the `.deb`
package, on Fedora and openSUSE the `.rpm`, and anywhere else the AppImage (use
`sh -s -- --appimage` to choose the AppImage yourself, without sudo).

Or download the file for your computer from the [latest release](https://github.com/bernardnongpoh/professor-os/releases/latest):

| Computer | File |
| --- | --- |
| Mac with Apple silicon (M1 or later) | `…_aarch64.dmg` |
| Mac with Intel | `…_x64.dmg` |
| Ubuntu / Debian | `…_amd64.deb` (or `_arm64.deb`) |
| Fedora / openSUSE | `….x86_64.rpm` (or `.aarch64.rpm`) |
| Other Linux | `…_amd64.AppImage` (or `_aarch64.AppImage`) |

> **macOS:** the app isn't signed with an Apple Developer ID yet. The install command above works
> without any warning. If you open the `.dmg` you downloaded instead, right-click Professor OS in
> Applications and choose **Open** the first time.

Requires macOS 13.3 or later, or a 64-bit Linux with WebKitGTK 4.1 (Ubuntu 22.04, Debian 12,
Fedora 38 or later).

## You need Claude Code or Codex

Professor OS does its thinking through an AI assistant you already pay for. It can't run without
one:

- **[Claude Code](https://claude.com/code)** with a Claude Pro or Max plan (or an Anthropic
  Console account). Recommended: it can also add confirmed plans to Google Calendar.
- **[Codex](https://developers.openai.com/codex)** with a ChatGPT Plus, Pro, Business or
  Enterprise plan.

You don't need to set these up beforehand. The first time Professor OS opens, it installs the one
you choose and signs you in, in a couple of clicks.

<p align="center">
  <img src="docs/screenshots/setup.png" width="640" alt="First run: choose Claude Code or Codex, install it and sign in">
</p>

## What it does

- **Just say it.** "Amit is my new PhD student working on compiler fuzzing" creates his page,
  the project, and the link between them. Paste a profile link and it fills in the details.
- **Pages for everything**: people (students, collaborators, colleagues), projects (planned, in
  progress, completed), courses, ideas, organizations and notes, each with an icon you pick.
- **Today**: your tasks by when they're due, quick add, and "Plan my day", which drafts a
  schedule you confirm before anything reaches your calendar.
- **Follow researchers**: new papers (OpenAlex), homepage changes and blog or GitHub posts, rated
  for how they relate to your own projects and ideas.
- **Search and commands** with ⌘K / Ctrl K, favorites, light and dark themes, accent colors.
- **Obsidian**: every page is also a Markdown file in `~/Documents/Professor OS`, and edits you
  make in Obsidian come back into the app.

<p align="center">
  <img src="docs/screenshots/project.png" width="410" alt="A project page with its icon and status">
  <img src="docs/screenshots/updates.png" width="410" alt="Updates from researchers you follow">
</p>

## Privacy

Your pages live on your computer: a local database and the Markdown folder above. When you chat,
your messages and the notes they need are sent to Anthropic (Claude Code) or OpenAI (Codex), as
with any use of those tools. The assistant only gets Professor OS's own tools: it can't run
commands, browse your files, or read your email.

## Uninstall

- **macOS:** delete Professor OS from Applications. Your data is in
  `~/Library/Application Support/com.professoros.app` and `~/Documents/Professor OS`.
- **Ubuntu / Debian:** `sudo apt remove professor-os`. **Fedora:** `sudo dnf remove professor-os`.
  **AppImage:** delete `~/.local/bin/professor-os` and
  `~/.local/share/applications/professor-os.desktop`.
- Data on Linux: `~/.local/share/com.professoros.app` and `~/Documents/Professor OS`.

## Build from source

You need [Rust](https://rustup.rs), Node.js 20+, and on Linux the
[Tauri prerequisites](https://v2.tauri.app/start/prerequisites/#linux).

```sh
npm install
npm run tauri dev                          # run it
(cd src-tauri && cargo test --lib)         # tests

# release build on macOS, with OpenSSL linked statically:
OPENSSL_DIR="$(scripts/static-openssl.sh)" npm run tauri build
```

Releases are built by GitHub Actions when a `v*` tag is pushed
(`.github/workflows/release.yml`).
