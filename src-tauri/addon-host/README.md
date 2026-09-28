# Standalone add-on host

This crate has no Tauri dependency. Build and test it with its own manifest and
`-j 2`. It accepts JSON-RPC 2.0 over inherited pipes only; on Linux it closes
every other inherited descriptor before it starts. Each envelope has a
parent-selected generation. It exits when the parent closes stdin.

The first `initialize` carries protocolVersion 1, a validated manifest, a source
string chunk and `final`. Further `initialize` frames carry source chunks and
`final`. The aggregate source is capped at 5 MiB; each encoded frame stays below
1 MiB. A two-second timeout applies between chunks. No paths or bytecode cross
this initialization API. Bundles have no runtime imports.

Responses acknowledge accepted parent requests immediately. `ready` notifications
with phase `yielded` follow bounded synchronous work plus the promise-job drain;
phase `activated` follows successful SDK registration. Supervisors must require
both progress and actual activation, not confuse request acceptance with success.
A pending broker promise does not retain a synchronous execution budget.

The native engine caps heap, stack, burst duration and rolling active time. Rust
limits outgoing frames, request/log/UI rates, and input buffering. The SDK bounds
callbacks, pending requests and views. All child content remains untrusted: the
parent must independently enforce grants, contexts, callbacks, tree limits, RSS,
queueing, activation deadlines and disposal. This executable alone is not the
plugin platform. It is not an OS sandbox against native QuickJS vulnerabilities.

The intended Windows target is GNU, independently of the MSVC desktop target.
The cross-platform workflow runs this exact binary's process tests and SDK test.
Linux local results are recorded in docs/addons/IMPLEMENTATION.md. Unexecuted
Windows or packaged desktop checks must not be reported as passing.
