//! Receive Wayland value120 wheel fractions that GTK3's version-5 pointer loses.
//!
//! A second version-8 pointer shares GTK's existing connection and dispatch loop.
//! It observes only this client's focused surfaces, translates mouse-wheel frames
//! into ordinary GDK smooth events, and suppresses matching GTK legacy duplicates.
//! Touchpads, X11, older compositors, and frames without value120 keep GTK's path.
//! No input devices are opened and no compositor/system configuration is changed.

use glib::translate::{from_glib_full, ToGlibPtr};
use gtk::{gdk, glib, prelude::*};
use libc::{c_char, c_int, c_void};
use std::{
    cell::RefCell,
    collections::VecDeque,
    ptr,
    rc::{Rc, Weak},
    sync::atomic::{AtomicBool, Ordering},
};

static AVAILABLE: AtomicBool = AtomicBool::new(false);
static ANNOUNCED: AtomicBool = AtomicBool::new(false);
thread_local! {
    static BRIDGE: RefCell<Option<Rc<RefCell<Bridge>>>> = const { RefCell::new(None) };
}

/// Whether a live Wayland seat supports the value120 wheel protocol.
/// This reports protocol availability, not the attached mouse's resolution.
pub fn is_available() -> bool {
    AVAILABLE.load(Ordering::Relaxed)
}

/// Install once per native webview, on GTK's main thread. Returns false for an
/// unsupported backend or disabled bridge. Page reloads need no reinstallation.
pub fn attach(view: &webkit2gtk::WebView) -> bool {
    if std::env::var("CODEMUX_PRECISE_WHEEL")
        .is_ok_and(|v| matches!(v.to_ascii_lowercase().as_str(), "0" | "false" | "off"))
    {
        return false;
    }
    let display = view.display();
    if display.type_().name() != "GdkWaylandDisplay" || display.list_seats().len() != 1 {
        return false;
    }
    let owner = BRIDGE.with(|slot| {
        if let Some(owner) = slot.borrow().as_ref() {
            if owner.borrow()._display != display {
                return None;
            }
            return Some(owner.clone());
        }
        // GTK owns this connection and continues to dispatch its default queue.
        let connection = unsafe { gdk_wayland_display_get_wl_display(display.to_glib_none().0) };
        if connection.is_null() {
            return None;
        }
        // Queued Wayland closures can outlive a destroyed proxy. Keep the
        // private descriptions with GTK's connection owner as well as Bridge;
        // GDK finalizes the connection before GObject releases this data.
        let protocol = unsafe {
            const KEY: &str = "codemux-private-wheel-protocol";
            if let Some(cached) = display.data::<Rc<Protocol>>(KEY) {
                cached.as_ref().clone()
            } else {
                let protocol = Rc::new(Protocol::new(&wl_seat_interface, &wl_pointer_interface)?);
                display.set_data(KEY, protocol.clone());
                protocol
            }
        };
        let owner = Rc::new_cyclic(|weak| {
            RefCell::new(Bridge {
                weak: weak.clone(),
                registry: ptr::null_mut(),
                seats: Vec::new(),
                targets: Vec::new(),
                capable_seats: 0,
                _display: display.clone(),
                protocol,
            })
        });
        let registry = unsafe {
            wl_proxy_marshal_flags(
                connection,
                1,
                &wl_registry_interface,
                wl_proxy_get_version(connection),
                0,
                ptr::null_mut::<c_void>(),
            )
        };
        if registry.is_null() {
            return None;
        }
        owner.borrow_mut().registry = registry;
        if unsafe {
            wl_proxy_add_listener(
                registry,
                (&REGISTRY_LISTENER as *const RegistryListener).cast(),
                Rc::as_ptr(&owner).cast_mut().cast(),
            )
        } != 0
        {
            return None;
        }
        // Finish registry/capability discovery before a page queries availability.
        // These dispatch Wayland callbacks, never a nested GTK event loop.
        if unsafe { wl_display_roundtrip(connection) } < 0
            || unsafe { wl_display_roundtrip(connection) } < 0
        {
            return None;
        }
        *slot.borrow_mut() = Some(owner.clone());
        Some(owner)
    });
    let Some(owner) = owner else {
        return false;
    };
    let mut state = owner.borrow_mut();
    state.targets.retain(|t| t.view.upgrade().is_some());
    if state
        .targets
        .iter()
        .any(|t| t.view.upgrade().as_ref() == Some(view))
    {
        return is_available();
    }
    let target = Rc::new(Target {
        view: view.downgrade(),
        duplicates: RefCell::new(Duplicates::default()),
    });
    let filter = target.clone();
    view.connect_scroll_event(move |_, event| {
        if event.is_send_event() {
            return glib::Propagation::Proceed;
        }
        let mouse = event
            .source_device()
            .is_some_and(|d| d.source() == gdk::InputSource::Mouse);
        if filter
            .duplicates
            .try_borrow()
            .is_ok_and(|history| history.suppresses(event.time(), mouse, event.is_send_event()))
        {
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    state.targets.push(target);
    if is_available() && !ANNOUNCED.swap(true, Ordering::Relaxed) {
        eprintln!("[codemux::scroll] Wayland high-resolution wheel bridge enabled (GTK connection, value120)");
    }
    is_available()
}

struct Target {
    view: glib::WeakRef<webkit2gtk::WebView>,
    // Both emulated-discrete and smooth GTK events can share one input timestamp.
    // Keep a bounded recent history; do not consume a match on its first event.
    duplicates: RefCell<Duplicates>,
}
#[derive(Default)]
struct Duplicates(VecDeque<u32>);
impl Duplicates {
    fn remember(&mut self, time: u32) {
        // Retain only frames which can produce legacy GTK deltas (see frame).
        // 4096 full notches tolerate a long Wayland batch before GTK drains it.
        if self.0.len() == 4096 {
            self.0.pop_front();
        }
        self.0.push_back(time);
    }
    fn suppresses(&self, time: u32, mouse: bool, synthetic: bool) -> bool {
        mouse && !synthetic && self.0.contains(&time)
    }
}

struct Bridge {
    weak: Weak<RefCell<Bridge>>,
    registry: *mut c_void,
    // Boxes keep listener userdata addresses stable as seats are added/removed.
    seats: Vec<Box<Seat>>,
    targets: Vec<Rc<Target>>,
    capable_seats: usize,
    // Keep GTK's borrowed wl_display alive until our proxies are destroyed.
    _display: gdk::Display,
    // Private metadata is shared with GDK's connection owner and outlives every
    // seat/pointer and queued closure that refers to its stable boxes.
    protocol: Rc<Protocol>,
}
struct Seat {
    owner: Weak<RefCell<Bridge>>,
    name: u32,
    proxy: *mut c_void,
    pointer: *mut c_void,
    surface: *mut c_void,
    x: f64,
    y: f64,
    frame: Frame,
}
#[derive(Default, Clone, Copy)]
struct Frame {
    source: Option<u32>,
    time: Option<u32>,
    x120: i64,
    y120: i64,
    seen120: bool,
    legacy_axis: bool,
}
impl Frame {
    fn wheel(self) -> Option<(u32, f64, f64)> {
        // value120 is wheel-specific; explicit source prevents replacing finger
        // events from a broken/mixed frame. Reject missing timestamps as well.
        if self.source != Some(0) || !self.seen120 || (self.x120 == 0 && self.y120 == 0) {
            return None;
        }
        Some((
            self.time?,
            self.x120 as f64 / 120.0,
            self.y120 as f64 / 120.0,
        ))
    }
}
impl Drop for Seat {
    fn drop(&mut self) {
        unsafe {
            destroy_request(&mut self.pointer, 1);
            destroy_request(&mut self.proxy, 3);
        }
    }
}
impl Drop for Bridge {
    fn drop(&mut self) {
        self.seats.clear();
        if !self.registry.is_null() {
            unsafe {
                wl_proxy_destroy(self.registry);
            }
        }
        AVAILABLE.store(false, Ordering::Relaxed);
    }
}
unsafe fn destroy_request(proxy: &mut *mut c_void, opcode: u32) {
    if !proxy.is_null() {
        wl_proxy_marshal_flags(*proxy, opcode, ptr::null(), wl_proxy_get_version(*proxy), 1);
        *proxy = ptr::null_mut();
    }
}

unsafe extern "C" fn global(
    data: *mut c_void,
    registry: *mut c_void,
    name: u32,
    interface: *const c_char,
    version: u32,
) {
    if version < 8 || std::ffi::CStr::from_ptr(interface).to_bytes() != b"wl_seat" {
        return;
    }
    let owner = &*data.cast::<RefCell<Bridge>>();
    let Ok(mut owner) = owner.try_borrow_mut() else {
        return;
    };
    let proxy = wl_proxy_marshal_flags(
        registry,
        0,
        owner.protocol.seat(),
        8,
        0,
        name,
        c"wl_seat".as_ptr(),
        8u32,
        ptr::null_mut::<c_void>(),
    );
    if proxy.is_null() {
        return;
    }
    let mut seat = Box::new(Seat {
        owner: owner.weak.clone(),
        name,
        proxy,
        pointer: ptr::null_mut(),
        surface: ptr::null_mut(),
        x: 0.0,
        y: 0.0,
        frame: Frame::default(),
    });
    if wl_proxy_add_listener(
        proxy,
        (&SEAT_LISTENER as *const SeatListener).cast(),
        (&mut *seat as *mut Seat).cast(),
    ) != 0
    {
        return;
    }
    owner.seats.push(seat);
}
unsafe extern "C" fn removed(data: *mut c_void, _: *mut c_void, name: u32) {
    let owner = &*data.cast::<RefCell<Bridge>>();
    let Ok(mut owner) = owner.try_borrow_mut() else {
        return;
    };
    if let Some(index) = owner.seats.iter().position(|s| s.name == name) {
        let seat = owner.seats.remove(index);
        if !seat.pointer.is_null() {
            owner.capable_seats = owner.capable_seats.saturating_sub(1);
        }
        drop(seat);
        AVAILABLE.store(owner.capable_seats == 1, Ordering::Relaxed);
    }
}
unsafe extern "C" fn capabilities(data: *mut c_void, proxy: *mut c_void, caps: u32) {
    let seat = &mut *data.cast::<Seat>();
    if caps & 1 != 0 && seat.pointer.is_null() {
        let Some(owner) = seat.owner.upgrade() else {
            return;
        };
        let interface = {
            let Ok(owner) = owner.try_borrow() else {
                return;
            };
            owner.protocol.pointer()
        };
        seat.pointer = wl_proxy_marshal_flags(proxy, 0, interface, 8, 0, ptr::null_mut::<c_void>());
        if seat.pointer.is_null() {
            return;
        }
        if wl_proxy_add_listener(
            seat.pointer,
            (&POINTER_LISTENER as *const PointerListener).cast(),
            data,
        ) != 0
        {
            destroy_request(&mut seat.pointer, 1);
            return;
        }
        if let Some(owner) = seat.owner.upgrade() {
            if let Ok(mut owner) = owner.try_borrow_mut() {
                owner.capable_seats += 1;
                AVAILABLE.store(owner.capable_seats == 1, Ordering::Relaxed);
            }
        }
    } else if caps & 1 == 0 && !seat.pointer.is_null() {
        destroy_request(&mut seat.pointer, 1);
        seat.surface = ptr::null_mut();
        seat.frame = Frame::default();
        if let Some(owner) = seat.owner.upgrade() {
            if let Ok(mut owner) = owner.try_borrow_mut() {
                owner.capable_seats = owner.capable_seats.saturating_sub(1);
                AVAILABLE.store(owner.capable_seats == 1, Ordering::Relaxed);
            }
        }
    }
}
unsafe extern "C" fn seat_name(_: *mut c_void, _: *mut c_void, _: *const c_char) {}
unsafe extern "C" fn enter(
    data: *mut c_void,
    _: *mut c_void,
    _: u32,
    surface: *mut c_void,
    x: i32,
    y: i32,
) {
    let seat = &mut *data.cast::<Seat>();
    seat.surface = surface;
    seat.x = x as f64 / 256.0;
    seat.y = y as f64 / 256.0;
    seat.frame = Frame::default();
}
unsafe extern "C" fn leave(data: *mut c_void, _: *mut c_void, _: u32, _: *mut c_void) {
    let seat = &mut *data.cast::<Seat>();
    seat.surface = ptr::null_mut();
    seat.frame = Frame::default();
}
unsafe extern "C" fn motion(data: *mut c_void, _: *mut c_void, _: u32, x: i32, y: i32) {
    let seat = &mut *data.cast::<Seat>();
    seat.x = x as f64 / 256.0;
    seat.y = y as f64 / 256.0;
}
unsafe extern "C" fn button(_: *mut c_void, _: *mut c_void, _: u32, _: u32, _: u32, _: u32) {}
unsafe extern "C" fn axis(data: *mut c_void, _: *mut c_void, time: u32, _: u32, value: i32) {
    let frame = &mut (*data.cast::<Seat>()).frame;
    frame.time = Some(time);
    frame.legacy_axis |= value != 0;
}
unsafe extern "C" fn axis_source(data: *mut c_void, _: *mut c_void, source: u32) {
    (*data.cast::<Seat>()).frame.source = Some(source);
}
unsafe extern "C" fn axis_stop(_: *mut c_void, _: *mut c_void, _: u32, _: u32) {}
unsafe extern "C" fn axis_discrete(_: *mut c_void, _: *mut c_void, _: u32, _: i32) {}
unsafe extern "C" fn axis_value120(data: *mut c_void, _: *mut c_void, axis: u32, value: i32) {
    let frame = &mut (*data.cast::<Seat>()).frame;
    match axis {
        0 => frame.y120 = frame.y120.saturating_add(value as i64),
        1 => frame.x120 = frame.x120.saturating_add(value as i64),
        _ => return,
    }
    frame.seen120 = true;
}
unsafe extern "C" fn frame(data: *mut c_void, _: *mut c_void) {
    // Rust must never unwind through libwayland/GTK. Fail closed on a callback error.
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| frame_inner(data))).is_err() {
        AVAILABLE.store(false, Ordering::Relaxed);
    }
}
unsafe fn frame_inner(data: *mut c_void) {
    // Snapshot before calling GTK; no Seat reference survives a native handler
    // that could reenter Wayland dispatch or remove its seat.
    let (frame, surface, pointer_x, pointer_y, owner) = {
        let seat = &mut *data.cast::<Seat>();
        (
            std::mem::take(&mut seat.frame),
            seat.surface,
            seat.x,
            seat.y,
            seat.owner.upgrade(),
        )
    };
    if !is_available() {
        return;
    }
    let Some((time, dx, dy)) = frame.wheel() else {
        return;
    };
    if surface.is_null() {
        return;
    }
    let Some(owner) = owner else {
        return;
    };
    let targets = {
        let Ok(owner) = owner.try_borrow() else {
            return;
        };
        if owner.capable_seats != 1 {
            return;
        }
        owner.targets.clone()
    };
    for target in targets {
        let Some(view) = target.view.upgrade() else {
            continue;
        };
        if !view.is_mapped() || view.display().list_seats().len() != 1 {
            continue;
        }
        let Some(window) = view.window() else {
            continue;
        };
        let top_window = window.toplevel();
        if gdk_wayland_window_get_wl_surface(top_window.to_glib_none().0) != surface {
            continue;
        }
        let Some(top_widget) = view.toplevel() else {
            continue;
        };
        let Some((x, y)) = top_widget.translate_coordinates(
            &view,
            pointer_x.floor() as i32,
            pointer_y.floor() as i32,
        ) else {
            continue;
        };
        if x < 0 || y < 0 || x >= view.allocated_width() || y >= view.allocated_height() {
            continue;
        }
        let Some(device) = view.display().default_seat().and_then(|s| s.pointer()) else {
            continue;
        };
        let mut event: gdk::Event = from_glib_full(gdk::ffi::gdk_event_new(gdk::ffi::GDK_SCROLL));

        event.set_device(Some(&device));
        event.set_source_device(Some(&device));
        let raw: *mut gdk::ffi::GdkEvent = event.to_glib_none().0;
        let scroll = &mut (*raw).scroll;
        let window_ptr: *mut gdk::ffi::GdkWindow = window.to_glib_none().0;
        scroll.window = glib::gobject_ffi::g_object_ref(window_ptr.cast()).cast();
        // Distinguish this native GDK event and its WebKit propagation copy from
        // GTK legacy duplicates. WebKit creates trusted DOM input from both.
        scroll.send_event = 1;
        scroll.time = time;
        scroll.direction = gdk::ffi::GDK_SCROLL_SMOOTH;
        scroll.x = x as f64 + pointer_x.fract();
        scroll.y = y as f64 + pointer_y.fract();
        scroll.delta_x = dx;
        scroll.delta_y = dy;
        let (_, root_x, root_y) = window.origin();
        scroll.x_root = root_x as f64 + scroll.x;
        scroll.y_root = root_y as f64 + scroll.y;
        let mut state = 0;
        gdk::ffi::gdk_device_get_state(
            device.to_glib_none().0,
            window.to_glib_none().0,
            ptr::null_mut(),
            &mut state,
        );
        scroll.state = state;
        if frame.legacy_axis {
            let Ok(mut duplicates) = target.duplicates.try_borrow_mut() else {
                continue;
            };
            duplicates.remember(time);
        }
        // A direct native event retains WebKit's trusted/cancelable wheel path.
        // Release all RefCell borrows before GTK invokes application handlers.
        view.event(&event);
        break;
    }
}

// The small ABI surface below deliberately shares GTK's libwayland-client:
// a separate Rust-owned display/queue cannot receive this client's focus events.
#[derive(Clone, Copy)]
#[repr(C)]
struct Interface {
    name: *const c_char,
    version: c_int,
    method_count: c_int,
    methods: *const Message,
    event_count: c_int,
    events: *const Message,
}
#[derive(Clone, Copy)]
#[repr(C)]
struct Message {
    name: *const c_char,
    signature: *const c_char,
    types: *const *const Interface,
}

// AppImages built on Ubuntu 22.04 include libwayland 1.20's version-7
// descriptions. The marshaller/decoder is generic: private v8 descriptions
// let our NEW objects receive value120 without changing GTK's v5 objects or
// replacing libraries. Current runtimes use their original descriptions.
struct Protocol {
    seat: *const Interface,
    pointer: *const Interface,
    _private: Option<PrivateProtocol>,
}
struct PrivateProtocol {
    _seat: Box<Interface>,
    _pointer: Box<Interface>,
    _events: Box<[Message]>,
    _types: Box<[*const Interface; 2]>,
}
impl Protocol {
    unsafe fn new(seat: &Interface, pointer: &Interface) -> Option<Self> {
        if seat.version >= 8 && pointer.version >= 8 {
            return Some(Self {
                seat,
                pointer,
                _private: None,
            });
        }
        // Validate the known core ABI before cloning its stable descriptions.
        // v7 has all requests and the nine events through axis_discrete. Seat
        // gained no requests/events in v8, only the supported child version.
        if seat.version < 7
            || pointer.version < 7
            || seat.method_count != 4
            || seat.event_count != 2
            || pointer.method_count != 2
            || pointer.event_count < 9
            || seat.methods.is_null()
            || seat.events.is_null()
            || pointer.methods.is_null()
            || pointer.events.is_null()
        {
            return None;
        }
        let prefix = std::slice::from_raw_parts(pointer.events, 9);
        let names: [&[u8]; 9] = [
            b"enter",
            b"leave",
            b"motion",
            b"button",
            b"axis",
            b"frame",
            b"axis_source",
            b"axis_stop",
            b"axis_discrete",
        ];
        if prefix.iter().zip(names).any(|(event, name)| {
            event.name.is_null() || std::ffi::CStr::from_ptr(event.name).to_bytes() != name
        }) {
            return None;
        }
        let types = Box::new([ptr::null(), ptr::null()]);
        let mut events = prefix.to_vec();
        events.push(Message {
            name: c"axis_value120".as_ptr(),
            signature: c"8ui".as_ptr(),
            types: types.as_ptr(),
        });
        let events = events.into_boxed_slice();
        let pointer = Box::new(Interface {
            version: 8,
            event_count: 10,
            events: events.as_ptr(),
            ..*pointer
        });
        let seat = Box::new(Interface {
            version: 8,
            ..*seat
        });
        Some(Self {
            seat: &*seat,
            pointer: &*pointer,
            _private: Some(PrivateProtocol {
                _seat: seat,
                _pointer: pointer,
                _events: events,
                _types: types,
            }),
        })
    }
    fn seat(&self) -> *const Interface {
        self.seat
    }
    fn pointer(&self) -> *const Interface {
        self.pointer
    }
}

#[repr(C)]
struct RegistryListener {
    global: unsafe extern "C" fn(*mut c_void, *mut c_void, u32, *const c_char, u32),
    removed: unsafe extern "C" fn(*mut c_void, *mut c_void, u32),
}
#[repr(C)]
struct SeatListener {
    capabilities: unsafe extern "C" fn(*mut c_void, *mut c_void, u32),
    name: unsafe extern "C" fn(*mut c_void, *mut c_void, *const c_char),
}
#[repr(C)]
struct PointerListener {
    enter: unsafe extern "C" fn(*mut c_void, *mut c_void, u32, *mut c_void, i32, i32),
    leave: unsafe extern "C" fn(*mut c_void, *mut c_void, u32, *mut c_void),
    motion: unsafe extern "C" fn(*mut c_void, *mut c_void, u32, i32, i32),
    button: unsafe extern "C" fn(*mut c_void, *mut c_void, u32, u32, u32, u32),
    axis: unsafe extern "C" fn(*mut c_void, *mut c_void, u32, u32, i32),
    frame: unsafe extern "C" fn(*mut c_void, *mut c_void),
    source: unsafe extern "C" fn(*mut c_void, *mut c_void, u32),
    stop: unsafe extern "C" fn(*mut c_void, *mut c_void, u32, u32),
    discrete: unsafe extern "C" fn(*mut c_void, *mut c_void, u32, i32),
    value120: unsafe extern "C" fn(*mut c_void, *mut c_void, u32, i32),
}
static REGISTRY_LISTENER: RegistryListener = RegistryListener { global, removed };
static SEAT_LISTENER: SeatListener = SeatListener {
    capabilities,
    name: seat_name,
};
static POINTER_LISTENER: PointerListener = PointerListener {
    enter,
    leave,
    motion,
    button,
    axis,
    frame,
    source: axis_source,
    stop: axis_stop,
    discrete: axis_discrete,
    value120: axis_value120,
};
#[link(name = "wayland-client")]
extern "C" {
    static wl_registry_interface: Interface;
    static wl_seat_interface: Interface;
    static wl_pointer_interface: Interface;
    fn wl_proxy_marshal_flags(
        proxy: *mut c_void,
        opcode: u32,
        interface: *const Interface,
        version: u32,
        flags: u32,
        ...
    ) -> *mut c_void;
    fn wl_proxy_get_version(proxy: *mut c_void) -> u32;
    fn wl_proxy_add_listener(
        proxy: *mut c_void,
        listener: *const c_void,
        data: *mut c_void,
    ) -> c_int;
    fn wl_proxy_destroy(proxy: *mut c_void);
    fn wl_display_roundtrip(display: *mut c_void) -> c_int;
}
#[link(name = "gdk-3")]
extern "C" {
    fn gdk_wayland_display_get_wl_display(display: *mut gdk::ffi::GdkDisplay) -> *mut c_void;
    fn gdk_wayland_window_get_wl_surface(window: *mut gdk::ffi::GdkWindow) -> *mut c_void;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn eighth_notch_preserves_fraction_and_sign() {
        assert_eq!(
            Frame {
                source: Some(0),
                time: Some(5),
                y120: -15,
                seen120: true,
                ..Frame::default()
            }
            .wheel(),
            Some((5, 0.0, -0.125))
        );
    }
    #[test]
    fn both_axes_and_multiple_notches_preserve_distance() {
        assert_eq!(
            Frame {
                source: Some(0),
                time: Some(u32::MAX),
                x120: 30,
                y120: 240,
                seen120: true,
                ..Frame::default()
            }
            .wheel(),
            Some((u32::MAX, 0.25, 2.0))
        );
    }
    #[test]
    fn incomplete_or_non_wheel_frames_fall_back() {
        let valid = Frame {
            source: Some(0),
            time: Some(1),
            y120: 15,
            seen120: true,
            ..Frame::default()
        };
        for source in [None, Some(1), Some(2), Some(3)] {
            assert_eq!(Frame { source, ..valid }.wheel(), None);
        }
        assert_eq!(
            Frame {
                time: None,
                ..valid
            }
            .wheel(),
            None
        );
        assert_eq!(
            Frame {
                seen120: false,
                ..valid
            }
            .wheel(),
            None
        );
        assert_eq!(Frame { y120: 0, ..valid }.wheel(), None);
    }
    #[test]
    fn both_legacy_copies_match_without_consuming_timestamp() {
        let mut history = Duplicates::default();
        history.remember(7);
        assert!(history.suppresses(7, true, false));
        assert!(history.suppresses(7, true, false));
        assert!(!history.suppresses(8, true, false));
    }
    #[test]
    fn synthetic_propagation_and_touchpads_are_never_suppressed() {
        let mut history = Duplicates::default();
        history.remember(7);
        assert!(!history.suppresses(7, true, true));
        assert!(!history.suppresses(7, false, false));
    }
    #[test]
    fn duplicate_history_survives_large_batches_and_stays_bounded() {
        let mut history = Duplicates::default();
        for time in 0..4096 {
            history.remember(time);
        }
        assert!(history.suppresses(0, true, false));
        assert!(history.suppresses(4095, true, false));
        history.remember(4096);
        assert_eq!(history.0.len(), 4096);
        assert!(!history.suppresses(0, true, false));
        assert!(history.suppresses(1, true, false));
    }
    fn private_protocol() -> Protocol {
        unsafe {
            let seat = Interface {
                version: 7,
                ..wl_seat_interface
            };
            let pointer = Interface {
                version: 7,
                event_count: 9,
                ..wl_pointer_interface
            };
            Protocol::new(&seat, &pointer).expect("known v7 core protocol")
        }
    }
    #[test]
    fn private_v8_descriptions_preserve_original_tables() {
        let protocol = private_protocol();
        unsafe {
            let pointer = &*protocol.pointer();
            let seat = &*protocol.seat();
            assert_eq!(pointer.version, 8);
            assert_eq!(pointer.event_count, 10);
            assert_eq!(seat.version, 8);
            assert_eq!(seat.methods, wl_seat_interface.methods);
            assert_eq!(pointer.methods, wl_pointer_interface.methods);
            let new_event = &*pointer.events.add(9);
            assert_eq!(
                std::ffi::CStr::from_ptr(new_event.name).to_bytes(),
                b"axis_value120"
            );
            assert_eq!(
                std::ffi::CStr::from_ptr(new_event.signature).to_bytes(),
                b"8ui"
            );
            assert!((*new_event.types).is_null());
            assert!((*new_event.types.add(1)).is_null());
            // GTK continues to use its original immutable interface event table.
            assert_ne!(pointer as *const _, &wl_pointer_interface as *const _);
            assert_ne!(pointer.events, wl_pointer_interface.events);
        }
    }
    #[test]
    fn private_metadata_rejects_unknown_core_layout() {
        unsafe {
            let seat = Interface {
                version: 7,
                ..wl_seat_interface
            };
            let pointer = Interface {
                version: 7,
                event_count: 8,
                ..wl_pointer_interface
            };
            assert!(Protocol::new(&seat, &pointer).is_none());
            let pointer = Interface {
                event_count: 9,
                method_count: 3,
                ..pointer
            };
            assert!(Protocol::new(&seat, &pointer).is_none());
        }
    }

    // End-to-end wire decode through the loaded libwayland-client. Can be run
    // unchanged with the actual AppImage's v7 library via process-only preload.
    // No compositor, GTK initialization, device access, or user input is needed.
    #[test]
    fn private_v8_metadata_decodes_fractional_wire_frames() {
        use std::{io::Write, os::fd::IntoRawFd, os::unix::net::UnixStream};
        #[link(name = "wayland-client")]
        extern "C" {
            fn wl_display_connect_to_fd(fd: c_int) -> *mut c_void;
            fn wl_display_disconnect(display: *mut c_void);
            fn wl_display_flush(display: *mut c_void) -> c_int;
            fn wl_display_dispatch(display: *mut c_void) -> c_int;
            fn wl_proxy_get_id(proxy: *mut c_void) -> u32;
        }
        struct Connection(*mut c_void);
        impl Drop for Connection {
            fn drop(&mut self) {
                unsafe {
                    wl_display_disconnect(self.0);
                }
            }
        }
        #[derive(Default)]
        struct Capture {
            frame: Frame,
            output: Vec<(u32, f64, f64)>,
        }
        unsafe extern "C" fn capture_source(data: *mut c_void, _: *mut c_void, source: u32) {
            (*data.cast::<Capture>()).frame.source = Some(source);
        }
        unsafe extern "C" fn capture_axis(
            data: *mut c_void,
            _: *mut c_void,
            time: u32,
            _: u32,
            _: i32,
        ) {
            (*data.cast::<Capture>()).frame.time = Some(time);
        }
        unsafe extern "C" fn capture_value120(
            data: *mut c_void,
            _: *mut c_void,
            axis: u32,
            value: i32,
        ) {
            let frame = &mut (*data.cast::<Capture>()).frame;
            frame.seen120 = true;
            if axis == 0 {
                frame.y120 += value as i64;
            } else {
                frame.x120 += value as i64;
            }
        }
        unsafe extern "C" fn capture_frame(data: *mut c_void, _: *mut c_void) {
            let capture = &mut *data.cast::<Capture>();
            if let Some(output) = std::mem::take(&mut capture.frame).wheel() {
                capture.output.push(output);
            }
        }
        let listener = PointerListener {
            source: capture_source,
            axis: capture_axis,
            value120: capture_value120,
            frame: capture_frame,
            ..POINTER_LISTENER
        };
        eprintln!(
            "Wayland runtime core tables: seat={}, pointer={}",
            unsafe { wl_seat_interface.version },
            unsafe { wl_pointer_interface.version }
        );
        let protocol = private_protocol();
        let (client, mut server) = UnixStream::pair().unwrap();
        let connection = Connection(unsafe { wl_display_connect_to_fd(client.into_raw_fd()) });
        assert!(!connection.0.is_null());
        unsafe {
            let registry = wl_proxy_marshal_flags(
                connection.0,
                1,
                &wl_registry_interface,
                1,
                0,
                ptr::null_mut::<c_void>(),
            );
            assert!(!registry.is_null());
            let seat = wl_proxy_marshal_flags(
                registry,
                0,
                protocol.seat(),
                8,
                0,
                1u32,
                c"wl_seat".as_ptr(),
                8u32,
                ptr::null_mut::<c_void>(),
            );
            assert!(!seat.is_null());
            let mut pointer = wl_proxy_marshal_flags(
                seat,
                0,
                protocol.pointer(),
                8,
                0,
                ptr::null_mut::<c_void>(),
            );
            assert!(!pointer.is_null());
            assert_eq!(wl_proxy_get_version(pointer), 8);
            let mut capture = Capture::default();
            assert_eq!(
                wl_proxy_add_listener(
                    pointer,
                    (&listener as *const PointerListener).cast(),
                    (&mut capture as *mut Capture).cast()
                ),
                0
            );
            assert!(wl_display_flush(connection.0) >= 0);
            let id = wl_proxy_get_id(pointer);
            let mut wire = Vec::new();
            let mut event = |opcode: u32, payload: &[u32]| {
                wire.extend(id.to_ne_bytes());
                wire.extend((((8 + 4 * payload.len()) as u32) << 16 | opcode).to_ne_bytes());
                for argument in payload {
                    wire.extend(argument.to_ne_bytes());
                }
            };
            for (time, value) in [(321, 15i32), (322, -15i32)] {
                event(6, &[0]); // axis_source WHEEL
                event(4, &[time, 0, 0]); // axis/time, zero coarse legacy delta
                event(9, &[0, value as u32]); // version-8 axis_value120
                event(5, &[]); // frame
            }
            server.write_all(&wire).unwrap();
            assert!(wl_display_dispatch(connection.0) >= 0);
            assert_eq!(capture.output, [(321, 0.0, 0.125), (322, 0.0, -0.125)]);
            destroy_request(&mut pointer, 1);
            let mut seat = seat;
            destroy_request(&mut seat, 3);
            wl_proxy_destroy(registry);
        }
        // Connection destruction precedes protocol metadata destruction.
    }
}
