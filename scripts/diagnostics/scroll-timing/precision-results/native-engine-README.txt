Native WebKit precision path isolation
=====================================

Reproduce from the repository with built Rust debug dependencies:
  python scripts/diagnostics/scroll-timing/native_precision_probe.py \
    --native \
    --clock-preload /tmp/codemux-scroll-timing/native_vblank_fallback.so \
    --output scripts/diagnostics/scroll-timing/precision-results/native-engine.json

--native and --frontend-url are mutually exclusive. Native mode installs only
passive wheel observers: it does not import/install the frontend wheel handler,
prevent default scrolling, or execute any JavaScript scroll setter. WebKit's
native smooth-scrolling setting is disabled. The existing production precise
wheel bridge receives 64 value120=15 frames and 8 corresponding GTK legacy
duplicates. The mock window declines focus and closes automatically.

Recorded result: 64 trusted, cancelable DOM wheel events, each 15.5px; total
992px, zero prevented events, and final native DOM scrollTop 992px. Per-event
rounding would produce 960px or 1024px, so this proves the native path retains
fractional accumulated distance. DOM getters still expose integer positions.

This is not pixel-level measurement of subpixel painting, input-to-photon
latency, or a real hardware-input comparison. rAF samples measure observable
DOM offsets and callbacks, not scanout. The trace includes an initial 46ms DOM
event gap followed by a batch; later input intervals are about 5ms. Do not use
the startup batch to infer a stable scrolling curve or smoothness guarantee.

Exact installed engine source (WebKitGTK 2.52.6) supports fractional native
painting, independently of this diagnostic:
https://github.com/WebKit/WebKit/blob/webkitgtk-2.52.6/Source/WebCore/page/scrolling/coordinated/ScrollingTreeScrollingNodeDelegateCoordinated.cpp#L55
  GTK's adjustedScrollPosition returns the FloatPoint unchanged.
https://github.com/WebKit/WebKit/blob/webkitgtk-2.52.6/Source/WebCore/platform/ScrollingEffectsController.cpp#L434
  With native smooth animation disabled, wheel delta goes to immediateScrollBy.
https://github.com/WebKit/WebKit/blob/webkitgtk-2.52.6/Source/WebCore/page/scrolling/coordinated/ScrollingTreeOverflowScrollingNodeCoordinated.cpp#L63
  The overflow layer receives a fractional currentScrollOffset as bounds origin.
https://github.com/WebKit/WebKit/blob/webkitgtk-2.52.6/Source/WebCore/platform/graphics/texmap/TextureMapperLayer.cpp#L270
  The compositor transform uses floating-point position minus bounds origin.

JavaScript writes use a different path:
https://github.com/WebKit/WebKit/blob/webkitgtk-2.52.6/Source/WebCore/dom/Element.cpp#L1749
  setScrollTop takes int; scrollTo also clamps coordinates to an IntPoint.
https://bugs.webkit.org/show_bug.cgi?id=188045
  Fractional Element scrollLeft/scrollTop support remains an open WebKit issue.
