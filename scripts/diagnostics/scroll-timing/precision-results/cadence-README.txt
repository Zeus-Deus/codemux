2026-10-06: periodic hiccup during the accepted fine-wheel glide

The previous candidate switched fine mouse input between 200ms and 30ms
segments at a 30ms event interval. Alternating 29/31ms samples reproduced
stops and surges without the transcript or virtualizer. Coalesced full ticks
also changed policy during a fine-wheel gesture.

Fine-wheel timing now learns the first interval, then averages four intervals
with a .75/.25 exponential estimate. Four times the estimate gives a duration
bounded by the existing 30/200ms endpoints. Existing velocity matching still
shortens a moving segment. The first isolated input keeps its 200ms glide;
a pause over 200ms resets cadence. Recognized fine mouse gestures keep full
packets on the same policy until idle. Touchpad timing remains 30ms.

cadence-results.json records ideal 5ms-frame curve measurements:
- Alternating 29/31ms: worst 5ms speed change .705 -> .051px/ms.
- Mixed fine/full packets: 2.495 -> .451px/ms.
- Sustained 5ms and sparse 80ms input: unchanged trajectories.
- Exact signed input travel preserved in every tested pattern.

Regression tests cover interval jitter, fast startup, exact travel, idle reset,
and forwarded fine/full packet policy. Run:
npm run test -- src/lib/wheel-scrolling.test.ts src/hooks/use-smooth-scrolling.test.tsx src/components/chat/MessageTrail.test.tsx
npm run check

Native production-bridge/frontend probe also passed: 64 value120=15 inputs,
8 deliberately duplicated legacy frames, exactly 64 trusted/cancelable DOM
wheel events, all prevented by the app, final DOM travel992px=input992px.
Reproduce with the current frontend server and compiled debug dependencies:
python scripts/diagnostics/scroll-timing/native_precision_probe.py --frontend-url http://localhost:1431 --clock-preload /tmp/codemux-scroll-timing/native_vblank_fallback.so --output /tmp/codemux-scroll-timing/precision-cadence.json

These checks establish animation and input behavior. They do not measure
fractional painted pixels or establish that long-chat rendering cannot stall.
Physical feedback on the revised dev app remains necessary.
