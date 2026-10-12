// Diagnostic main appended to the CURRENT production module by native_precision_probe.py.
// Exercises its private native frame path without requesting physical input.
fn main() {
    use javascriptcore::ValueExt;
    use std::cell::Cell;
    use webkit2gtk::{SettingsExt, WebViewExt};
    gtk::init().unwrap();
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title("CodeMux native precise-wheel verification");
    window.set_focus_on_map(false);
    window.set_accept_focus(false);
    window.set_default_size(720, 360);
    let view = webkit2gtk::WebView::new();
    window.add(&view);
    WebViewExt::settings(&view)
        .unwrap()
        .set_enable_smooth_scrolling(false);
    window.show_all();
    println!("available={}", attach(&view));
    let once = Rc::new(Cell::new(false));
    view.connect_load_changed(move |view, event| {
        if event != webkit2gtk::LoadEvent::Finished || once.replace(true) { return; }
        let view=view.clone(); let count=Rc::new(Cell::new(0u32)); let ready=std::time::Instant::now(); let js_ready=Rc::new(Cell::new(false));let requested=Rc::new(Cell::new(false));
        glib::timeout_add_local(std::time::Duration::from_millis(5), move || {
            if ready.elapsed()<std::time::Duration::from_millis(500) {return glib::ControlFlow::Continue;}
            if !js_ready.get() {
                if !requested.replace(true) {
                    let ready=js_ready.clone();let pending=requested.clone();
                    view.evaluate_javascript("Boolean(window.probeReady)",None,None,None::<&gtk::gio::Cancellable>, move |r| {ready.set(r.is_ok_and(|v|v.to_boolean()));pending.set(false);});
                }
                return glib::ControlFlow::Continue;
            }
            let i=count.get();
            if i>=64 {
                let view=view.clone();
                glib::timeout_add_local_once(std::time::Duration::from_millis(150), move || {
                    view.evaluate_javascript("JSON.stringify({events:window.events, total:window.total, height:window.innerHeight, offset:document.getElementById('scroll').scrollTop, frontend:window.frontend, native:window.native, prevented:window.prevented, frames:window.frames})",None,None,None::<&gtk::gio::Cancellable>,|r| { println!("DOM {}",r.unwrap().to_str());gtk::main_quit(); });
                });
                return glib::ControlFlow::Break;
            }
            count.set(i+1);
            BRIDGE.with(|slot| {
                let owner=slot.borrow().as_ref().unwrap().clone();
                let seat_ptr={ let mut o=owner.borrow_mut(); &mut *o.seats[0] as *mut Seat };
                unsafe {
                    let window=view.window().unwrap(); let top=window.toplevel();
                    (*seat_ptr).surface=gdk_wayland_window_get_wl_surface(top.to_glib_none().0);
                    (*seat_ptr).x=100.0;(*seat_ptr).y=100.0;
                    (*seat_ptr).frame=Frame{source:Some(0),time:Some((glib::ffi::g_get_monotonic_time()/1000) as u32),y120:15,seen120:true,legacy_axis:i%8==7,..Frame::default()};
                    let event_time=(*seat_ptr).frame.time.unwrap();frame(seat_ptr.cast(),ptr::null_mut());
                    // Reproduce GTK3's coarse duplicate every eighth movement.
                    if i%8==7 {
                        let mut legacy:gdk::Event=from_glib_full(gdk::ffi::gdk_event_new(gdk::ffi::GDK_SCROLL));
                        let device=view.display().default_seat().unwrap().pointer().unwrap();
                        legacy.set_device(Some(&device));legacy.set_source_device(Some(&device));
                        let raw:*mut gdk::ffi::GdkEvent=legacy.to_glib_none().0;
                        let wp:*mut gdk::ffi::GdkWindow=window.to_glib_none().0;
                        (*raw).scroll.window=glib::gobject_ffi::g_object_ref(wp.cast()).cast();
                        (*raw).scroll.time=event_time;(*raw).scroll.x=100.0;(*raw).scroll.y=100.0;(*raw).scroll.direction=gdk::ffi::GDK_SCROLL_SMOOTH;(*raw).scroll.delta_y=1.5;
                        view.event(&legacy);
                    }
                }
            });
            glib::ControlFlow::Continue
        });
    });
    let frontend = std::env::var("CODEMUX_PRECISION_FRONTEND_URL").ok();
    let native = std::env::var("CODEMUX_PRECISION_NATIVE").is_ok_and(|value| value == "1");
    let module = frontend.as_ref().map(|url| format!("<script type=module>import{{installWheelScrolling}}from {:?};installWheelScrolling(document,()=>true);window.probeReady=true;</script>", format!("{}/src/lib/wheel-scrolling.ts",url.trim_end_matches('/')))).unwrap_or_default();
    let html = format!(
        r#"<!doctype html><style>body{{margin:0}}#scroll{{height:100vh;overflow:auto}}#content{{height:10000px}}</style><div id=scroll><div id=content></div></div><script>
window.events=[];window.total=0;window.prevented=0;window.frames=[];window.firstInput=null;window.frontend={frontend};window.native={native};window.probeReady=!window.frontend;
document.addEventListener('wheel',e=>{{if(window.firstInput===null)window.firstInput=performance.now();window.events.push({{deltaY:e.deltaY,wheelDeltaY:e.wheelDeltaY,trusted:e.isTrusted,cancelable:e.cancelable,time:performance.now()-window.firstInput}});window.total+=e.deltaY;if(!window.frontend&&!window.native){{e.preventDefault();document.getElementById('scroll').scrollTop=window.total;}}}},{{passive:window.native,capture:true}});
document.addEventListener('wheel',e=>{{window.prevented+=Number(e.defaultPrevented);}},{{passive:true}});
function sample(t){{if(window.firstInput!==null)window.frames.push({{time:t-window.firstInput,offset:document.getElementById('scroll').scrollTop}});requestAnimationFrame(sample);}}requestAnimationFrame(sample);
</script>{module}"#,
        frontend = frontend.is_some(),
        native = native
    );
    view.load_html(&html, frontend.as_deref());
    glib::timeout_add_local_once(std::time::Duration::from_secs(12), || {
        eprintln!("timeout");
        gtk::main_quit();
    });
    gtk::main();
    unsafe {
        window.destroy();
    }
}
