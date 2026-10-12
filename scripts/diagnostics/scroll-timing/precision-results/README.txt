High-resolution wheel input: physical capture and native bridge verification

Physical observation
--------------------
physical-wayland-summary.json summarizes actual mouse movements over a GTK3
window using two pointers on the SAME GTK-owned Wayland connection. This is
not a synthetic wheel test and does not read /dev/input. The observer is
../wayland_hires_observer.c; the user's existing CodeMux process was untouched.

Installed GTK3 3.24.52 binds wl_seat version 5. It preserves the legacy axis
value but cannot receive axis_value120 (introduced at version 8). Hyprland
0.56.2 accumulates high-resolution wheel input and rewrites the legacy axis
into full-notch deltas for compatibility; it sends the original fractional
value120 separately to version 8 pointers. This explains why animation changes
alone cannot recover the missing tiny mouse movements.

The paired observer received 2129 version 8 frames vs 276 GTK smooth events.
1852 frames had a zero legacy axis while value120 remained nonzero. Fractions
were +/-15 (one eighth of 120 per full notch), with occasional +/-30 packets.
GTK's emitted smooth deltas were always +/-1.5 full legacy notch units.

Primary source:
https://github.com/GNOME/gtk/blob/3.24.52/gdk/wayland/gdkdisplay-wayland.c#L244
https://github.com/GNOME/gtk/blob/3.24.52/gdk/wayland/gdkdevice-wayland.c#L1894
https://github.com/hyprwm/Hyprland/blob/v0.56.2/src/managers/input/InputManager.cpp#L1049
https://github.com/hyprwm/Hyprland/blob/v0.56.2/src/managers/SeatManager.cpp#L417
GTK4 already receives value120:
https://github.com/GNOME/gtk/blob/4.22.5/gdk/wayland/gdkseat-wayland.c#L1056

To repeat the physical observation (requires physical scrolling):
  gcc -Wall -Wextra -Wno-unused-parameter -O2 \
    scripts/diagnostics/scroll-timing/wayland_hires_observer.c \
    -o /tmp/codemux-wayland-hires-observer \
    $(pkg-config --cflags --libs gtk+-3.0 wayland-client)
  /tmp/codemux-wayland-hires-observer 60 > /tmp/codemux-wayland-hires.log

The optional number is the window lifetime in seconds. The window does not
request keyboard focus. It logs version 8 value120 frames beside GTK's legacy
scroll events and closes automatically.

Production bridge verification
------------------------------
Run from the repository after building the Rust debug target:
  python scripts/diagnostics/scroll-timing/native_precision_probe.py \
    --output /tmp/codemux-native-bridge.json

The Python runner reads the CURRENT src-tauri/src/precise_wheel.rs and appends
native_precision_probe_main.rs into a temporary Rust source. It compiles
against the application's existing debug dependencies, opens a non-focusing
mock WebKit window, sends 64 value120=15 frames through the production native
frame handler, and injects 8 corresponding legacy GTK notch events. No copy
of the production implementation is committed in the diagnostics. This test
needs no physical mouse input and closes automatically.

native-bridge.json records the result on this display:
  64 native fractions ->64 DOM wheel events, each 15.5px.
  Every event isTrusted=true and cancelable=true.
  8 legacy GTK duplicate events ->0 extra DOM events.
  Requested distance 992px; accumulated destination 992px.

The pixel gain depends on WebKit's viewport sizing. The runner checks event
count, trust, cancelability, uniform fractional deltas, total distance, and
final offset rather than hardcoding this monitor's 992px result. It uses a
floating destination accumulator to avoid per-event scrollTop rounding.

The replacement GDK event carries send_event=1 so its later native WebKit
propagation copy bypasses legacy duplicate filtering. WebKit's GTK native
wheel factory still creates trusted DOM input (verified above). The factory
has no send_event-based trust downgrade:
https://github.com/WebKit/WebKit/blob/webkitgtk-2.52.6/Source/WebKit/Shared/gtk/WebEventFactory.cpp#L315

Scope and limits
----------------
The bridge operates on the GTK connection and owned webview surfaces only.
It replaces explicit WHEEL source frames with value120 and an axis timestamp;
touchpads, wheel-tilt source, unsupported/incomplete frames, X11, old compositor
or incompatible core protocol tables, and multi-seat displays keep GTK input. An opt-out
is CODEMUX_PRECISE_WHEEL=0. Normalization is value120/120, matching wheel units
rather than retaining Hyprland's compositor-specific1.5-unit legacy gain.

This restores event precision, not hardware-vsync timing or mouse inertia.
A short proportional frontend glide remains a separate policy. A capability
flag reports protocol support, not the resolution of every attached device.
Duplicate timestamps are retained for 4096 legacy-axis frames (16KiB per
webview); an exceptionally stalled native event batch beyond that bound can
exceed the duplicate history. Normal GTK dispatch drains the queue continuously.

Combined frontend verification
------------------------------
With an already-running Vite server, the same mock page can import the actual
frontend wheel handler, install it with native precision available, and check
its completed destination:
  python scripts/diagnostics/scroll-timing/native_precision_probe.py \
    --frontend-url http://localhost:1431 \
    --output /tmp/codemux-native-and-frontend.json

native-and-frontend.json records 64 trusted/cancelable fractional events, all 64
prevented by the actual frontend handler, input total 992px, and final offset 992px.
Its standalone WebKit process uses the original display clock. To compose with
the production mode-paced vblank fallback without changing system configuration,
first build that diagnostic .so using ../build_native_probe.py (see the main
README), then add:
  --clock-preload /tmp/codemux-scroll-timing/native_vblank_fallback.so

native-frontend-mode-clock.json records the same exact completed distance with
the mode-paced clock. This option affects only the temporary probe process.
These pages use mock content and never inspect user conversations.

AppImage bundled-client compatibility
-------------------------------------
The release workflow builds on Ubuntu 22.04 and normal CodeMux AppImages bundle
Wayland 1.20 client protocol tables at version 7. The bridge now supplies PRIVATE
version-8 seat/pointer descriptions for its new objects on that runtime. It
copies the original nine pointer event descriptions and adds axis_value120
(name axis_value120, signature 8ui, unsigned axis / signed value). Seat requests
and events are unchanged. Stable boxes are shared by Bridge and private data
on GdkDisplay, keeping descriptions alive through proxy destruction and queued
event cleanup until GTK closes its connection. GTK's original global descriptions and
existing version-5 pointer objects are untouched. Newer libraries retain their
existing descriptions.

appimage-v7-private-v8.json records nine passing tests with the actual bundled
libwayland-client.so.0 extracted from a local CodeMux AppImage. A socketpair mock
server sent WHEEL + axis/time + value120 (+15, -15) + frame packets through the
REAL older client marshaller and event decoder. The decoded signed fractions
were +0.125 and -0.125, with no protocol error or compositor/device/GUI access.
The result records the library SHA256 and confirms both runtime table versions
are 7 while the private objects support version 8.

Reproduce with an extracted bundled library:
  python scripts/diagnostics/scroll-timing/native_protocol_probe.py \
    --client-library /path/to/AppDir/usr/lib/libwayland-client.so.0 \
    --output /tmp/codemux-appimage-protocol.json

Without --client-library it runs the same production metadata/wire tests on
the host client. The supplied library is preloaded ONLY for the temporary test
process. No library is installed/replaced and no system settings are changed.

Primary release/package evidence:
https://packages.ubuntu.com/jammy/libwayland-client0
https://wayland.freedesktop.org/releases/wayland-1.20.0.tar.xz

The Wayland 1.20 marshaller explicitly accepts the constructor interface and
version arguments; decoding uses each proxy's interface event descriptions.
The compatibility test exercises that behavior instead of assuming that newer
protocol metadata requires a newer generic runtime engine.
