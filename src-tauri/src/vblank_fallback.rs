//! Mode-paced fallback for WebKitGTK's DRM display clock on Linux.
//!
//! WebKitGTK 2.52 probes `drmWaitVBlank` before constructing its display link.
//! Unsupported waits otherwise select an independent 60 Hz timer, even on a
//! high-refresh display. This executable interposes only the blocking relative
//! sequence-0 probe and sequence-1 waits used by WebKit, and only after libdrm
//! reports EOPNOTSUPP. Working DRM clocks and every other request pass through.
//!
//! The fallback follows each CRTC's active mode, including fractional refresh
//! rates. It is a timer, NOT synchronized to physical scanout and NOT a VRR
//! clock. Mode changes are detected within one second; a fresh probe always
//! rechecks the mode. `CODEMUX_VBLANK_FALLBACK=0` restores unmodified libdrm.
//! This symbol is exported only by the GUI executable, never via LD_PRELOAD.

use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_long, c_ulong, c_void};
use std::io::Write;
use std::sync::{Mutex, OnceLock};

const RELATIVE: u32 = 0x1;
const HIGH_CRTC_MASK: u32 = 0x3e;
const SECONDARY: u32 = 0x2000_0000;
const INTERLACE: u32 = 1 << 4;
const DOUBLESCAN: u32 = 1 << 5;
const RECHECK_NS: u64 = 1_000_000_000;
const MAX_CACHED_DISPLAYS: usize = 32;

// C layouts from libdrm's xf86drm.h and xf86drmMode.h. In particular the
// timestamp and signal fields use C long, which differs on 32-bit targets.
#[repr(C)]
#[derive(Clone, Copy)]
struct Request {
    kind: u32,
    sequence: u32,
    signal: c_ulong,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Reply {
    kind: u32,
    sequence: u32,
    seconds: c_long,
    microseconds: c_long,
}

#[repr(C)]
pub union VBlank {
    request: Request,
    reply: Reply,
}

#[repr(C)]
struct Resources {
    count_fbs: c_int,
    fbs: *mut u32,
    count_crtcs: c_int,
    crtcs: *mut u32,
    count_connectors: c_int,
    connectors: *mut u32,
    count_encoders: c_int,
    encoders: *mut u32,
    min_width: u32,
    max_width: u32,
    min_height: u32,
    max_height: u32,
}

#[repr(C)]
#[derive(Default)]
struct Mode {
    clock_khz: u32,
    hdisplay: u16,
    hsync_start: u16,
    hsync_end: u16,
    htotal: u16,
    hskew: u16,
    vdisplay: u16,
    vsync_start: u16,
    vsync_end: u16,
    vtotal: u16,
    vscan: u16,
    vrefresh: u32,
    flags: u32,
    kind: u32,
    name: [c_char; 32],
}

#[repr(C)]
struct Crtc {
    id: u32,
    buffer_id: u32,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    mode_valid: c_int,
    mode: Mode,
    gamma_size: c_int,
}

type Wait = unsafe extern "C" fn(c_int, *mut VBlank) -> c_int;
type GetResources = unsafe extern "C" fn(c_int) -> *mut Resources;
type FreeResources = unsafe extern "C" fn(*mut Resources);
type GetCrtc = unsafe extern "C" fn(c_int, u32) -> *mut Crtc;
type FreeCrtc = unsafe extern "C" fn(*mut Crtc);

struct ModeApi {
    get_resources: GetResources,
    free_resources: FreeResources,
    get_crtc: GetCrtc,
    free_crtc: FreeCrtc,
}

struct Libdrm {
    wait: Wait,
    modes: Option<ModeApi>,
}

fn libdrm() -> Option<&'static Libdrm> {
    static API: OnceLock<Option<Libdrm>> = OnceLock::new();
    API.get_or_init(|| unsafe {
        // Search after the executable so this cannot resolve our own symbol.
        let wait = libc::dlsym(libc::RTLD_NEXT, c"drmWaitVBlank".as_ptr());
        if wait.is_null() {
            return None;
        }
        let resources = libc::dlsym(libc::RTLD_NEXT, c"drmModeGetResources".as_ptr());
        let free_resources = libc::dlsym(libc::RTLD_NEXT, c"drmModeFreeResources".as_ptr());
        let crtc = libc::dlsym(libc::RTLD_NEXT, c"drmModeGetCrtc".as_ptr());
        let free_crtc = libc::dlsym(libc::RTLD_NEXT, c"drmModeFreeCrtc".as_ptr());
        let modes = if [resources, free_resources, crtc, free_crtc]
            .iter()
            .any(|pointer| pointer.is_null())
        {
            None
        } else {
            Some(ModeApi {
                get_resources: std::mem::transmute::<*mut c_void, GetResources>(resources),
                free_resources: std::mem::transmute::<*mut c_void, FreeResources>(free_resources),
                get_crtc: std::mem::transmute::<*mut c_void, GetCrtc>(crtc),
                free_crtc: std::mem::transmute::<*mut c_void, FreeCrtc>(free_crtc),
            })
        };
        Some(Libdrm {
            wait: std::mem::transmute::<*mut c_void, Wait>(wait),
            modes,
        })
    })
    .as_ref()
}

fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        !matches!(
            std::env::var("CODEMUX_VBLANK_FALLBACK").as_deref(),
            Ok("0") | Ok("false") | Ok("off")
        )
    })
}

fn request_crtc(request: Request) -> Option<usize> {
    // Never invent absolute sequence semantics, events, signals, flips or
    // NEXTONMISS behavior. Only the two blocking requests WebKit uses qualify.
    if request.kind & !(RELATIVE | HIGH_CRTC_MASK | SECONDARY) != 0
        || request.kind & RELATIVE == 0
        || request.sequence > 1
        || request.signal != 0
    {
        return None;
    }
    let high = (request.kind & HIGH_CRTC_MASK) >> 1;
    if high != 0 && request.kind & SECONDARY != 0 {
        return None;
    }
    Some(if high != 0 {
        high as usize
    } else {
        usize::from(request.kind & SECONDARY != 0)
    })
}

fn mode_period_ns(mode: &Mode) -> Option<u64> {
    if mode.clock_khz == 0 || mode.htotal == 0 || mode.vtotal == 0 {
        return None;
    }
    // clock is kHz. Retain sub-Hz precision instead of using rounded vrefresh.
    let mut numerator = u64::from(mode.htotal)
        .checked_mul(u64::from(mode.vtotal))?
        .checked_mul(1_000_000)?;
    let mut denominator = u64::from(mode.clock_khz);
    if mode.flags & INTERLACE != 0 {
        denominator = denominator.checked_mul(2)?;
    }
    if mode.flags & DOUBLESCAN != 0 {
        numerator = numerator.checked_mul(2)?;
    }
    numerator = numerator.checked_mul(u64::from(mode.vscan.max(1)))?;
    let period = numerator / denominator;
    // Fail closed on nonsensical mode data (outside 1–1000 Hz).
    (1_000_000..=1_000_000_000)
        .contains(&period)
        .then_some(period)
}

impl ModeApi {
    fn period(&self, fd: c_int, index: usize) -> Option<u64> {
        unsafe {
            let resources = (self.get_resources)(fd);
            if resources.is_null() {
                return None;
            }
            let resources_ref = &*resources;
            let id = if index < resources_ref.count_crtcs.max(0) as usize
                && !resources_ref.crtcs.is_null()
            {
                Some(*resources_ref.crtcs.add(index))
            } else {
                None
            };
            (self.free_resources)(resources);
            let crtc = (self.get_crtc)(fd, id?);
            if crtc.is_null() {
                return None;
            }
            let period = if (*crtc).mode_valid != 0 {
                mode_period_ns(&(*crtc).mode)
            } else {
                None
            };
            (self.free_crtc)(crtc);
            period
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct DisplayKey {
    device: u64,
    inode: u64,
    rdev: u64,
    crtc: usize,
}

impl DisplayKey {
    fn for_fd(fd: c_int, crtc: usize) -> Option<Self> {
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        if unsafe { libc::fstat(fd, stat.as_mut_ptr()) } != 0 {
            return None;
        }
        let stat = unsafe { stat.assume_init() };
        if stat.st_mode & libc::S_IFMT != libc::S_IFCHR {
            return None;
        }
        // Identity, not descriptor number: fd reuse and different cards must
        // never reuse another display's cached period.
        Some(Self {
            device: stat.st_dev as u64,
            inode: stat.st_ino as u64,
            rdev: stat.st_rdev as u64,
            crtc,
        })
    }
}

#[derive(Clone, Copy)]
struct CachedMode {
    checked_ns: u64,
    period_ns: u64,
    sequence_offset: u32,
}

#[derive(Default)]
struct ModeCache(HashMap<DisplayKey, CachedMode>);

impl ModeCache {
    fn period(
        &mut self,
        key: DisplayKey,
        now: u64,
        probe: bool,
        query: impl FnOnce() -> Option<u64>,
    ) -> Option<u64> {
        if !probe {
            if let Some(mode) = self.0.get(&key) {
                if now.checked_sub(mode.checked_ns)? < RECHECK_NS {
                    return Some(mode.period_ns);
                }
            }
        }
        // Remove the old entry before querying: a disconnected/invalid mode
        // cannot keep a stale clock alive after revalidation fails.
        let previous = self.0.remove(&key);
        let period_ns = query()?;
        // Keep the emulated counter continuous across refresh-rate changes.
        // The physical driver's counter is unavailable; uptime supplies the
        // initial epoch, with an offset preserving subsequent continuity.
        let sequence_offset = previous.map_or(0, |mode| {
            mode.sequence_offset
                .wrapping_add((now / mode.period_ns) as u32)
                .wrapping_sub((now / period_ns) as u32)
        });
        if self.0.len() >= MAX_CACHED_DISPLAYS {
            if let Some(oldest) = self
                .0
                .iter()
                .min_by_key(|(_, mode)| mode.checked_ns)
                .map(|(key, _)| *key)
            {
                self.0.remove(&oldest);
            }
        }
        self.0.insert(
            key,
            CachedMode {
                checked_ns: now,
                period_ns,
                sequence_offset,
            },
        );
        Some(period_ns)
    }
}

fn monotonic_ns() -> Option<u64> {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) } != 0 {
        return None;
    }
    u64::try_from(time.tv_sec)
        .ok()?
        .checked_mul(1_000_000_000)?
        .checked_add(u64::try_from(time.tv_nsec).ok()?)
}

fn next_deadline(now: u64, period: u64) -> Option<u64> {
    (now / period).checked_add(1)?.checked_mul(period)
}

fn emulate(api: &ModeApi, fd: c_int, request: Request, index: usize) -> Option<Reply> {
    static CACHE: OnceLock<Mutex<ModeCache>> = OnceLock::new();
    static ANNOUNCED: OnceLock<()> = OnceLock::new();
    let now = monotonic_ns()?;
    let key = DisplayKey::for_fd(fd, index)?;
    let (period, sequence_offset) = {
        let mut cache = CACHE
            .get_or_init(|| Mutex::new(ModeCache::default()))
            .lock()
            .ok()?;
        let period = cache.period(key, now, request.sequence == 0, || api.period(fd, index))?;
        (period, cache.0.get(&key)?.sequence_offset)
    };
    // The mutex is released before waiting: separate displays never serialize
    // their clocks. Absolute deadlines avoid accumulating sleep/work drift.
    if request.sequence == 1 {
        let deadline = next_deadline(monotonic_ns()?, period)?;
        let time = libc::timespec {
            tv_sec: (deadline / 1_000_000_000).try_into().ok()?,
            tv_nsec: (deadline % 1_000_000_000).try_into().ok()?,
        };
        loop {
            match unsafe {
                libc::clock_nanosleep(
                    libc::CLOCK_MONOTONIC,
                    libc::TIMER_ABSTIME,
                    &time,
                    std::ptr::null_mut(),
                )
            } {
                0 => break,
                libc::EINTR => continue,
                _ => return None,
            }
        }
    }
    let actual = monotonic_ns()?;
    let reply = Reply {
        // libdrm clears RELATIVE before returning, including successful waits.
        kind: request.kind & !RELATIVE,
        sequence: ((actual / period) as u32).wrapping_add(sequence_offset),
        seconds: (actual / 1_000_000_000).try_into().ok()?,
        microseconds: ((actual % 1_000_000_000) / 1000).try_into().ok()?,
    };
    ANNOUNCED.get_or_init(|| {
        // Logging must not panic across the C ABI if stderr is closed.
        let _ = writeln!(
            std::io::stderr().lock(),
            "[codemux::vblank] DRM vblank waits unavailable; using display-mode timer ({:.2} Hz initially). Timer is not synchronized to scanout. Set CODEMUX_VBLANK_FALLBACK=0 to disable.",
            1_000_000_000.0 / period as f64
        );
    });
    Some(reply)
}

/// libdrm-compatible entry point exported by the Linux GUI executable.
///
/// # Safety
/// A non-null `vblank` must point to a writable libdrm `drmVBlank` union.
#[no_mangle]
#[allow(non_snake_case)]
pub unsafe extern "C" fn drmWaitVBlank(fd: c_int, vblank: *mut VBlank) -> c_int {
    let Some(drm) = libdrm() else {
        *libc::__errno_location() = libc::ENOSYS;
        return -1;
    };
    wait_with(drm, fd, vblank, enabled)
}

unsafe fn wait_with(
    drm: &Libdrm,
    fd: c_int,
    vblank: *mut VBlank,
    fallback_enabled: impl FnOnce() -> bool,
) -> c_int {
    if vblank.is_null() {
        return (drm.wait)(fd, vblank);
    }
    // libdrm may mutate its input even on failure. Preserve the original
    // request for classification, and leave libdrm's output untouched on every
    // failure/pass-through path.
    let original = (*vblank).request;
    let result = (drm.wait)(fd, vblank);
    let original_errno = *libc::__errno_location();
    if result == 0 || original_errno != libc::EOPNOTSUPP || !fallback_enabled() {
        *libc::__errno_location() = original_errno;
        return result;
    }
    let reply = request_crtc(original).and_then(|index| {
        drm.modes
            .as_ref()
            .and_then(|api| emulate(api, fd, original, index))
    });
    if let Some(reply) = reply {
        (*vblank).reply = reply;
        *libc::__errno_location() = 0;
        0
    } else {
        *libc::__errno_location() = original_errno;
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(kind: u32, sequence: u32) -> Request {
        Request {
            kind,
            sequence,
            signal: 0,
        }
    }

    fn mode(refresh_clock_khz: u32) -> Mode {
        Mode {
            clock_khz: refresh_clock_khz,
            htotal: 1000,
            vtotal: 1000,
            ..Mode::default()
        }
    }

    fn key(device: u64, crtc: usize) -> DisplayKey {
        DisplayKey {
            device,
            inode: 1,
            rdev: device,
            crtc,
        }
    }

    #[test]
    fn preserves_native_results_errors_and_mutated_output() {
        // Use fd as the mock's errno; zero simulates native success. errno is
        // thread-local so parallel tests cannot disturb another caller.
        unsafe extern "C" fn mock_wait(error: c_int, vblank: *mut VBlank) -> c_int {
            if !vblank.is_null() {
                (*vblank).request = Request {
                    kind: 0,
                    sequence: 99,
                    signal: 0,
                };
            }
            *libc::__errno_location() = error;
            if error == 0 {
                0
            } else {
                -1
            }
        }
        let drm = Libdrm {
            wait: mock_wait,
            modes: None,
        };
        for (error, allow) in [
            (0, true),
            (libc::EINVAL, true),
            (libc::EPERM, true),
            (libc::EOPNOTSUPP, false),
            (libc::EOPNOTSUPP, true),
        ] {
            let mut vblank = VBlank {
                request: request(RELATIVE, 1),
            };
            let result = unsafe { wait_with(&drm, error, &mut vblank, || allow) };
            assert_eq!(result, if error == 0 { 0 } else { -1 });
            assert_eq!(unsafe { *libc::__errno_location() }, error);
            assert_eq!(unsafe { vblank.request.sequence }, 99);
            assert_eq!(unsafe { vblank.request.kind }, 0);
        }
        assert_eq!(
            unsafe { wait_with(&drm, libc::EINVAL, std::ptr::null_mut(), || true) },
            -1
        );
    }

    #[test]
    fn accepts_only_webkit_blocking_relative_requests() {
        assert_eq!(request_crtc(request(RELATIVE, 0)), Some(0));
        assert_eq!(request_crtc(request(RELATIVE, 1)), Some(0));
        assert_eq!(request_crtc(request(RELATIVE | SECONDARY, 1)), Some(1));
        assert_eq!(request_crtc(request(RELATIVE | (5 << 1), 1)), Some(5));
        assert_eq!(
            request_crtc(request(RELATIVE | SECONDARY | (2 << 1), 1)),
            None
        );
        assert_eq!(request_crtc(request(0, 1)), None);
        assert_eq!(request_crtc(request(RELATIVE, 2)), None);
        for flag in [
            0x0400_0000,
            0x0800_0000,
            0x1000_0000,
            0x4000_0000,
            0x8000_0000,
        ] {
            assert_eq!(request_crtc(request(RELATIVE | flag, 1)), None);
        }
        assert_eq!(
            request_crtc(Request {
                signal: 1,
                ..request(RELATIVE, 1)
            }),
            None
        );
    }

    #[test]
    fn follows_refresh_rates_without_rounding_to_integer_hz() {
        for (hz, period) in [
            (24, 41_666_666),
            (60, 16_666_666),
            (144, 6_944_444),
            (200, 5_000_000),
            (240, 4_166_666),
        ] {
            assert_eq!(mode_period_ns(&mode(hz * 1000)), Some(period));
        }
        let fractional = Mode {
            clock_khz: 148_352,
            htotal: 2200,
            vtotal: 1125,
            vrefresh: 60,
            ..Mode::default()
        };
        assert_eq!(mode_period_ns(&fractional), Some(16_683_293));
    }

    #[test]
    fn handles_mode_scan_flags_and_rejects_invalid_modes() {
        let mut m = mode(60_000);
        m.flags = INTERLACE;
        assert_eq!(mode_period_ns(&m), Some(8_333_333));
        m.flags = DOUBLESCAN;
        assert_eq!(mode_period_ns(&m), Some(33_333_333));
        m.flags = 0;
        m.vscan = 2;
        assert_eq!(mode_period_ns(&m), Some(33_333_333));
        assert_eq!(mode_period_ns(&mode(0)), None);
        m.htotal = 0;
        assert_eq!(mode_period_ns(&m), None);
        m.htotal = u16::MAX;
        m.vtotal = u16::MAX;
        m.vscan = u16::MAX;
        assert_eq!(mode_period_ns(&m), None);
    }

    #[test]
    fn caches_per_device_and_crtc_and_refreshes_modes() {
        let mut cache = ModeCache::default();
        let first = key(1, 0);
        let second = key(1, 1);
        let another_card = key(2, 0);
        assert_eq!(
            cache.period(first, 0, false, || Some(5_000_000)),
            Some(5_000_000)
        );
        assert_eq!(
            cache.period(second, 0, false, || Some(16_666_666)),
            Some(16_666_666)
        );
        assert_eq!(
            cache.period(another_card, 0, false, || Some(6_944_444)),
            Some(6_944_444)
        );
        assert_eq!(
            cache.period(first, RECHECK_NS - 1, false, || panic!("cached")),
            Some(5_000_000)
        );
        assert_eq!(
            cache.period(first, RECHECK_NS, false, || Some(4_166_666)),
            Some(4_166_666)
        );
        let changed = cache.0.get(&first).unwrap();
        assert_eq!(
            ((RECHECK_NS / changed.period_ns) as u32).wrapping_add(changed.sequence_offset),
            (RECHECK_NS / 5_000_000) as u32
        );
        assert_eq!(
            cache.period(first, RECHECK_NS + 1, true, || Some(8_333_333)),
            Some(8_333_333)
        );
        let changed = cache.0.get(&first).unwrap();
        assert_eq!(
            (((RECHECK_NS + 1) / changed.period_ns) as u32).wrapping_add(changed.sequence_offset),
            200
        );
        assert_eq!(cache.period(second, RECHECK_NS, false, || None), None);
        assert!(!cache.0.contains_key(&second));
    }

    #[test]
    fn bounds_cache_and_uses_absolute_deadlines() {
        let mut cache = ModeCache::default();
        for i in 0..100 {
            cache.period(key(i, 0), i, false, || Some(5_000_000));
        }
        assert_eq!(cache.0.len(), MAX_CACHED_DISPLAYS);
        assert!(!cache.0.contains_key(&key(0, 0)));
        assert_eq!(next_deadline(11_000_000, 5_000_000), Some(15_000_000));
        assert_eq!(next_deadline(15_000_000, 5_000_000), Some(20_000_000));
        assert_eq!(next_deadline(u64::MAX, 5_000_000), None);
    }

    #[test]
    fn layouts_match_libdrm_headers() {
        assert_eq!(std::mem::size_of::<Mode>(), 68);
        assert_eq!(std::mem::offset_of!(Crtc, mode), 28);
        assert_eq!(std::mem::size_of::<Crtc>(), 100);
        #[cfg(target_pointer_width = "64")]
        {
            assert_eq!(std::mem::size_of::<VBlank>(), 24);
            assert_eq!(std::mem::size_of::<Resources>(), 80);
            assert_eq!(std::mem::offset_of!(Resources, crtcs), 24);
        }
        #[cfg(target_pointer_width = "32")]
        {
            assert_eq!(std::mem::size_of::<VBlank>(), 16);
            assert_eq!(std::mem::size_of::<Resources>(), 48);
            assert_eq!(std::mem::offset_of!(Resources, crtcs), 12);
        }
    }
}
