#!/usr/bin/env python3
"""Compare synthetic Mouse and Touchpad-source GTK smooth wheel events.

Run with /usr/bin/python wheel_source_probe.py [--output /tmp/wheel-source.json].
Requires GTK3/WebKit2 4.1 PyGObject on a running Linux graphical session. Opens
only an isolated mock document. Constructs a temporary Touchpad GdkDevice;
never modifies an existing device. Real wheel events are blocked before WebKit,
and only timestamp-zero synthetic events are sampled. No user/app data is read.
The results validate source classification, not physical touchpad performance.
"""
import argparse
import json
import os
from pathlib import Path
import re

# The isolated fixture needs SHM on the NVIDIA machine used for these probes.
os.environ.setdefault("WEBKIT_DMABUF_RENDERER_FORCE_SHM", "1")
import gi

gi.require_version("Gdk", "3.0")
gi.require_version("Gtk", "3.0")
gi.require_version("WebKit2", "4.1")
from gi.repository import Gdk, GLib, GObject, Gtk, WebKit2


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=Path("/tmp/wheel-source.json"))
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[3]
    source = (root / "src/lib/wheel-scrolling.ts").read_text()
    classifier = re.search(
        r"export function isWebKitMouseWheel\(event: WheelEvent\): boolean \{.*?\n\}",
        source, re.S,
    ).group(0).replace("export ", "").replace(": WheelEvent", "").replace(": boolean", "")

    manager = WebKit2.UserContentManager()
    manager.register_script_message_handler("probe")
    view = WebKit2.WebView(user_content_manager=manager)
    view.get_settings().set_enable_smooth_scrolling(False)
    window = Gtk.Window(title="CodeMux synthetic wheel source diagnostic")
    window.set_default_size(700, 700)
    window.add(view)
    pointer = Gdk.Display.get_default().get_default_seat().get_pointer()
    assert pointer.get_source() == Gdk.InputSource.MOUSE
    # input-source is construct-only: create a separate unregistered device.
    touchpad = GObject.new(
        pointer.__gtype__, name="CodeMux temporary touchpad source",
        type=Gdk.DeviceType.SLAVE, input_source=Gdk.InputSource.TOUCHPAD,
        display=pointer.get_display(),
    )
    assert touchpad.get_source() == Gdk.InputSource.TOUCHPAD
    view.connect("scroll-event", lambda _, event: event.time != Gdk.CURRENT_TIME)
    rows = []
    samples = [(kind, raw) for kind in ("mouse", "touchpad") for raw in
               (.001, .009, .08, .125, .126, .5, 1.234, 8, -.001, -.009, -.125, -1.234)]
    index = 0
    finished = False
    html = ("<html><body style='margin:0;height:8000px'>"
            "<div style='padding:80px;font:18px sans-serif'>Synthetic wheel source test</div>"
            "<script>" + classifier + """;
      window.addEventListener('wheel', event => {
        event.preventDefault();
        window.webkit.messageHandlers.probe.postMessage(JSON.stringify({
          deltaY: event.deltaY, wheelDeltaY: event.wheelDeltaY,
          deltaMode: event.deltaMode, isTrusted: event.isTrusted,
          mouse: isWebKitMouseWheel(event), innerHeight, devicePixelRatio
        }));
      }, {passive:false});
    </script></body></html>""")

    def finish():
        nonlocal finished
        if finished:
            return False
        finished = True
        payload = {
            "webkit": ".".join(str(get()) for get in (
                WebKit2.get_major_version, WebKit2.get_minor_version, WebKit2.get_micro_version)),
            "gtk_height": view.get_allocated_height(),
            "temporary_source": "TOUCHPAD",
            "samples": rows, "expected_count": len(samples), "actual_count": len(rows),
        }
        args.output.write_text(json.dumps(payload, indent=2) + "\n")
        print(json.dumps(payload), flush=True)
        window.destroy()
        Gtk.main_quit()
        return False

    def inject():
        if index >= len(samples):
            return finish()
        kind, raw = samples[index]
        event = Gdk.Event.new(Gdk.EventType.SCROLL)
        event.window = view.get_window()
        event.send_event = True
        event.time = Gdk.CURRENT_TIME
        event.x, event.y = 300, 200
        event.direction = Gdk.ScrollDirection.SMOOTH
        event.delta_x, event.delta_y = 0, raw
        event.set_device(pointer)
        event.set_source_device(pointer if kind == "mouse" else touchpad)
        view.event(event)
        return False

    def receive(_, value):
        nonlocal index
        if index >= len(samples):
            return
        row = json.loads(value.get_js_value().to_string())
        row["source"], row["raw_delta"] = samples[index]
        rows.append(row)
        index += 1
        GLib.timeout_add(70, inject)

    def loaded(_, event):
        if event == WebKit2.LoadEvent.FINISHED:
            GLib.timeout_add(200, inject)

    manager.connect("script-message-received::probe", receive)
    view.connect("load-changed", loaded)
    view.load_html(html, "about:blank")
    window.show_all()
    GLib.timeout_add_seconds(10, finish)
    Gtk.main()
    return 0 if len(rows) == len(samples) else 1


if __name__ == "__main__":
    raise SystemExit(main())
