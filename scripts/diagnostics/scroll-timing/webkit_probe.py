import gi,json,sys,time,os
gi.require_version('Gtk','3.0');gi.require_version('WebKit2','4.1')
from gi.repository import Gtk,Gdk,WebKit2,GLib
mode=sys.argv[1] if len(sys.argv)>1 else 'off'
manager=WebKit2.UserContentManager();manager.register_script_message_handler('probe')
view=WebKit2.WebView(user_content_manager=manager)
view.get_settings().set_enable_smooth_scrolling(mode=='on')
win=Gtk.Window(title='CodeMux isolated scroll timing');win.set_default_size(1000,800);win.add(view);win.fullscreen()
manager.connect('script-message-received::probe',lambda _,v: (print(v.get_js_value().to_string(),flush=True),Gtk.main_quit()))
html='''<!DOCTYPE html><style>body{margin:0;background:#202020;color:white}#s{height:100vh;overflow:auto}p{height:60px;margin:0;border-bottom:1px solid #444}</style><div id=s></div><script>
const s=document.querySelector('#s');s.innerHTML=Array.from({length:2000},(_,i)=>'<p>Mock row '+i+'</p>').join('');
let frames=[],wheels=[],scrolls=[],start;
s.addEventListener('wheel',e=>wheels.push([performance.now(),e.deltaY,e.deltaMode,e.isTrusted]),{passive:true});
s.addEventListener('scroll',()=>scrolls.push([performance.now(),s.scrollTop]),{passive:true});
requestAnimationFrame(function tick(t){start??=t;frames.push([t,s.scrollTop]);if(t-start<6000)requestAnimationFrame(tick);else window.webkit.messageHandlers.probe.postMessage(JSON.stringify({frames,wheels,scrolls,ua:navigator.userAgent,viewport:[innerWidth,innerHeight]}));});
</script>'''
view.load_uri(sys.argv[3]) if len(sys.argv)>3 else view.load_html(html,'http://localhost/');win.show_all()
# Native GDK input enters WebKit's normal wheel pipeline, unlike DOM dispatchEvent.
count=0
pointer=Gdk.Display.get_default().get_default_seat().get_pointer()
def wheel():
 global count
 e=Gdk.Event.new(Gdk.EventType.SCROLL);e.window=view.get_window();e.send_event=True;e.time=Gdk.CURRENT_TIME;e.x=500;e.y=400;e.direction=Gdk.ScrollDirection.SMOOTH;e.delta_x=0;e.delta_y=0.2;e.set_device(pointer);e.set_source_device(pointer)
 view.event(e);count+=1
 return count<250
def begin(): GLib.timeout_add(int(sys.argv[2]) if len(sys.argv)>2 else 8,wheel);return False
GLib.timeout_add(1800,begin);GLib.timeout_add_seconds(15,lambda:(Gtk.main_quit(),False)[1]);Gtk.main()
