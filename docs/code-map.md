# Code map

Updated: 2026-11-18

This is a fork of `storytold/craft-anything` (MIT OR Apache-2.0). The code is upstream's; the spec and roadmap are upstream's `docs/ROADMAP.md`. This page maps the workspace for a session that needs to find its way, and lists what this fork changed. Upstream's own docs (`docs/STATUS.md`, `docs/extending.md`, `docs/tools.md`, `docs/pitfalls.md`) go deeper on each part.

Fork state on 2026-11-18: `origin/main` is 355 commits behind upstream's `main` (`b60cca57`, 2026-11-18). No sync PR is open. Our own commits are the list under "What this fork changed".

## Layout

| Path | Owns | State |
|---|---|---|
| `crates/formats` | Pure file-format parsers, no workspace dependencies | present |
| `crates/evidence` | Evidence records, canonical ids, confidence, unknowns, the error contract | present |
| `crates/disasm` | The native engine: decoding, functions, control flow, references, pseudo-C | present |
| `crates/js` | JavaScript, Electron and web-bundle analysis | present |
| `crates/analysis` | Analyzers that turn parsed targets into evidence | present |
| `crates/engines` | Adapters for optional external engines (Ghidra, adb, Chromium, Frida, JADX) | present |
| `crates/platform` | Native services: owned processes, capture, sandbox, engine store, `fs::write_atomic` | present |
| `crates/ai` | AI backends, the agent loop, crews, the build loop | present |
| `crates/engine` | Session, projects, the tool registry every surface dispatches through, settings, jobs | present |
| `crates/mcp` | The MCP server | present |
| `crates/setup` | Connect the app to coding-agent apps: detect, plan, merge, back up, roll back, doctor | present |
| `crates/ui-egui` | The egui interface: Beginner and Experienced modes | present |
| `crates/testkit` | Test helpers and synthetic sample builders | present |
| `apps/craft-anything`, `apps/craft-anything-cli`, `apps/craft-anything-web` | The desktop app, the command line (every tool, MCP, setup, doctor), the browser build | present |
| `xtask/` | Repo checks: `layers`, `tools`, `wasm`, `secrets`, `ci`, `version` (`AGENTS.md`) | present |
| `packaging/` | `windows` (WiX, signing), `macos` (dmg, notarisation, entitlements), `linux` (AppImage, Flatpak, nfpm), `freebsd`, `web` | present |
| `vendor/wgpu-hal` | A copy of `wgpu-hal` with a Metal shader-language probe (`vendor/wgpu-hal/CRAFT_ANYTHING.md`) | present |
| `docs/` | Upstream's roadmap, status, tool lists, licences, brand | present |
| `PIPELINE.md`, `CHANGELOG.fork.md`, this file | Ours: the fork's stage, our changelog, this map | ours |

## What this fork changed

Everything below is in our commits only (`git log upstream/main..origin/main`). Each fix is ported upstream after it merges here (`PIPELINE.md`).

- **Build loop (R2.9):** fresh builder per loop run through one tested `run_options`; pause, "Send now", stall and token limits; CLI settings re-read after each run; a builder with no session gets the brief on every in-run turn; a builder that stops without saying so ends its turn; a stalled build finishes as Ready when the work is done. Files: `crates/ai/src/rebuild/`, `crates/ui-egui/src/build_loop.rs` and `project.rs`, `crates/engine/src/tools/project_tools.rs`, `apps/craft-anything-cli/src/rebuild_cmd.rs` (`rebuild --loop`).
- **Codex:** continues its earlier session instead of starting fresh each message; a retry that cannot resume resends the original message. Files: `crates/ai/src/cli_agents/`.
- **Qwen Code:** runs over ACP so approvals apply and it gets our tools; its model is passed and offered. Files: `crates/ai/src/cli_agents/adapters/qwen.rs`.
- **OpenCode:** the 1.x error tells the person to remove npm `opencode-ai` before installing OpenCode 2; documented in `docs/opencode.md`.
- **A project's builder gets only its own target;** the program a project's launcher starts counts as its target.
- **CI:** the nightly FreeBSD run is off in this fork (`7c4a7a63`).
- **Review fixes** (`fixed-in-review:` commits): popover spacing, i18n-checkable status strings, a stand-in cargo in the CLI test, `ControlSize` in the settings button.

## Mac, PC and Linux (R04)

| Platform | Packaging | Notes |
|---|---|---|
| Windows (x86_64; arm64 has its own workflow) | `packaging/windows` (WiX, `sign.ps1`) | Windows 11 live checks are tracked in `docs/ROADMAP.md`. |
| macOS | `packaging/macos` (dmg, notarisation, entitlements) | The Dock launch finds a Codex CLI bundled in ChatGPT.app or Codex.app (`docs/STATUS.md`). |
| Linux | `packaging/linux` (AppRun, Flatpak, nfpm) | |
| FreeBSD | `packaging/freebsd` | The nightly run is off in this fork. |
| Web | `packaging/web` | WebGPU with a WebGL2 fallback. |

CI and releases here call shared workflows in `storytold/craft-common-actions` (`ci.yml` and `release.yml` are one `uses:` line each), so the OS matrix is not in this tree; I have not checked it.

`PIPELINE.md` lists the fork's platforms as mac, windows, linux.

## Open-source pieces

Upstream's list is `NOTICE`, `ATTRIBUTION.md` and `docs/licences.md`; this fork adds none.

## Peers

None are listed for this repo yet (`PIPELINE.md`: "Peers reviewed: not yet").
