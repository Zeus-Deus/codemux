import gi,json,sys
gi.require_version('Gtk','3.0');gi.require_version('WebKit2','4.1')
from gi.repository import Gtk,Gdk,WebKit2,GLib
mode=sys.argv[1] if len(sys.argv)>1 else 'off'
manager=WebKit2.UserContentManager();manager.register_script_message_handler('probe');manager.register_script_message_handler('ready');view=WebKit2.WebView(user_content_manager=manager);view.get_settings().set_enable_smooth_scrolling(mode=='on')
win=Gtk.Window(title='CodeMux mock transcript timing');win.set_default_size(1400,1000);win.add(view);win.fullscreen()
manager.connect('script-message-received::probe',lambda _,v:(print(v.get_js_value().to_string(),flush=True),Gtk.main_quit()))
coords=[700,400];pointer=Gdk.Display.get_default().get_default_seat().get_pointer();count=0
script='''(()=>{const s=document.querySelector('[data-slot="transcript-list"]');if(!s)throw Error('No transcript');const r=s.getBoundingClientRect();const rect={x:r.x+r.width/2,y:r.y+r.height/2,width:r.width,height:r.height,scrollHeight:s.scrollHeight};let frames=[],wheels=[],scrolls=[],start;const mutations=[];new MutationObserver(ms=>mutations.push([performance.now(),ms.length])).observe(s,{childList:true,subtree:true});s.addEventListener('wheel',e=>wheels.push([performance.now(),e.deltaY,e.deltaMode,e.isTrusted]),{passive:true});s.addEventListener('scroll',()=>scrolls.push([performance.now(),s.scrollTop]),{passive:true});requestAnimationFrame(function tick(t){start??=t;frames.push([t,s.scrollTop]);if(t-start<6000)requestAnimationFrame(tick);else window.webkit.messageHandlers.probe.postMessage(JSON.stringify({frames,wheels,scrolls,mutations,rect,ua:navigator.userAgent,viewport:[innerWidth,innerHeight]}));});window.webkit.messageHandlers.ready.postMessage(JSON.stringify(rect));return JSON.stringify(rect)})()'''
def wheel():
 global count
 e=Gdk.Event.new(Gdk.EventType.SCROLL);e.window=view.get_window();e.send_event=True;e.time=Gdk.CURRENT_TIME;e.x=coords[0];e.y=coords[1];e.direction=Gdk.ScrollDirection.SMOOTH;e.delta_x=0;e.delta_y=-0.2;e.set_device(pointer);e.set_source_device(pointer);view.event(e);count+=1;return count<250
def ready(_,value):
 rect=json.loads(value.get_js_value().to_string());coords[:]=[rect['x'],rect['y']];GLib.timeout_add(1800,lambda:(GLib.timeout_add(8,wheel),False)[1])
manager.connect('script-message-received::ready',ready)
def setup():
 code=script

 view.evaluate_javascript(code,-1,None,None,None,None,None);return False
view.load_uri(__import__('os').environ.get('SCROLL_PROBE_URL','http://localhost:1431/'));win.show_all();GLib.timeout_add(7000,setup);GLib.timeout_add_seconds(25,lambda:(Gtk.main_quit(),False)[1]);Gtk.main()
