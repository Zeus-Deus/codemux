"""Native wheel ticks at ordinary mouse intervals; no physical mouse recording."""
import gi
import json
import sys
gi.require_version('Gtk', '3.0')
gi.require_version('WebKit2', '4.1')
from gi.repository import Gtk, Gdk, WebKit2, GLib

manager = WebKit2.UserContentManager()
for name in ['ready', 'probe']:
    manager.register_script_message_handler(name)
view = WebKit2.WebView(user_content_manager=manager)
view.get_settings().set_enable_smooth_scrolling(False)
window = Gtk.Window(title='CodeMux isolated wheel glide probe')
window.set_default_size(600, 400)
window.set_accept_focus(False)
window.add(view)
pointer = Gdk.Display.get_default().get_default_seat().get_pointer()
injections = []
ready_received = False

def ready(_, value):
    global ready_received
    if ready_received:
        return
    ready_received = True
    point = json.loads(value.get_js_value().to_string())
    events = [(1200, 25.4, 'single25'), (1900, 120, 'single120')]
    for start, interval in [(2700, 50), (3700, 100), (4900, 150)]:
        events += [(start+i*interval, 25.4, f'spaced{interval}') for i in range(5)]
    events += [(6300+i*8, 25.4, 'burst8') for i in range(20)]
    events += [(6420, -120, 'reversal')]
    if len(sys.argv)>2 and sys.argv[2] == 'onset':
        events = [(1200, 25.4, 'small-down'), (1900, -25.4, 'small-up'),
                  (2700, 5, 'tiny-down'), (3400, -5, 'tiny-up'),
                  (4200, 120, 'notch-down'), (4900, -120, 'notch-up')]
        events += [(5800+i*150, 25.4 if i%2 == 0 else -25.4,
                    'alternating') for i in range(5)]
    if len(sys.argv)>2 and sys.argv[2] == 'precision':
        events = [(1200, 15.5, 'isolated-fraction')]
        events += [(2000+i*80, 15.5, 'slow-fractions') for i in range(6)]
        events += [(3400+i*5, 15.5, 'fast-fractions') for i in range(64)]
        events += [(4900+i*80, 15.5 if i<3 else -15.5,
                    'direction-change') for i in range(6)]
    def wheel(delta, label):
        event = Gdk.Event.new(Gdk.EventType.SCROLL)
        event.window = view.get_window()
        event.send_event = True
        event.time = Gdk.CURRENT_TIME
        event.x, event.y = point['x'], point['y']
        event.direction = Gdk.ScrollDirection.SMOOTH
        event.delta_x = 0
        # WebKit's GTK pixel step is viewport height raised to 2/3. Preserve
        # requested pixel distances despite the compositor's window sizing.
        event.delta_y = delta / view.get_allocated_height() ** (2/3)
        event.set_device(pointer)
        event.set_source_device(pointer)
        injections.append([GLib.get_monotonic_time()/1000, delta, label])
        view.event(event)
        return False
    for delay, delta, label in events:
        GLib.timeout_add(delay, wheel, delta, label)

def finished(_, value):
    result = json.loads(value.get_js_value().to_string())
    result['injections'] = injections
    print(json.dumps(result), flush=True)
    Gtk.main_quit()

manager.connect('script-message-received::ready', ready)
manager.connect('script-message-received::probe', finished)
view.load_uri(sys.argv[1] if len(sys.argv)>1 else 'http://localhost:1431/scripts/diagnostics/scroll-timing/glide-page.html?smooth=app')
window.show_all()
GLib.timeout_add_seconds(15, lambda: (Gtk.main_quit(), False)[1])
Gtk.main()
