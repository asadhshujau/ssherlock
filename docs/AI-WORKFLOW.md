# Working notes for AI assistants

How the maintainer wants AI coding assistants (Claude Code and others) to work on this repo.
`CLAUDE.md` imports this file, so Claude Code loads it on every machine.

The maintainer works from several PCs. An assistant's local memory doesn't follow them between
machines, so **standing preferences and corrections go in this file**, committed to master.
Local memory is fine as a cache, never as the only copy. Phase status lives in the progress
table in `docs/PLAN.md`; feature status lives in `docs/FEATURES.md`.

## Phases

- **Don't start a phase until the maintainer says so.** When one is done, update `PLAN.md` and
  `FEATURES.md`, then stop and wait.
- Mark a phase done only after its exit criteria pass. Items that are partly done stay `[~]`,
  with an italic note under them saying what's left and which phase finishes it.

## Branches and PRs

- Each step of a phase goes on its own branch and is opened as a GitHub PR with `gh pr create`.
  The PR body follows CLAUDE.md §13: what, why, how tested, which FEATURES boxes. A step that
  depends on an unmerged PR is stacked on that PR's branch.
- Once a step passes `just check`, commit, push and open its PR without asking again.
- **Don't merge PRs as they're opened.** Keep stacking until the phase's work is done. Then
  give the maintainer a checklist of manual checks, merge the whole stack in order, and only
  then mark the phase done.
- Commit straight to master only when the maintainer explicitly asks, as they did for some
  docs-only changes.
- **Merging a stack:** use merge commits, not squash. For each PR in order:
  1. Merge it: `gh api -X PUT repos/asadhshujau/ssherlock/pulls/N/merge -f merge_method=merge`.
  2. Retarget the next PR to master:
     `gh api -X PATCH repos/asadhshujau/ssherlock/pulls/NEXT -f base=master`.
  3. Only then delete the merged branch.

  Deleting a base branch first makes GitHub close the next PR instead of retargeting it.
  `gh pr edit --base` and `gh pr view` fail on this repo with a GraphQL error about the
  deprecated Projects (classic) API, so use `gh api`.
- Before merging, check CI on **every** PR in the stack, macOS included. A macOS-only build
  break once went unnoticed across eight stacked PRs (#13–#20).
- `gh pr checks` here has no `--json` flag; parse its tab-separated output. Docs-only PRs
  get no CI checks.

## Platforms

The maintainer develops on **macOS and Linux** (Ubuntu with GNOME on Wayland). Plans, exit
criteria, CI and setup docs must cover both, never only one. Windows support is undecided.

## Stay inside the repo

Ask before reading or running anything outside the repository: dependency sources in
`~/.cargo/registry`, files under `$HOME`, the app's real data directory
(`~/.local/share/ssherlock`), and so on. Normal `cargo build`/`test` inside the repo is fine.
Work from compiler errors, docs in the repo and crate docs you already know. When outside
information would really help, ask first.

If the maintainer grants a standing exception, record it here with its scope. During Phase 1
they allowed reading docs.rs and the registry sources of `alacritty_terminal`, `portable-pty`
and `gpui`. That exception ended with Phase 1. During Phase 2 they allowed docs.rs for `russh`,
which ended with Phase 2.

Standing exceptions:

- **`~/.cargo`, any time.** Reading anything under `~/.cargo`, such as dependency sources in
  `~/.cargo/registry/src/`, needs no asking (granted 2026-11-15).

## Test servers, never real ones

Test SSH against servers you start yourself: the Docker OpenSSH container in
`crates/ssherlock-ssh/tests/docker/`, or an in-process russh server inside a test. Never connect
to a real server. Don't look on the maintainer's machine for hosts, `~/.ssh/config`,
`known_hosts`, keys or a running agent to try. Checks that need a real server go on the manual
checklist given at the end of a phase.

## Check the UI yourself

When a change affects what's on screen, launch the app headlessly, look at a screenshot and
drive it yourself before reporting. Don't hand visual checks back to the maintainer. If a tool
is missing, give them the one-line install command (there is no passwordless sudo). Tell them
only what you truly can't do yourself, with steps to do it.

- X11 / rendering: README, *Checking the UI headlessly*.
- Wayland (weston, or a headless GNOME Shell matching the maintainer's desktop):
  `docs/headless-wayland.md`, with `scripts/mutter-input.py` for input.
- Always sandbox: own `HOME`/`XDG_DATA_HOME`, `SHELL=/bin/sh`,
  `DBUS_SESSION_BUS_ADDRESS=disabled:` for the app, and stub `xdg-open`/`gio`/… first on
  `PATH`. Without them, a headless Ctrl+click once opened a link in the maintainer's real
  browser. Also unset `SSH_AUTH_SOCK` and `SSH_AGENT_PID`, or the app can use the maintainer's
  real SSH agent when it logs in to a test server.
- Wait for the window with `xdotool search --sync`, not a fixed sleep.

## Shell gotchas

- The Bash tool runs zsh. Run bash-only syntax such as `"${!arr[@]}"` under `bash -c`.
- Never `pkill -f` a pattern that also matches your own command line; kill exact PIDs.
