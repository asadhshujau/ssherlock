# Checking the UI headlessly on Wayland

The README's *Checking the UI headlessly* section runs the app on a virtual X display. That
covers rendering, but not the Wayland backend, which most Linux desktops use now. Clipboard,
focus and keyboard bugs can show up on Wayland only. This page has two ways to run SSHerlock
as a real Wayland client without touching your desktop. Both were used to verify Phase 1 on
GNOME 50.

Extra packages:

```sh
sudo apt install weston wl-clipboard      # Debian, Ubuntu
sudo dnf install weston wl-clipboard      # Fedora
sudo pacman -S weston wl-clipboard        # Arch
```

Use the sandboxing from the README here too: own `HOME`/`XDG_DATA_HOME`,
`SHELL=/bin/sh`, `DBUS_SESSION_BUS_ADDRESS=disabled:` and stub openers on `PATH`. The
snippets below assume `$sandbox` and `$sandbox/stubs` exist as set up there.

A Wayland socket path must be under 108 bytes, so keep `XDG_RUNTIME_DIR` short, for example
`/tmp/ssh-wl`.

## Quick: weston nested in Xvfb

Running weston with its X11 backend inside Xvfb means `xdotool` and `import` still work.
Input goes to weston's X window and weston passes it to its Wayland clients.

```sh
run=/tmp/ssh-wl; mkdir -p -m 700 "$run"
Xvfb :98 -screen 0 1400x900x24 & xvfb=$!
sleep 1
env -u WAYLAND_DISPLAY DISPLAY=:98 XDG_RUNTIME_DIR="$run" DBUS_SESSION_BUS_ADDRESS=disabled: \
  HOME="$sandbox" weston --backend=x11 --width=1280 --height=800 --socket=wl-test --idle-time=0 &
weston=$!
sleep 3
env -u DISPLAY WAYLAND_DISPLAY=wl-test XDG_RUNTIME_DIR="$run" HOME="$sandbox" \
  XDG_DATA_HOME="$sandbox/share" SHELL=/bin/sh DBUS_SESSION_BUS_ADDRESS=disabled: \
  PATH="$sandbox/stubs:$PATH" RUST_LOG=ssherlock_ui=debug \
  VK_DRIVER_FILES=/usr/share/vulkan/icd.d/lvp_icd.json ./target/debug/ssherlock & app=$!
sleep 5
DISPLAY=:98 xdotool mousemove 700 400 click 1 type 'echo hello'
DISPLAY=:98 import -window root target/wayland.png
# Another Wayland client reading the clipboard:
WAYLAND_DISPLAY=wl-test XDG_RUNTIME_DIR="$run" wl-paste --list-types
kill "$app" "$weston" "$xvfb"
```

weston has no primary-selection support, so `wl-paste --primary` fails there. That isn't a bug.

## Real GNOME: a headless GNOME Shell

GNOME's compositor (mutter) is stricter than weston about clipboard and focus. To test against
it, run a second, headless GNOME Shell on a private session bus, with its own runtime, home and
config directories.

```sh
gs=/tmp/ssh-gs; mkdir -p -m 700 "$gs"
mkdir -p "$sandbox/gnome/.config" "$sandbox/gnome/.local/share" "$sandbox/gnome/.cache"
cat > "$sandbox/gshell.sh" <<EOF
#!/bin/sh
echo "\$DBUS_SESSION_BUS_ADDRESS" > "$sandbox/gbus"
exec gnome-shell --wayland --headless --virtual-monitor 1280x800 --wayland-display=wl-gs
EOF
chmod +x "$sandbox/gshell.sh"
env -u DISPLAY -u WAYLAND_DISPLAY -u DBUS_SESSION_BUS_ADDRESS XDG_RUNTIME_DIR="$gs" \
  HOME="$sandbox/gnome" XDG_CONFIG_HOME="$sandbox/gnome/.config" \
  XDG_DATA_HOME="$sandbox/gnome/.local/share" XDG_CACHE_HOME="$sandbox/gnome/.cache" \
  XDG_SESSION_TYPE=wayland dbus-run-session -- "$sandbox/gshell.sh" > "$sandbox/gshell.log" 2>&1 &
sleep 8
export DBUS_SESSION_BUS_ADDRESS=$(cat "$sandbox/gbus")   # the private bus, for the steps below
```

Start the app with `WAYLAND_DISPLAY=wl-gs XDG_RUNTIME_DIR="$gs"` and the usual sandboxing.
The app keeps `DBUS_SESSION_BUS_ADDRESS=disabled:`; only the helper tools use the private bus.

**Input.** There is no X server to send events to. Instead, `scripts/mutter-input.py` drives
mutter's RemoteDesktop D-Bus API on the private bus. It connects only to the bus named in
`MUTTER_INPUT_BUS` and never to your session bus. On your session bus it would type into your
real desktop. To keep one virtual keyboard for the whole
test, feed it through a FIFO:

```sh
mkfifo "$sandbox/in"
sleep infinity > "$sandbox/in" &          # keeps the FIFO open between commands
MUTTER_INPUT_BUS=$(cat "$sandbox/gbus") \
  python3 -I scripts/mutter-input.py < "$sandbox/in" > "$sandbox/input.log" &
send() { for c in "$@"; do printf '%s\n' "$c" > "$sandbox/in"; done; }
send key:shift home move:600,400 click 'type:echo hello' key:Return
```

Write lines with `printf '%s\n'`, not zsh's `echo`, which turns `\033` into a raw escape byte.

**Screenshots.** `org.gnome.Shell.Screenshot` refuses callers unless the shell is in unsafe
mode. Turn it on once through Looking Glass:

```sh
send key:alt+F2 sleep:1 type:lg key:Return sleep:2 \
  'type:global.context.unsafe_mode = true' key:Return sleep:1 key:Escape
gdbus call --session -d org.gnome.Shell -o /org/gnome/Shell/Screenshot \
  -m org.gnome.Shell.Screenshot.Screenshot false false "$PWD/target/gnome.png"
```

**Clipboard.** `WAYLAND_DISPLAY=wl-gs XDG_RUNTIME_DIR="$gs" wl-paste` (and `--primary`)
reads it as another Wayland app would. For X11 apps, Xwayland starts with the shell. Its
display number is in `gshell.log` ("Using public X11 display :N") and its auth file is
`$gs/.mutter-Xwaylandauth.*`, so `DISPLAY=:N XAUTHORITY=… xclip -o -selection clipboard`
works.

**Side effects.** The headless shell starts its own gvfs, which tries to mount USB phones and
drives. Your real session already holds them, so this fails harmlessly, but it shows in the
log. Stop the shell with `kill` on its `gnome-shell` process. `dbus-run-session` then takes the
private bus and everything started on it down too.

On macOS there is no equivalent: the app runs on the real display (see the README).
