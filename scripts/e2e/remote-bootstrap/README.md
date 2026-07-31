# One-command remote bootstrap — end-to-end harness

Proves the claim `scripts/install.sh` + `codemux connect` make on a fresh
headless box:

> One command takes a machine from "nothing configured" to "reachable from
> anywhere, and still reachable after you log out and after it reboots."

against **real release artifacts** in **real systemd containers**, with a mock
account API standing in for `api.codemux.org`.

```sh
# build the artifacts once (~11 min on this host)
npm run tauri -- build --bundles deb,rpm

# then
scripts/e2e/remote-bootstrap/run.sh            # both distros
scripts/e2e/remote-bootstrap/run.sh ubuntu     # just one
scripts/e2e/remote-bootstrap/run.sh all --artifact /path/to/codemux.AppImage
scripts/e2e/remote-bootstrap/run.sh ubuntu --keep   # leave it up to poke at
```

Exit status is 0 only when every assertion passed; a ✓/✗ summary table is
printed at the end.

## What is here

| file | role |
|------|------|
| `run.sh` | the orchestrator — discovery, lifecycle, every assertion |
| `docker-compose.yml` | mock API + one systemd container per distro on a private bridge |
| `Dockerfile.ubuntu` | Ubuntu 24.04, systemd as PID 1 — the `.deb` half |
| `Dockerfile.fedora` | Fedora 41, systemd as PID 1 — the `.rpm` half |
| `Dockerfile.mock-api` | `python:3.12-slim` wrapper for the mock |
| `mock-api/server.py` | the mock account API + request recorder (stdlib only) |

## The flow it runs, per distro

| step | what runs | what it proves |
|------|-----------|----------------|
| a | `CODEMUX_ARTIFACT=… sh install.sh` | the installer works from a local artifact, resolves runtime deps through the package manager, and `codemux capabilities` reports the expected version |
| b | `CODEMUX_PASSWORD=… codemux login --email …` | headless sign-in against the API; `codemux whoami` reads the persisted session back; the mock recorded the signin POST |
| c | `codemux connect` | unit file written with `Environment=CODEMUX_API_URL`, `systemctl --user is-active` = `active`, `is-enabled` = `enabled`, linger on, port 4377 bound, `/api/health` OK locally **and from another container** |
| c2 | `curl http://localhost:4377/` | the release build really embeds the web UI (HTML + `/assets/` bundle), not just the pairing API |
| d | `codemux remote pair` | the control socket is up, so a second SSH session can mint a pairing code against the running unit — and the pairing URL host is the container's eth0 IP, **not** `127.0.0.1` |
| d2 | a second `codemux serve` | it exits non-zero with the mutual-exclusion message, **not** `Address already in use` — the guard keys off the control endpoint, so it only works if `serve` took it |
| e | poll the mock's request log | relay mode really started iroh and registered headlessly: a `POST /api/devices` arrived with the session bearer, a 64-hex `nodeId`, a `deviceId`, `platform: linux`, and the host's name; the desktop's own `registration.registered` reads back true over the control socket |
| e1b | `/proc/<serve-pid>/environ` | every account-API call in the run went to the mock — the live process names it and never `api.codemux.org` |
| e2 | `systemctl --user restart codemux.service` | a no-flag `serve` boot leaves the persisted settings row byte-identical: `relay_mode_enabled` stays true, port and scope are not reset |
| e3 | `remote enable --port 4399` + restart | `serve` binds the *persisted* port with no `--port` anywhere, then restores 4377 |
| f | `connect status`, `connect off`, re-`connect`, re-`install.sh` | status reports relay on, a device id, and `Codemux running: yes`; `off` removes the unit and really turns relay off (on disk too) without signing out; both `connect` and `install.sh` are idempotent |
| g | `docker restart <target>` | after a cold boot with **nobody logged in**, linger brought the user manager back, the unit auto-started, 4377 is bound, and the box is still reachable cross-container |

## Design notes

### Why SSH instead of `docker exec`

This is the crux of running the test at all.

`systemctl --user` needs a per-user systemd instance (`user@<uid>.service`) and
its bus at `$XDG_RUNTIME_DIR/bus`. Those are created by **logind**, when
**pam_systemd** opens a session. A `docker exec` shell has no PAM, therefore no
session, therefore no user bus — `systemctl --user` fails with
`Failed to connect to bus`. `codemux connect` probes exactly this
(`systemd_user_available` → `systemctl --user show-environment`), so under
`docker exec` it would correctly take its *fallback* branch and print
"No background service was installed" — testing the opposite of what we want.

So the containers run `sshd`, the harness generates a throwaway ed25519 key per
run, and every command in the flow goes over a real SSH login as an
unprivileged user. That is byte-for-byte the situation `codemux connect`
targets (someone SSH'd into a VPS), and it makes the *logout* half of the
promise real rather than assumed: linger is what keeps the unit alive once that
SSH session ends.

Root-side setup and inspection (installing the pubkey, `ss -ltnp`,
`journalctl`, `docker restart`) still go over `docker exec` — those are the
harness looking at the box, not the flow under test.

The host reaches the containers on their bridge IPs directly, so **no ports are
published**; nothing in this harness is reachable from outside the machine.

### The containers are NOT privileged

No `privileged: true` and no `security_opt` loosening — the default seccomp and
AppArmor/SELinux profiles still apply and the containers get no extra devices.
The whole concession is:

```yaml
cgroup: host                       # docker run --cgroupns=host
volumes:
  - /sys/fs/cgroup:/sys/fs/cgroup  # read-write
tmpfs: [/run, /run/lock, /tmp]
cap_add: [SYS_ADMIN]               # for polkitd only — see below
```

What systemd as PID 1 needs is **a writable cgroup hierarchy**. Without it PID 1
dies immediately with

```
Failed to create /init.scope control group: Read-only file system
Failed to allocate manager object.
```

Capabilities are *not* what systemd is missing: `--cap-add SYS_ADMIN` alone
still fails the same way, and with the hierarchy writable systemd boots with no
added caps at all. Measured on this host (Docker 29.6, cgroup v2, systemd
cgroup driver):

| configuration | result |
|---|---|
| defaults | PID 1 exits: `/init.scope … Read-only file system` |
| `--cap-add SYS_ADMIN` | same failure |
| `--cgroupns=private -v /sys/fs/cgroup:rw` | PID 1 exits: `/init.scope … No such file or directory` |
| `--cgroupns=private --cap-add SYS_ADMIN --tmpfs /sys/fs/cgroup` | PID 1 exits |
| `--cgroupns=host -v /sys/fs/cgroup:rw`, no caps | boots; logind active — but `polkit.service` fails 217/USER, so linger is denied |
| **`--cgroupns=host -v /sys/fs/cgroup:rw --cap-add SYS_ADMIN`** | **boots; logind active; polkitd up; linger works** |

`cgroup: host` is required rather than cosmetic: with a *private* cgroup
namespace, the bind-mounted hierarchy and the namespaced path systemd derives
from `/proc/1/cgroup` disagree, and systemd dies with `ENOENT`.

It is also narrower than it sounds. systemd reads its own cgroup from
`/proc/1/cgroup` and creates everything beneath it, so the units under test
live inside this container's own Docker scope — verified:

```
CGroup: /system.slice/docker-6374d089….scope/system.slice/sshd.service
```

The residual concession is that the hierarchy is mounted read-write and the
host's cgroup tree is visible, so a *malicious* payload in the container could
write outside that scope. For a harness running artifacts this repo just built,
that is an accepted trade; the alternative (`privileged`) is strictly worse.

`STOPSIGNAL SIGRTMIN+3` makes `docker restart` a clean systemd shutdown rather
than a SIGKILL — otherwise step (g) would be testing crash recovery, not
reboot.

### Why polkit is installed

`codemux connect` finishes with `loginctl enable-linger`, which logind
delegates to polkit's `org.freedesktop.login1.set-self-linger` (`allow_active`).
On an image with no polkit at all, every non-root caller is denied and connect
reports:

```
⚠ Could not keep the service alive past logout (Could not enable linger: Access denied).
```

which is correct behaviour but would silently reduce step (g) to a no-op. Real
Ubuntu/Fedora installs ship polkit, so the images do too.

`polkitd` is also the *only* reason `cap_add: [SYS_ADMIN]` is in the compose
file. Both distros ship `polkit.service` hardened with
`ProtectSystem=`/`ProtectHome=`/`PrivateTmp=`, and systemd must retain
`CAP_SYS_ADMIN` to build those mount namespaces before dropping to the
`polkitd` user. Without the capability the unit never starts:

```
polkit.service: Failed to keep CAP_SYS_ADMIN: Operation not permitted
polkit.service: Main process exited, code=exited, status=217/USER
```

and then logind has nobody to answer `set-self-linger`, so
`loginctl enable-linger` fails with a D-Bus timeout instead of a clean denial —
the same silent no-op, one layer down.

### Step (g) deliberately does not log in first

The post-reboot assertions run over `docker exec` with
`systemctl --user --machine=tester@.host`, **before** any SSH login. If the
harness logged in first, pam_systemd would start the user manager and the unit
would come back for that reason — proving nothing. Checking it with nobody
logged in is the only way linger is actually under test.

### Never the real API

Every container has `CODEMUX_API_URL=http://mock-api:8787` in its environment,
`codemux connect` propagates that into the unit file it writes
(`Environment=CODEMUX_API_URL=…`), and the mock lives on an internal compose
network with no egress requirement. No real credentials exist anywhere: the
password is a fixture string and the mock accepts it structurally (the wire
value is the Argon2-derived `AuthSecret`, which would mean vendoring the KDF to
verify).

The mock records every request it receives to
`/var/log/mock-api/requests.jsonl`, which is what makes step (e) an assertion
rather than a guess. `GET /_e2e/requests` returns the log as JSON.

#### Network note: relay mode really starts now

Because `codemux serve` runs the app's setup hook, relay mode genuinely comes
up, and iroh does what iroh does — it opens a QUIC endpoint and sends
discovery/holepunching **UDP toward n0's public relay servers**. So the
containers are not silent on the wire, and on a firewalled or offline host you
will see that traffic fail. It is deliberately non-blocking: registration
derives the `node_id` from the local endpoint, so `POST /api/devices` and every
assertion in step (e) succeed whether or not a relay is ever reached.

What must never happen is an *account API* call to the production host, and the
harness asserts that rather than assuming it (step e1b):

- the **live** `codemux serve` process's `/proc/<pid>/environ` must contain
  `CODEMUX_API_URL=http://mock-api:8787` and must not mention
  `api.codemux.org`;
- no file under `~/.config/systemd/user/` may reference `api.codemux.org`.

Together with the recorded signin and device POSTs on the mock, that pins every
account-API call in the run to the mock. Nothing in the harness ever resolves or
dials the real host.

## Requirements

- Docker with cgroup v2 (the containers are **not** privileged; see above)
- `docker compose` v2
- an `ssh` client and `ssh-keygen` on the host
- release artifacts under `src-tauri/target/release/bundle/` (or `--artifact`)

## Regressions this harness guards

Every defect this suite once tolerated as `⚠ KNOWN ISSUE` is fixed, so there is
no known-issue status any more: each one is a hard assertion and a regression
turns the suite **red**, not yellow.

The single root cause was that `codemux serve` never ran the app's Tauri `setup`
hook, so the control server was never spawned and `restore_on_boot()` never
hydrated the persisted web-remote config — after which `serve`'s own
`control_enable` wrote that un-restored default back over the settings row.
`build_headless_app()` now runs the hook explicitly, and the assertions that
pin each former symptom are:

| former known issue | now asserted by |
|---|---|
| `codemux remote pair` could not reach the running unit | step d — a `Pairing link:` line, and its host is the container IP |
| a second `serve` died on `Address already in use` | step d2 — non-zero exit with "already running on this machine", and *not* an address collision |
| no `POST /api/devices` was ever sent | step e — the recorded request, with bearer, `nodeId` (64-hex), `deviceId`, `platform`, hostname |
| `device_registered` stayed false | step e — read back over the control socket, and its `device_id`/`iroh_node_id` must match the ids the mock recorded |
| `serve` reset `relay_mode_enabled` to false on boot | step e2 — the settings row is byte-identical across a no-flag `serve` restart |
| persisted `port` / `bind_scope` were ignored | step e3 — a persisted 4399 is rebound with no `--port` anywhere |
| `connect status` said `Relay mode: off` | step f |
| `connect status` printed no `Device id:` | step f |
| `connect status` said `Codemux running: no` | step f |
| `connect off` reported relay "was already off" | step f, plus `relay_mode_enabled:false` on disk afterwards |

Tightening those turned up one *new* bug, which is the clearest argument for the
suite existing: `codemux connect off` removed the systemd unit **before** asking
the running instance to turn relay off. That was invisible while `serve`
published no control endpoint (`off` always took the headless DB path), but once
it does, the unit it just stopped *was* the instance — so `off` failed with

```
Failed to connect to Codemux control endpoint: Connection refused (os error 111)
```

after removing the unit, without turning relay off and without printing its
report. `run_connect_off` now does config-then-service, the same order
`run_connect` already used.

## Build caveats

- **The AppImage does not build on an Arch host.** Two independent blockers,
  both in `linuxdeploy` rather than in this repo:
  1. linuxdeploy ships its own (old) `binutils`, and its `strip` cannot parse
     the `.relr.dyn` sections modern Arch libraries carry:
     `unknown type [0x13] section '.relr.dyn'` → `Strip call failed` for
     essentially every bundled library, and the run aborts.
     `NO_STRIP=1` gets past this.
  2. With stripping skipped, the `gtk` plugin then aborts (exit 134) while
     deploying dependencies for the bun-compiled
     `codemux-claude-sidecar-*` — the same patchelf-corruption class of
     problem that made that sidecar a `resource` instead of an `externalBin`
     (see commit 025fa19).

  CI builds AppImages on `ubuntu-22.04`, where neither applies. The `.deb` and
  `.rpm` bundle fine here, and between them the matrix is fully covered, so
  the harness does not need the AppImage.
- **Fedora falls back to the AppImage** only when the build produced no `.rpm`.
  `install.sh` supports that path natively (`CODEMUX_METHOD=appimage`), so it
  is still a real installer path — just a different one.
- **`tauri build` exits 1 *after* bundling** unless `TAURI_SIGNING_PRIVATE_KEY`
  is set: the updater signing step runs last and fails with "A public key has
  been found, but no private key." Both bundles are already written at that
  point, so the harness is unaffected — just don't read the exit code as "no
  artifacts". Check `src-tauri/target/release/bundle/{deb,rpm}/` instead.
