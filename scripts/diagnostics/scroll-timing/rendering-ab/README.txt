Read-only rendering A/B investigation; no frontend changes retained.

baseline, nomask, contain and mvcp use the native default clock.
clock* traces use the display-mode vblank fallback at 200 Hz on this machine.
clock320clean and clock335clean are paired runs without getter instrumentation.
The latter used the official LegendList 3.3.5 tarball through a temporary Vite alias.
No installed dependency tree was changed.

profile2, clockbaseline, clockrowcontain, clocknowrap and clockfasttext wrap DOM
geometry getters and record calls >= 2 ms. Instrumentation can affect timing.
All content is synthetic mock data. Stack traces point to local Vite modules.

The bottleneck is synchronous row measurement in LegendList's layout effect:
getBoundingClientRect forces 20–26 ms of layout for some newly mounted code cards.
Those rows have around 58 nodes and 229 characters of text, rather than huge prose.
Removing the viewport mask, adding containment, changing size anchoring, or
changing text shaping did not establish a material improvement in tail stalls.
LegendList 3.3.5 batches measurements and lowered average callback intervals in
one paired trial, but did not reduce p95. No dependency upgrade was retained.

Inputs: 250 native wheel events at 8 ms, 2560x1440, CodeMux mock in Vite dev mode.
These are callback intervals, not physical scanout or physical mouse measurements.
summary.json uses the same active-window boundary and nearest-rank percentile as
../analyze.py. Raw traces are preserved for reanalysis.
