Current accepted behavior and future investigation: see FINDINGS.txt.
The narrative below preserves historical trials; older candidate/test counts
are labeled by their phase and do not describe the final cadence revision.

Linux scroll timing investigation

All results are from this machine; environment.json records versions and limitations.
The raw traces record rAF timestamps/scrollTop, delivered native wheel deltas,
trusted event status, scroll events, and (for the CodeMux mock) DOM mutation timing.
summary.json uses the active input window through 250 ms after the last event for
p95/max frame intervals. Distance at input end is bracketed by adjacent rAF samples.

Implemented fixes, on branch investigate/linux-scroll-timing:
* src-tauri/src/vblank_fallback.rs supplies a display-mode-paced clock only when
  libdrm reports unsupported blocking relative vblank waits. Working clocks and
  other requests pass through. CODEMUX_VBLANK_FALLBACK=0 disables the fallback.
  This timer is not synchronized to physical scanout and does not implement VRR.
* src/lib/wheel-scrolling.ts now makes short wheel glides the Linux default.
  Small isolated ticks glide for about 200 ms with continuous velocity when
  retargeted; larger/faster input shortens the segment. Signed reversal preserves
  the requested destination. The obsolete appearance.smooth_scrolling preference
  and settings toggle are no longer used. Native controls, horizontal input and
  reduced motion keep their native path. Vertical trackpad input also glides.
  Capture reaches picker menus that stop bubbling, while explicit exclusions
  protect horizontal wheel owners and the transcript's forwarding rail.

Current glide evidence is in glide-results. The original optional 12 ms response
was rejected in the user's physical mouse test despite its better burst metrics:
74% of a tick within 16 ms is too abrupt for the intended glide. Those archived
mode-clock-* traces and timing-chart describe that first attempt, not a successful
physical-input fix or the current animation. New native WebKit injections include
single 25/120 px ticks, 50/100/150 ms spacing, a burst and reversal. An isolated
25 px tick progresses over ~200 ms instead of mostly moving in the first frame.
Five ticks 150 ms apart keep moving throughout the sequence, like the Electron
comparison. All 38 native events were intercepted both normally and with a child
stopping propagation. The horizontal-owner probe kept vertical movement at zero
and its own horizontal scrolling intact. The new rapid burst covered 90.6% of requested movement by input end and then
settled to the complete input distance within WebKit's 1 px quantization.
The actual dev executable was started and logged the 200 Hz fallback; no desktop
NVIDIA setting was changed. The user's physical-mouse test confirmed noticeably
smoother long-chat scrolling and fast wheel movement. Small initial up/down
movements still feel stepped; that remains a separate follow-up. Preserve this
tested animation as the baseline before changing its start or reversal curve.

Rejected follow-up start response: the trial started a fresh glide at 75% of its average speed,
rather than zero. Duration, signed destination and moving retarget velocity are
preserved. Native onset-results compare this against commit 8ef3cca0 in separate
GTK/WebKit probes using identical small up/down inputs. The ~25 px downward
tick's first recorded pixel changed from 37 ms to 16 ms; the upward tick's
second pixel changed from 37 ms to 18 ms. A ~120 px downward tick started at
8 ms versus 19 ms. All 11 alternating onset events were trusted and intercepted.
The separate 250-event rapid burst covered 92.8% by input end and settled to
6,349 px of 6,350 requested, retaining the subpixel remainder. Native callback
median stayed 5 ms. These are single paired trials of recorded scroll positions,
not physical input-to-presentation measurements. Very small ~5 px ticks still
have observable whole-pixel stepping; this does not eliminate WebKit rounding.
The trial passed npm run check and all 16 affected wheel-scrolling tests, but
the user reported an initial push. Its seeded velocity was removed; the accepted
zero-velocity baseline is retained. Callback onset alone is not sufficient to
validate physical wheel control.
To reproduce the onset pattern, pass `onset` after the glide_probe.py URL.

The after traces named mode-clock-* load the actual production Rust module in
an isolated GTK process, rather than the fixed diagnostic C shim. The native
executable export mechanism was separately verified with a linked probe; this
is not a benchmark of an installed full CodeMux release.
The final plain-page rapid-input trial covered 97.8% of 6,350 requested pixels
by the last input event in the first attempt; the previous native animation covered 11.2–12.4%.
Median animation callbacks changed from 16 ms to 5 ms on the 200 Hz display.
Timers delivered the nominal 2 ms wheel bursts over different durations
(577 ms before, 724 ms after). Do not interpret the curves as identical HID
recordings. The final 1 px difference is WebKit scrollTop quantization; the app
preserved subpixel remainder across gestures, covered by a regression test.
The new animator alone, on the old clock, covered 95.4% in its separate trial.

Rendering is a separate remaining limitation: the CodeMux mock transcript still
had 44 ms p95 and 98 ms maximum active callback intervals after the fixes. The
baseline was 49/107 ms; the Electron shell was 10/20.1 ms. Rendering A/B trials
are in rendering-ab and did not justify retaining a speculative library or CSS
change. No claim of eliminating every transcript hitch is supported.

Historical baseline validation: npm run check and 61 affected frontend tests passed; earlier
cargo check -j 2 and
7 scoped vblank_fallback Rust tests passed. Mock visual inspection could not be
completed: browser tooling initially lacked a control endpoint; the running dev
instance later returned "No pane found" for browser creation. Computer-use
reported no available browser. No system driver setting or installed app changed.

The original injected probes do not record the physical mouse. The later
bounded physical precision investigation is documented below. Native GDK input and Electron
sendInputEvent enter their browser event paths, but scheduling/batching differs.
The Electron comparison shell is the locally available 41.5.0 runtime; it is not
a measurement of the existing t3code process. Mock UI results use Vite development
mode. Callback cadence is not physical display presentation timing.

To repeat the plain WebKit probes (GTK3 + WebKit2 4.1 Python GI required):
  WEBKIT_DMABUF_RENDERER_FORCE_SHM=1 python webkit_probe.py off 8 > results/new-off.json
  WEBKIT_DMABUF_RENDERER_FORCE_SHM=1 python webkit_probe.py on 2 > results/new-on.json
To repeat the Electron probe (use a local Electron executable):
  /path/to/electron electron_probe.cjs 8 > results/new-electron.json
For the CodeMux mock, start the mock server in this worktree:
  npm run dev -- --port 1431 --strictPort --config scripts/diagnostics/scroll-timing/vite.config.ts
  WEBKIT_DMABUF_RENDERER_FORCE_SHM=1 python app_probe.py app > results/new-app.json
  /path/to/electron electron_app_probe.cjs > results/new-app-electron.json
SCROLL_PROBE_URL overrides the mock URL. Stop only the server you started.

To repeat the implemented fix, from this diagnostic directory:
  python build_native_probe.py
Use the printed absolute library path below. The build uses a separate temporary
Cargo package and cached libc; it does not change the application dependency tree.
  LD_PRELOAD=/printed/path/libcodemux_vblank_probe.so WEBKIT_DMABUF_RENDERER_FORCE_SHM=1 python glide_probe.py > glide-results/new-ticks.json
  LD_PRELOAD=/printed/path/libcodemux_vblank_probe.so WEBKIT_DMABUF_RENDERER_FORCE_SHM=1 python webkit_probe.py off 2 'http://localhost:1431/scripts/diagnostics/scroll-timing/page.html?smooth=app' > results/new-mode-wheel.json
  LD_PRELOAD=/printed/path/libcodemux_vblank_probe.so WEBKIT_DMABUF_RENDERER_FORCE_SHM=1 python app_probe.py app > results/new-mode-transcript.json
LD_PRELOAD is used only to test the Rust module in these isolated child probes.
The actual app exports the hook from its executable and does not use LD_PRELOAD.

The temporary diagnostic_vblank.c shim is a fixed 200 Hz timing experiment,
not a production fix and not synchronized with physical scanout. It changes
only the child process launched with LD_PRELOAD. Do not install it in the app.
  gcc -shared -fPIC -O2 -Wall -Wextra -Werror -I/usr/include/libdrm diagnostic_vblank.c -o /tmp/diagnostic_vblank.so -ldl
  LD_PRELOAD=/tmp/diagnostic_vblank.so WEBKIT_DMABUF_RENDERER_FORCE_SHM=1 python webkit_probe.py off 8 > results/new-clock.json

Rebuild summary and chart:
  python analyze.py
  python plot.py
  python glide-plot.py

The current ordinary-tick comparison is glide-chart.png/svg; timing-chart.png/svg
is the archived rapid-input investigation of the first attempted animator.

Physical input investigation (2026-10-05): a bounded read-only numeric wheel
recording of a G502 showed REL_WHEEL_HI_RES increments of +/-15 (one eighth of
a 120-unit notch). The paired real Tauri DOM sample showed 264 wheel events,
all deltaY +/-186 and deltaMode 0; 2,014 high-resolution increments and 257
coarse events occurred in the overlapping physical capture. The windows
partly differ at their boundaries, so these counts are not an exact event
matching assertion. Fine motion exists at the device and is absent at the DOM.
GTK3 binds wl_seat v5; axis_value120 requires v8. Hyprland 0.56.2 accumulates
fractional wheel values for the legacy path while preserving value120 on v8.
This is an input precision problem, independent of the fixed frame timing.
Temporary app recording collected only numeric wheel deltas/scroll positions
into /tmp and was removed immediately afterward; no telemetry is shipped.

Precision follow-up: src-tauri/src/precise_wheel.rs binds a version-8 pointer
on GTK's existing Wayland connection. Only own-surface mouse-wheel frames with
value120 are translated to native GDK smooth events, at value120/120 wheel
units. Matching GTK legacy copies are suppressed by input timestamp; native
replacement propagation copies are retained. Older compositors, X11, touchpads,
unmapped/unknown surfaces and unsupported multi-seat configurations keep GTK.
CODEMUX_PRECISE_WHEEL=0 disables the bridge. The protocol flag is learned once
at boot; only fractional pixel deltas use a 30 ms filter, with zero initial
velocity. Full notches retain the accepted longer glide. No per-device gain
calibration or input device permissions are required by the application.

The physical Wayland prototype received 2,129 fractional wheel frames while GTK
received 276 smooth events. The native WebKit module test received exactly 64
trusted, cancelable DOM wheel events of 15.5 px for 64 eighth-notch frames; eight
matching coarse GTK events added no movement. Total was 992 px. These native
module injection results are separate from the physical protocol observation,
and do not claim measured physical input-to-presentation latency. At the main
app's recorded 1,386 px viewport, one normal GTK wheel unit is 124 px: the new
eighth-notch input is 15.5 px. The old Hyprland compatibility path generated
1.5 units (186 px) only at coarse notches. Whole-notch distance now follows
standard wheel units rather than that compositor-specific legacy multiplier.

The short-filter WebKit trace precision-results/short-filter-onset.json shows
~25.3 px input settling within 1 px in 33-35 ms, and ~5 px input in 29-33 ms.
Full ~119.5 px notches still glide over ~200 ms. These are single diagnostic
callback traces, not an assertion that all perceived hitches are resolved.

Earlier short-filter frontend verification: npm run check and the 42 affected wheel, boot-hook
and MessageTrail tests passed. Precision capability discovery schedules native
initialization on the GTK thread before returning, avoiding a race with React
effects at startup. Unsupported backends retain the committed baseline.
