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

Validation: current npm run check and 61 affected frontend tests pass; earlier
cargo check -j 2 and
7 scoped vblank_fallback Rust tests passed. Mock visual inspection could not be
completed: browser tooling initially lacked a control endpoint; the running dev
instance later returned "No pane found" for browser creation. Computer-use
reported no available browser. No system driver setting or installed app changed.

These probes do not record the physical mouse. Native GDK input and Electron
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
