2026-10-06 fine-wheel pacing candidate
=====================================

Historical candidate: superseded by cadence-README.txt after physical testing
reported periodic hiccups at the 30ms cutoff. Its measurements remain below.

The previous 30ms precision filter has been replaced for confirmed Mouse input:
sparse fine-wheel input uses the accepted velocity-preserving glide; samples
<=30ms apart retain the responsive 30ms filter (minimum8ms). Full notches keep
the accepted baseline timing. Fine non-Mouse/ambiguous input keeps the prior
30ms timing. No seeded initial velocity, sensitivity multiplier, or fling added.
All destinations still accumulate signed physical input and fractional carry.

isWebKitMouseWheel excludes the entire possible non-Mouse40px/unit range,
accounting for integer truncation of wheelDeltaY. It does not infer device type
from delta size alone or depend on viewport height for source recognition.
Missing/zero/noninteger/wrong-sign ticks and overlapping ranges stay ambiguous.
Rail input is classified from the original native event before its synthetic
forwarder discards the original ticks.

Native bridge plus CURRENT adaptive frontend:64 trusted/cancelable15.5px inputs,
64 prevented DOM events, exact992px destination. See adaptive-native-and-frontend.

fine-short30 and fine-continuous200 were rerun with exactly77 ordered injections.
An earlier continuous200 trace had a Vite full reload and83 observed events;
it was replaced by the valid77-event run. glide_probe now schedules only once.
All new traces are written outside the watched repository until the probe exits.

Adaptive-comparison-summary separates sample counts and elapsed flat time.
The adaptive run occurred during a debug build and has fewer callback samples;
do not treat raw moving-frame counts as a controlled paint-quality comparison.
The all200ms candidate reduced slow pauses but lagged fast input (699/992px at
320ms in the deterministic5ms-frame harness), so it is NOT the chosen policy.
The adaptive candidate's tests require bounded fast-input lag and exact travel.

These are callback/DOM positions, not physical pixels, scanout, or proof that
the user will find it buttery smooth. Real desktop testing remains necessary.
Earlier native-and-frontend/short-filter-onset/research timing artifacts document
the previous fixed30ms candidate rather than the current adaptive policy.

The native interpolation diagnostic remains a mock experiment only. It changes
input event counts and lacks verified DOM scroll ownership and cancellation;
it has not been added to the user's app. No real input recording is running.
