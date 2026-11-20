# Working notes for AI assistants

How the maintainer wants AI coding assistants (Claude Code and others) to work on this repo.
`CLAUDE.md` imports this file, so Claude Code loads it on every machine.

The maintainer works from several PCs. An assistant's local memory doesn't follow them between
machines, so **standing preferences and corrections go in this file**, committed to master.
Local memory is fine as a cache, never as the only copy. Phase status lives in the progress
table in `docs/PLAN.md`; feature status lives in `docs/FEATURES.md`; the manual checks still
to run live in `docs/MANUAL-CHECKS.md`.

## Phases

- **Don't start a phase until the maintainer says so.** When one is done, update `PLAN.md` and
  `FEATURES.md`, then stop and wait.
- Mark a phase done only after its exit criteria pass. Items that are partly done stay `[~]`,
  with an italic note under them saying what's left and which phase finishes it.
- **Manual checks don't hold up a merge or the next phase** (since Phase 4, 2026-11-16). Once
  the headless exit checks pass, merge the stack and mark the phase done. Its manual checks go
  in `docs/MANUAL-CHECKS.md`, not in the `PLAN.md` progress row (see *Manual checks* below).

## Manual checks

Manual checks are tracked on their own in `docs/MANUAL-CHECKS.md` (since 2026-11-20), so the
maintainer can choose at any time between building the next phase and working through checks.

- **When a phase is done**, add its manual checks under *Open* in that file, grouped by phase,
  each one a step the maintainer can follow. Do it in the PR that marks the phase done.
- **When the maintainer reports a check passes**, tick it and move it to *Done* with the date.
- **When the maintainer reports a bug**, even in the middle of a phase and even if it belongs
  to an old phase, fix it then. The fix goes on its own branch off master with its own PR, not
  on the phase's stack, and carries a regression test (CLAUDE.md §10). If the bug came from a
  check on the list, or the fix settles one, tick that check in the same PR and note the PR
  number in *Done*. Then go back to the phase.
- Never tick a check because a headless run passed. These are the checks a headless run can't
  cover.

## Answering "what's to do?"

Read `docs/PLAN.md`, `docs/FEATURES.md` and `docs/MANUAL-CHECKS.md` fresh each time; they may
have changed on another PC.

- **A general question** ("what's to do?", "what's pending?"): show both the open manual
  checks and the phase in progress or the next one, with any open PRs.
- **A specific question**: answer only that. "Which manual checks are pending?" gets the open
  checks. "What phase are we on?" or "what's the next phase?" gets the phase.

## Branches and PRs

- Each step of a phase goes on its own branch and is opened as a GitHub PR with `gh pr create`.
  The PR body follows CLAUDE.md §13: what, why, how tested, which FEATURES boxes. A step that
  depends on an unmerged PR is stacked on that PR's branch.
- Once a step passes `just check`, commit, push and open its PR without asking again.
- **Don't merge PRs as they're opened.** Keep stacking until the phase's work is done. Then
  add the phase's manual checks to `docs/MANUAL-CHECKS.md`, merge the whole stack in order,
  and only then mark the phase done.
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
`known_hosts`, keys or a running agent to try. Checks that need a real server go in
`docs/MANUAL-CHECKS.md`.

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
