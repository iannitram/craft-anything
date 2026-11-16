# Releasing Craft Anything

The recipe is shared by every Crafting App: `release/playbook.md` in
[craftrules](https://github.com/storytold/craftrules) (`../craftrules`), and the runbook for cutting
one is `release/cut-a-release.md` there. That repo is private; you don't need it to follow this
page. In short: a release is whatever is on the `release` branch. Pushing to it makes
`.github/workflows/release.yml` build, sign and package every platform and attach the files to a
draft GitHub Release, which a maintainer publishes once every job is green. Signing needs the
maintainers' certificates (secrets of the `release` environment, which only the `release` branch can read), so a fork's run produces unsigned packages.

`ci.yml` checks every pull request and every push to `main` once the repository is public; while
it's private its jobs are skipped (they would cost Actions minutes), so run it by hand before a
release: `gh workflow run ci.yml --ref main`. The other workflows (`freebsd.yml`,
`windows-arm64.yml`, `packaging-lint.yml`) run by hand (`gh workflow run <file>`).

## Cutting a release

1. Bump the version on `main`: `cargo xtask version set X.Y.Z` (the workspace version in the root
   `Cargo.toml` is the single source; `cargo xtask version` prints it).
2. Fast-forward `release` to `main`. The push runs `.github/workflows/release.yml`, which builds every
   platform and creates or updates the draft GitHub Release **Craft Anything vX.Y.Z**.
3. When every job is green, run the packaged Deep-build check below on macOS and Windows.
   Publish the draft only after both pass.
4. After publication, follow the ticket checklist below.

A dry run of the unsigned platforms from any branch: `gh workflow run release.yml --ref <branch>`.

## Before publishing: a Deep build from the packages

Run this check on a Mac and on Windows using the finished packages attached to the draft release.
Starting the packaged CLI alone checks that it can launch (#91); a real Claude Code build also
checks that Claude Code accepts its MCP tools (#490). A source build or a CLI found on PATH does
not check what the package ships.

1. Install or extract the candidate package and open its desktop app. On macOS use the DMG's
   `Craft Anything.app`; on Windows use the MSI installation or extracted portable zip, recording
   which one. Check the bundled CLI's `--version` by its full path: beside the app executable on
   Windows, or `Craft Anything.app/Contents/MacOS/craft-anything-cli` on macOS. It must match the
   candidate version. Keep the app and CLI together throughout the check.
2. Use a local, signed-in Claude Code as the builder. Record its version and the OS version.
   In Craft Anything select Deep mode and start a fresh project from a small app you own, with a
   concrete request whose result you can build and launch. Keep Auto mode off so a request that
   needs your permission can be checked.
3. Ask the builder to call `core_session_show` and report the selected mode before building.
   Verify the tool result reports Deep mode. Also verify that the Craft MCP server used by this
   run is the bundled CLI from step 1, using Claude Code's MCP log if necessary. A server from
   another installation does not pass this check.
4. Ask the builder to perform a write or command that reaches `core_action_approve` and displays
   an approval card in Craft Anything. Answer **Allow once** on that card and confirm the builder
   continues with the approved action. A displayed and answered card is required: automatic
   approvals for ordinary app edits or known build commands cannot satisfy this step. See
   [the Claude Code integration](claude-code.md) for the approval rules.
5. Let the run finish its build and checks, then use Play and inspect the requested behaviour.
   Pass only if the Craft tool call succeeded, the approval card appeared and was answered with
   **Allow once**, the builder continued, the run reached Ready and the result launches and does
   what was requested. Missing tools, a server that cannot start, a missing or stuck
   approval or a failed check must be resolved and this check repeated before publication.

Record each platform's package filename, candidate version and commit, OS and Claude Code
versions, request, tool result, displayed card and its answer, builder continuation, build/check
outcome and Play result in the release
PR or release validation notes. Keep credentials and personal paths out of the record. If a
platform has not been checked, record it as pending and leave the release in draft.

## After publishing: close tickets waiting for the release

A fix on `main` does not help someone still using an older release. Close tickets waiting for
that fix only after the release containing it has been published.

1. Find every open ticket marked as fixed but waiting for a release. Check the open issues,
   the roadmap's release follow-ups and the fixes included since the previous release; do not
   rely on a `fixed` label alone.
2. For each ticket, verify that the published release contains its fix. Compare the fix commit
   with the published tag (`git merge-base --is-ancestor <fix-commit> <release-tag>`), rather
   than checking today's `main`. Read the ticket's reported version and symptoms before
   deciding it is covered. Keep tickets open when their cause is still uncertain, the fix is
   absent from the release or related work remains.
3. Close each covered ticket with a comment naming the published version, linking to its
   release page and the fix, and asking the person to file again if the problem persists after
   updating. Say whether the reported case was reproduced or the fix was checked another way;
   do not claim a live check that was not run.
4. Update the roadmap's release follow-ups with what was closed and leave an unticked line
   for anything still waiting. Record unresolved symptoms or missing validation under the
   relevant item so the next release does not lose them.

## What it ships

| Platform | Files |
|---|---|
| macOS | `craft-anything-<v>-macos-universal.dmg` (`Craft Anything.app`), `craft-anything-cli-<v>-macos-universal.zip` |
| Windows | `craft-anything-<v>-windows-{x64,x86,arm64}.msi` and `-portable.zip` |
| Linux | `craft-anything-<v>-linux-{x86_64,aarch64}.{AppImage,AppImage.zsync,deb,rpm,tar.gz,flatpak}`, `craft-anything-<v>-linux-riscv64.tar.gz` |
| FreeBSD | `craft-anything-<v>-freebsd-x86_64.tar.gz` |
| Web | `craft-anything-web-<v>.zip` (the browser viewer, `apps/craft-anything-web`) |

Plus `SHA256SUMS.txt`.

## App-specific choices

- **No file associations.** Craft Anything opens any file, so it claims no file type and no URL
  scheme on any platform. It appears in Windows' "Open with" list and accepts paths on the
  command line. Projects (`*.cany`) are folders.
- **The macOS executable** is `Craft Anything.app/Contents/MacOS/craft-anything`. Only the bundle
  carries the display name. `craft-anything-cli` sits beside it, as it does in the Windows zip: the
  app starts it as the coding agents' MCP server, so an app without it can't build (#91).
- **Windows portable mode:** the portable zip ships `portable.txt`. While it sits next to the exe,
  settings live in `CraftAnythingData\` (`crates/engine/src/settings.rs`).
- **Flatpak** grants `--share=network` (AI backends, engine downloads) and `--filesystem=home:ro`
  (files to inspect, passed by path), plus `--filesystem=xdg-documents/Craft Anything:create` so the
  default projects folder can be made and written (#505).
- **Fonts** are committed in `assets/fonts` (OFL-1.1). Release and CI builds also embed craft-fonts
  at the commit pinned in `release.yml` and `ci.yml` (`CRAFT_FONTS_DIR`, `CRAFT_FONTS_REQUIRED=1`),
  the fallbacks for Korean, Chinese, Japanese, Thai, Arabic, Hebrew, Devanagari and symbols; a
  build without them shows those as boxes (#19). With `CRAFT_FONTS_REQUIRED=1` the build fails
  when `CRAFT_FONTS_DIR` is missing or bad, so set both. Every package ships all their licences
  (`OFL-*.txt`, craft-fonts' as `OFL-<font>.txt`).
- **The WiX UpgradeCode** `8F69A8B5-4384-479B-B3FF-93C5374BCA51` belongs to Craft Anything for good.
