Synthetic GTK wheel source classification diagnostic

Run from this worktree:
  /usr/bin/python scripts/diagnostics/scroll-timing/wheel_source_probe.py --output /tmp/wheel-source.json

The helper loads only a local mock document and the current frontend classifier.
It constructs a separate, temporary GdkWaylandDevice with the construct-only
TOUCHPAD source. No existing device is mutated. Real wheel events are stopped
before WebKit; only generated timestamp-zero events are sampled. No user input,
application contents or screenshots are recorded. The fixture selects SHM for
its isolated WebKit process; no desktop setting is changed.

wheel-source-results.json records one complete 24-event run on installed
WebKitGTK 2.52.6, GTK3, CSS viewport height 1390 and devicePixelRatio 1.
Both positive and negative smooth deltas were injected for each source.
Raw 0.125 units produced deltaY 15.5 / wheelDeltaY -15 with Mouse source,
and deltaY 5 / wheelDeltaY -15 with Touchpad source. All 12 Touchpad samples
were classified false. All 10 Mouse samples with nonzero legacy ticks were
classified true. Two very tiny Mouse samples (absolute raw delta 0.001) had
legacy ticks truncated to zero and stayed conservatively ambiguous/false.

The exact WebKitGTK conversion gives non-Mouse smooth input 40 pixels per raw
unit, while Mouse input uses floor(view height^(2/3)). DOM wheelDeltaY is the
opposite raw delta multiplied by 120 and truncated to an integer. Excluding the
whole possible 40-pixel interval therefore avoids changing genuine Touchpad
sample timing, without assuming a CSS viewport-to-GTK viewport relationship.
A Mouse view height whose scale rounds to 40 also remains ambiguous.

Primary source for installed version:
https://github.com/WebKit/WebKit/blob/webkitgtk-2.52.6/Source/WebKit/UIProcess/API/gtk/WebKitWebViewBase.cpp#L1374
https://github.com/WebKit/WebKit/blob/webkitgtk-2.52.6/Source/WebCore/dom/WheelEvent.cpp#L61
https://github.com/WebKit/WebKit/blob/webkitgtk-2.52.6/Source/WebCore/platform/Scrollbar.cpp#L65

This validates source conversion and classifier ownership in a native engine.
It does not measure a physical touchpad's motion or prove perceived smoothness.
