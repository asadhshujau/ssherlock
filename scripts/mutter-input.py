#!/usr/bin/env python3
"""Type and click in a headless GNOME Shell through org.gnome.Mutter.RemoteDesktop.

A development aid for checking the UI on real GNOME (mutter) without touching your
desktop; see docs/headless-wayland.md. Not shipped, not part of the build.

Set MUTTER_INPUT_BUS to the headless shell's private bus address. The script never
falls back to your session bus, so it can't type into your real desktop by mistake.
Commands come from the arguments, or one per line on stdin when there are none
(feed it from a FIFO to keep one virtual keyboard alive between commands):

  type:TEXT          type TEXT (ASCII; "\\n" is not interpreted, use key:Return)
  key:A+B+...        press a combination, e.g. key:ctrl+shift+c, key:Return
  home               move the pointer to the top-left corner
  move:DX,DY         move the pointer relatively; after `home` this is absolute
  down / up / click  left button
  wheel:N            scroll N steps (positive is down)
  sleep:SECS         wait
  quit               stop reading stdin

A freshly created virtual keyboard can drop its first key or two, so send
`key:shift` once before typing.
"""

import os
import sys
import time

import gi

gi.require_version("Gio", "2.0")
from gi.repository import Gio, GLib  # noqa: E402

KEYSYMS = {
    "ctrl": 0xFFE3,
    "shift": 0xFFE1,
    "alt": 0xFFE9,
    "super": 0xFFEB,
    "Return": 0xFF0D,
    "Escape": 0xFF1B,
    "Tab": 0xFF09,
    "BackSpace": 0xFF08,
    "Delete": 0xFFFF,
    "Insert": 0xFF63,
    "Up": 0xFF52,
    "Down": 0xFF54,
    "Left": 0xFF51,
    "Right": 0xFF53,
    "Page_Up": 0xFF55,
    "Page_Down": 0xFF56,
    **{f"F{n}": 0xFFBD + n for n in range(1, 13)},
}
BTN_LEFT = 0x110  # evdev code
SESSION = "org.gnome.Mutter.RemoteDesktop.Session"


def connect():
    """Connects to the bus named by MUTTER_INPUT_BUS, and only to that one."""
    address = os.environ.get("MUTTER_INPUT_BUS")
    if not address:
        sys.exit("set MUTTER_INPUT_BUS to the headless shell's bus (see docs/headless-wayland.md)")
    flags = (
        Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT
        | Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION
    )
    return Gio.DBusConnection.new_for_address_sync(address, flags, None, None)


bus = None


def call(path, iface, method, args=None):
    return bus.call_sync(
        "org.gnome.Mutter.RemoteDesktop", path, iface, method, args, None, 0, -1, None
    )


def keysym(session, sym, pressed):
    call(session, SESSION, "NotifyKeyboardKeysym", GLib.Variant("(ub)", (sym, pressed)))


def sym_of(name):
    if name in KEYSYMS:
        return KEYSYMS[name]
    if len(name) == 1:
        return ord(name)
    raise ValueError(f"unknown key {name}")


def run(session, cmd):
    op, _, arg = cmd.partition(":")
    if op == "type":
        for ch in arg:
            keysym(session, ord(ch), True)
            keysym(session, ord(ch), False)
            time.sleep(0.02)
    elif op == "key":
        syms = [sym_of(n) for n in arg.split("+")]
        for s in syms:
            keysym(session, s, True)
        for s in reversed(syms):
            keysym(session, s, False)
    elif op in ("home", "move"):
        dx, dy = (-10000.0, -10000.0) if op == "home" else map(float, arg.split(","))
        call(session, SESSION, "NotifyPointerMotionRelative", GLib.Variant("(dd)", (dx, dy)))
    elif op in ("down", "up", "click"):
        for pressed in {"down": [True], "up": [False], "click": [True, False]}[op]:
            call(session, SESSION, "NotifyPointerButton", GLib.Variant("(ib)", (BTN_LEFT, pressed)))
    elif op == "wheel":
        call(session, SESSION, "NotifyPointerAxisDiscrete", GLib.Variant("(ui)", (0, int(arg))))
    elif op == "sleep":
        time.sleep(float(arg))
    else:
        raise ValueError(f"unknown command {cmd}")


def commands():
    if sys.argv[1:]:
        yield from sys.argv[1:]
        return
    for line in sys.stdin:
        line = line.rstrip("\n")
        if line == "quit":
            return
        if line:
            yield line


def main():
    global bus
    bus = connect()
    session = call(
        "/org/gnome/Mutter/RemoteDesktop", "org.gnome.Mutter.RemoteDesktop", "CreateSession"
    ).unpack()[0]
    call(session, SESSION, "Start")
    try:
        for cmd in commands():
            try:
                run(session, cmd)
            except ValueError as error:
                print(error, file=sys.stderr, flush=True)
            time.sleep(0.05)
            print(f"done {cmd}", flush=True)
    finally:
        time.sleep(0.2)
        call(session, SESSION, "Stop")


if __name__ == "__main__":
    main()
