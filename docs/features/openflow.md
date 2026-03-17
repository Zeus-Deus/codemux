# OpenFlow Capability

- Purpose: Describe what OpenFlow is in Codemux and what currently works.
- Audience: Anyone working on orchestration, agent workflows, or OpenFlow UX.
- Authority: Canonical OpenFlow capability and constraints document.
- Update when: OpenFlow behavior, scope, or reliability expectations change.
- Read next: `docs/plans/openflow.md`, `docs/core/STATUS.md`

## What OpenFlow Is

OpenFlow is Codemux's multi-agent orchestration layer. It should behave like a first-class workspace or run type inside Codemux while keeping a modular runtime boundary that could be reused elsewhere later.

## What Works Today

- OpenFlow workspace and orchestration UI shell exist
- real agent PTYs can be spawned and monitored
- run records, phases, retry and cancel style controls, and shared communication logs exist
- sidebar visibility and orchestration monitoring exist in prototype form
- test agents can use Codemux browser automation
- extra diagnostics exist for wrapper lifecycle, native launch attribution, and OpenFlow breadcrumbs
- frontend OpenFlow state and orchestration helpers are now split into dedicated store/modules instead of one catch-all app-state file

## What Is Still Prototype-Level

- large multi-agent reliability, especially 15-20 agent runs
- browser view inside the OpenFlow workspace is still not the final integrated experience
- user questions versus change requests are not handled as cleanly as they should be
- dev-time lifecycle issues and single-instance hardening still need work

## Constraints

- treat current OpenFlow as a serious prototype, not a release-ready autonomous system
- Codemux is the primary host experience, but the runtime should stay modular
- the OpenFlow browser view currently mounts the shared default browser session rather than a run-scoped embedded browser surface
