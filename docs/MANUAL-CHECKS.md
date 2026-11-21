# Manual checks

Checks that only the maintainer can run: on a real Mac, with a real OS keychain, through native
file dialogs, against a real server. They are tracked here, apart from phase status, so they
never hold up a phase. Phases keep moving in `docs/PLAN.md` while this list is worked through
whenever there is time.

How the list is kept (the rules for AI assistants are in `docs/AI-WORKFLOW.md`, *Manual checks*):

- When a phase is done, its manual checks are added under **Open**, grouped by phase.
- A check is ticked when the maintainer reports it passes, or when a bug found while running it
  is fixed. Ticked checks move to **Done** with the date and, for a fix, the PR number.
- A bug that turns up with no check behind it gets fixed the same way; it only touches this
  list if it settles a check.

## Open

### Phase 3 — Hosts, groups & storage

On a Mac:

- [ ] The Cmd shortcuts work: Hosts page (Cmd+Shift+H), quick connect (Cmd+O), new and close
      tab (Cmd+T, Cmd+W), save in the host editor (Cmd+Enter).
- [ ] In the host editor, Space and Enter press the focused checkbox and buttons.
- [ ] The hosts page and the host editor look right in dark mode.

### Phase 4 — Keychain

With a real OS keychain (the headless exit check kept the vault key in a file):

- [ ] Linux, GNOME Keyring: the vault key is stored in the keyring, no `vault.key` file appears
      in the data folder, and saved keys and passwords still work after restarting the app.
- [ ] macOS, Keychain: the same.

Native file dialogs, on Linux and on macOS:

- [ ] Import a key from a file through the open dialog.
- [ ] Export a key (OpenSSH, PKCS#8, public key) through the save dialog; the file is written.

Against a real server:

- [ ] Generate a key, add it with **Export to host**, reconnect and log in with it.
- [ ] Log in with a saved password and with an identity; restart the app and log in again
      without retyping anything.

### Phase 5 — Port forwarding

Against a real server:

- [ ] Add a Local rule to a database the server can reach (e.g. `localhost:15432` →
      `localhost:5432`), start it from Port forwarding → Rules, and connect a client (`psql -h
      localhost -p 15432`). The page shows the traffic and open connection.
- [ ] Add a SOCKS proxy rule, start it, and point a browser at it (Firefox: SOCKS5 on
      `localhost` and its port, with "Proxy DNS when using SOCKS v5"). Pages load as if
      browsing from the server; a name only the server can resolve works.
- [ ] Add a Remote rule (e.g. the server's `localhost:9000` → this machine's `localhost:3000`
      with something listening there), start it, and `curl localhost:9000` on the server.
- [ ] Set a rule to start when connecting, open a tab to the host: the page says "Active in a
      terminal tab", and closing the tab stops it.
- [ ] With a forward running, turn Wi-Fi off for half a minute and on again (or sleep the
      laptop): the forward shows "Waiting for the connection…", then Active, and works again.
- [ ] A rule on "All interfaces" is reachable from another machine on the network.

On Linux:

- [ ] A Local rule on a port below 1024 fails with a clear reason on the page.

On a Mac:

- [ ] The rule editor and the Port forwarding page look right in light and dark mode, and
      Cmd+Enter saves in the rule editor.

## Done

| Date | Phase | Check | Result |
|------|-------|-------|--------|
| 2026-11-20 | 4 | Key details panel on Linux: the bottom row of buttons shows for a long key | found during the Phase 4 checks, fixed in #55 |
