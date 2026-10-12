// Appended only to a TEMPORARY diagnostic copy of production precise_wheel.rs.
// The adapter changes dispatch pacing; production input and duplicate logic stay.
#[derive(Default)]
struct DiagnosticGlide {
    template: Option<gdk::Event>,
    position: f64,
    written: f64,
    destination: f64,
    velocity: f64,
    start_position: f64,
    start_velocity: f64,
    start_time: f64,
    duration: f64,
    active: bool,
}
impl DiagnosticGlide {
    fn advance(&mut self, now: f64) -> bool {
        let t = ((now-self.start_time)/self.duration).clamp(0.0,1.0);
        let d = self.destination-self.start_position;
        let slope = self.start_velocity*self.duration;
        self.position=self.start_position+d*t*t*(3.0-2.0*t)+slope*t*(1.0-t)*(1.0-t);
        self.velocity=(d*6.0*t*(1.0-t)+slope*(1.0-4.0*t+3.0*t*t))/self.duration;
        t==1.0
    }
    fn input(&mut self, event: &gdk::Event, now: f64) {
        if self.active {self.advance(now);} else {self.velocity=0.0;}
        let raw:*mut gdk::ffi::GdkEvent=event.to_glib_none().0;
        self.destination+=unsafe {(*raw).scroll.delta_y};
        let distance=self.destination-self.position;
        // Viewport gain here is124px per GDK tick; keep duration comparable to
        // Chromium's200ms small wheel glide, shorten accumulated larger moves.
        let pixel_distance=distance.abs()*124.0;
        self.duration=200.0-(pixel_distance-120.0).clamp(0.0,360.0)/360.0*100.0;
        if self.velocity*distance>0.0 {
            self.duration=self.duration.min(2.5*(distance/self.velocity).abs());
        }
        self.duration=self.duration.max(16.0);
        self.start_position=self.position;
        self.start_velocity=self.velocity.signum()*self.velocity.abs().min(3.0*distance.abs()/self.duration);
        self.start_time=now;
        self.template=Some(event.clone());
        self.active=true;
    }
    fn tick(&mut self, now:f64)->Option<gdk::Event> {
        if !self.active {return None;}
        let settled=self.advance(now);
        let delta=self.position-self.written;
        self.written=self.position;
        self.active=!settled;
        if delta.abs()<1e-12 {return None;}
        let event=self.template.as_ref()?.clone();
        let raw:*mut gdk::ffi::GdkEvent=event.to_glib_none().0;
        unsafe {(*raw).scroll.delta_y=delta;(*raw).scroll.time=now as u32;}
        Some(event)
    }
}
thread_local! {
    static DIAGNOSTIC:RefCell<DiagnosticGlide>=RefCell::new(DiagnosticGlide::default());
    static DIAGNOSTIC_INPUTS:RefCell<Vec<(f64,i64)>>=const {RefCell::new(Vec::new())};
    static DIAGNOSTIC_GTK_FRAMES:RefCell<Vec<f64>>=const {RefCell::new(Vec::new())};
}
fn diagnostic_now()->f64 {unsafe {glib::ffi::g_get_monotonic_time() as f64/1000.0}}
fn diagnostic_wheel(view:&webkit2gtk::WebView,event:&gdk::Event) {
    if std::env::var("CODEMUX_NATIVE_GLIDE_MODE").unwrap()=="direct" {view.event(event);}
    else {DIAGNOSTIC.with(|state|state.borrow_mut().input(event,diagnostic_now()));}
}
fn main() {
    use javascriptcore::ValueExt;
    use std::cell::Cell;
    use webkit2gtk::{SettingsExt,WebViewExt};
    gtk::init().unwrap();
    let window=gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title("CodeMux native glide diagnostic");
    window.set_focus_on_map(false);window.set_accept_focus(false);
    window.set_default_size(720,360);
    let view=webkit2gtk::WebView::new();window.add(&view);
    WebViewExt::settings(&view).unwrap().set_enable_smooth_scrolling(false);
    view.add_tick_callback(|view,clock| {
        let now=clock.frame_time() as f64/1000.0;
        DIAGNOSTIC_GTK_FRAMES.with(|frames|frames.borrow_mut().push(now));
        let event=DIAGNOSTIC.with(|state|state.borrow_mut().tick(now));
        if let Some(event)=event {view.event(&event);}
        glib::ControlFlow::Continue
    });
    window.show_all();println!("available={}",attach(&view));
    let once=Rc::new(Cell::new(false));
    view.connect_load_changed(move|view,event| {
        if event!=webkit2gtk::LoadEvent::Finished||once.replace(true) {return;}
        let view=view.clone();let loaded=std::time::Instant::now();
        let start=Rc::new(Cell::new(0.0));let next=Rc::new(Cell::new(0usize));
        let mut schedule=vec![(200.0,15i64)];
        schedule.extend((0..6).map(|i|(1000.0+i as f64*80.0,15)));
        schedule.extend([(2200.0,15),(2225.0,15),(2250.0,15),(2275.0,15),(2310.0,-15),(2335.0,-15)]);
        schedule.extend([(2600.0,120),(2700.0,-120)]);
        schedule.extend((0..64).map(|i|(3200.0+i as f64*5.0,15)));
        glib::timeout_add_local(std::time::Duration::from_millis(1),move|| {
            if loaded.elapsed()<std::time::Duration::from_millis(900) {return glib::ControlFlow::Continue;}
            if start.get()==0.0 {start.set(diagnostic_now());}
            let elapsed=diagnostic_now()-start.get();
            if elapsed>=4200.0 {
                let inputs=DIAGNOSTIC_INPUTS.with(|events|events.borrow().iter().map(|(t,v)|format!("[{t},{v}]")).collect::<Vec<_>>().join(","));
                let frames=DIAGNOSTIC_GTK_FRAMES.with(|frames|frames.borrow().iter().map(|t|t.to_string()).collect::<Vec<_>>().join(","));
                let monotonic=diagnostic_now();
                let wall=unsafe {glib::ffi::g_get_real_time() as f64/1000.0};
                let js=format!("JSON.stringify({{events:window.events,total:window.total,offset:document.getElementById('scroll').scrollTop,prevented:window.prevented,frames:window.frames,height:window.innerHeight,inputs:[{inputs}],gtkFrames:[{frames}],clock:{{monotonic:{monotonic},wall:{wall}}}}})");
                view.evaluate_javascript(&js,None,None,None::<&gtk::gio::Cancellable>,|r| {println!("DOM {}",r.unwrap().to_str());gtk::main_quit();});
                return glib::ControlFlow::Break;
            }
            while next.get()<schedule.len()&&schedule[next.get()].0<=elapsed {
                let i=next.get();let (_,value)=schedule[i];next.set(i+1);
                DIAGNOSTIC_INPUTS.with(|events|events.borrow_mut().push((diagnostic_now(),value)));
                BRIDGE.with(|slot| {
                    let owner=slot.borrow().as_ref().unwrap().clone();
                    let seat_ptr={let mut o=owner.borrow_mut();&mut *o.seats[0] as *mut Seat};
                    unsafe {
                        let window=view.window().unwrap();let top=window.toplevel();
                        (*seat_ptr).surface=gdk_wayland_window_get_wl_surface(top.to_glib_none().0);
                        (*seat_ptr).x=100.0;(*seat_ptr).y=100.0;
                        (*seat_ptr).frame=Frame{source:Some(0),time:Some(diagnostic_now() as u32),y120:value,seen120:true,legacy_axis:i%8==7,..Frame::default()};
                        let event_time=(*seat_ptr).frame.time.unwrap();frame(seat_ptr.cast(),ptr::null_mut());
                        if i%8==7 {
                            let mut legacy:gdk::Event=from_glib_full(gdk::ffi::gdk_event_new(gdk::ffi::GDK_SCROLL));
                            let device=view.display().default_seat().unwrap().pointer().unwrap();
                            legacy.set_device(Some(&device));legacy.set_source_device(Some(&device));
                            let raw:*mut gdk::ffi::GdkEvent=legacy.to_glib_none().0;
                            let wp:*mut gdk::ffi::GdkWindow=window.to_glib_none().0;
                            (*raw).scroll.window=glib::gobject_ffi::g_object_ref(wp.cast()).cast();
                            (*raw).scroll.time=event_time;(*raw).scroll.x=100.0;(*raw).scroll.y=100.0;
                            (*raw).scroll.direction=gdk::ffi::GDK_SCROLL_SMOOTH;
                            (*raw).scroll.delta_y=1.5*value.signum() as f64;
                            view.event(&legacy);
                        }
                    }
                });
            }
            glib::ControlFlow::Continue
        });
    });
    view.load_html(r#"<!doctype html><style>body{margin:0}#scroll{height:100vh;overflow:auto}#content{height:10000px}</style><div id=scroll><div id=content></div></div><script>
window.events=[];window.total=0;window.prevented=0;window.frames=[];window.epoch=performance.timeOrigin;
document.addEventListener('wheel',e=>{window.events.push({time:performance.timeOrigin+performance.now(),deltaY:e.deltaY,trusted:e.isTrusted,cancelable:e.cancelable});window.total+=e.deltaY;window.prevented+=Number(e.defaultPrevented);},{passive:true,capture:true});
function sample(t){window.frames.push({time:performance.timeOrigin+t,offset:document.getElementById('scroll').scrollTop});requestAnimationFrame(sample);}requestAnimationFrame(sample);
</script>"#,None);
    glib::timeout_add_local_once(std::time::Duration::from_secs(12),||{eprintln!("timeout");gtk::main_quit();});
    gtk::main();unsafe {window.destroy();}
}
