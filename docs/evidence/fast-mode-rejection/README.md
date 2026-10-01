# Fast mode rejection evidence

Both screenshots use the development mock at `http://localhost:1421`, captured with `codemux browser screenshot`. Port 1420 was occupied. All conversation and workspace data come from repository fixtures.

The mock IPC rejects `agent_chat_set_fast_mode` with `tier unavailable (mock rejection)`. Select Cursor Auto, open the reasoning picker, and select Fast using End followed by Enter.

- `before.png`: the toggle handler from main (`b4b2ac61`) updates the indicator to Fast despite the rejection.
- `after.png`: this branch's handler retains Standard and shows the error.

The Rust regression tests separately verify rejected changes do not alter saved settings, missing sessions retain the choice for auto-resume, and disabled Claude Fast settings do not reach resumed SDK sessions. These screenshots do not measure live inference speed.
