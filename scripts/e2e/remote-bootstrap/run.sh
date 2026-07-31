#!/usr/bin/env bash
#
# End-to-end verification of the one-command remote bootstrap.
#
#   ./run.sh [ubuntu|fedora|all] [--artifact <path>] [--keep]
#
# Takes a real release artifact (.deb / .rpm / .AppImage) and, inside a
# throwaway systemd container per distro, walks the whole promised path:
#
#   install.sh  ->  codemux login  ->  codemux connect  ->  reachable, and
#   still reachable after a reboot
#
# asserting each claim rather than eyeballing it. Nothing ever talks to the
# real api.codemux.org: every container gets CODEMUX_API_URL pointed at a mock
# on a private compose network, and that mock records every request so the
# device-registration POST can be asserted after the fact.
#
# Exit status is 0 only if every assertion passed.
#
# See README.md for the design notes (why SSH, why privileged, what each step
# proves).

set -uo pipefail

# ── Locations ────────────────────────────────────────────────────────────────

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${HERE}/../../.." && pwd)"
BUNDLE_DIR="${REPO_ROOT}/src-tauri/target/release/bundle"
COMPOSE_PROJECT="codemux-e2e-remote-bootstrap"

# ── Fixtures ─────────────────────────────────────────────────────────────────

E2E_USER="tester"
E2E_EMAIL="test@example.com"
E2E_PASSWORD="e2e-not-a-real-password"
E2E_TOKEN="e2e-session-token"
E2E_USER_ID="usr_e2e_1"
MOCK_API="http://mock-api:8787"
CODEMUX_PORT=4377

# ── Options ──────────────────────────────────────────────────────────────────

TARGETS=""
ARTIFACT_OVERRIDE=""
KEEP=0
REBUILD_IMAGES=0

usage() {
    sed -n '2,20p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
    cat <<'USAGE'

Options
  --artifact <path>   use this artifact for every distro instead of
                      discovering one per distro under
                      src-tauri/target/release/bundle/
  --keep              leave the containers running for poking at
  --rebuild           force `docker compose build --no-cache`
  -h, --help          this message
USAGE
}

while [ $# -gt 0 ]; do
    case "$1" in
        ubuntu|fedora|all) TARGETS="$1" ;;
        --artifact) shift; ARTIFACT_OVERRIDE="${1:-}" ;;
        --artifact=*) ARTIFACT_OVERRIDE="${1#--artifact=}" ;;
        --keep) KEEP=1 ;;
        --rebuild) REBUILD_IMAGES=1 ;;
        -h|--help) usage; exit 0 ;;
        *) echo "unknown option: $1 (try --help)" >&2; exit 2 ;;
    esac
    shift
done
[ -n "$TARGETS" ] || TARGETS="all"
case "$TARGETS" in
    all) DISTROS="ubuntu fedora" ;;
    *)   DISTROS="$TARGETS" ;;
esac

# ── Output ───────────────────────────────────────────────────────────────────

if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
    C_GREEN=$'\033[0;32m'; C_RED=$'\033[0;31m'; C_YELLOW=$'\033[0;33m'
    C_BLUE=$'\033[0;34m'; C_BOLD=$'\033[1m'; C_DIM=$'\033[2m'; C_RESET=$'\033[0m'
else
    C_GREEN=''; C_RED=''; C_YELLOW=''; C_BLUE=''; C_BOLD=''; C_DIM=''; C_RESET=''
fi

banner() { printf '\n%s══ %s %s\n' "$C_BOLD$C_BLUE" "$*" "$C_RESET"; }
info()   { printf '%s   %s%s\n' "$C_DIM" "$*" "$C_RESET"; }
warn()   { printf '%s ! %s%s\n' "$C_YELLOW" "$*" "$C_RESET" >&2; }
fatal()  { printf '%s ✗ %s%s\n' "$C_RED" "$*" "$C_RESET" >&2; exit 1; }

# Results table: one "distro|step|status|detail" row per assertion.
RESULTS=()
FAILURES=0
CURRENT_DISTRO=""

pass() {
    RESULTS+=("${CURRENT_DISTRO}|$1|PASS|${2:-}")
    printf '%s ✓ %s%s\n' "$C_GREEN" "$1" "$C_RESET"
}
fail() {
    RESULTS+=("${CURRENT_DISTRO}|$1|FAIL|${2:-}")
    FAILURES=$((FAILURES + 1))
    printf '%s ✗ %s%s\n' "$C_RED" "$1" "$C_RESET" >&2
    [ -n "${2:-}" ] && printf '%s     %s%s\n' "$C_RED" "$2" "$C_RESET" >&2
    return 0
}
skip() {
    RESULTS+=("${CURRENT_DISTRO}|$1|SKIP|${2:-}")
    printf '%s ○ %s (%s)%s\n' "$C_YELLOW" "$1" "${2:-skipped}" "$C_RESET"
}
# NOTE: there is deliberately no "known issue" status any more. Every defect
# this harness once tolerated that way (no control socket, relay mode reset on
# boot, no device registration, persisted port ignored) is fixed, so each one is
# now a hard assertion — a regression must turn the suite red, not yellow.

# assert_contains <step> <haystack> <needle> — the workhorse.
assert_contains() {
    local step="$1" haystack="$2" needle="$3"
    if printf '%s' "$haystack" | grep -qF -- "$needle"; then
        pass "$step"
    else
        fail "$step" "expected to find «${needle}»"
        printf '%s     ── actual output ──\n%s\n%s' "$C_DIM" "$haystack" "$C_RESET" >&2
    fi
}

assert_not_contains() {
    local step="$1" haystack="$2" needle="$3"
    if printf '%s' "$haystack" | grep -qF -- "$needle"; then
        fail "$step" "must NOT contain «${needle}»"
        printf '%s     ── actual output ──\n%s\n%s' "$C_DIM" "$haystack" "$C_RESET" >&2
    else
        pass "$step"
    fi
}

assert_eq() {
    local step="$1" actual="$2" expected="$3"
    if [ "$actual" = "$expected" ]; then
        pass "$step" "$actual"
    else
        fail "$step" "expected «${expected}», got «${actual}»"
    fi
}

assert_ne() {
    local step="$1" actual="$2" forbidden="$3"
    if [ "$actual" != "$forbidden" ]; then
        pass "$step" "$actual"
    else
        fail "$step" "must not be «${forbidden}»"
    fi
}

# ── Preflight ────────────────────────────────────────────────────────────────

command -v docker >/dev/null 2>&1 || fatal "docker not found on PATH"
docker info >/dev/null 2>&1 || fatal "cannot talk to the docker daemon"
docker compose version >/dev/null 2>&1 || fatal "docker compose (v2) not available"
command -v ssh >/dev/null 2>&1 || fatal "an ssh client is required (the harness logs into the containers)"

# ── Artifact discovery ───────────────────────────────────────────────────────

# The bundle layout tauri writes: bundle/deb/*.deb, bundle/rpm/*.rpm,
# bundle/appimage/*.AppImage.
discover_artifact() {
    local kind="$1" found=""
    case "$kind" in
        deb)      found=$(ls -1t "${BUNDLE_DIR}"/deb/*.deb 2>/dev/null | head -n 1) ;;
        rpm)      found=$(ls -1t "${BUNDLE_DIR}"/rpm/*.rpm 2>/dev/null | head -n 1) ;;
        appimage) found=$(ls -1t "${BUNDLE_DIR}"/appimage/*.AppImage 2>/dev/null | head -n 1) ;;
    esac
    printf '%s' "$found"
}

# What each distro installs. Fedora prefers the .rpm and falls back to the
# AppImage when no .rpm was produced (the rpm bundler is the one most likely to
# be missing on a given build host) — install.sh supports exactly that fallback
# via CODEMUX_METHOD=appimage, so testing it is not a cop-out.
artifact_for() {
    local distro="$1"
    if [ -n "$ARTIFACT_OVERRIDE" ]; then printf '%s' "$ARTIFACT_OVERRIDE"; return; fi
    case "$distro" in
        ubuntu) discover_artifact deb ;;
        fedora)
            local rpm; rpm=$(discover_artifact rpm)
            if [ -n "$rpm" ]; then printf '%s' "$rpm"; else discover_artifact appimage; fi
            ;;
    esac
}

# ── SSH plumbing ─────────────────────────────────────────────────────────────
#
# The flow runs over a real SSH login, not `docker exec`. That is the whole
# point: pam_systemd turns a login into a logind session, which starts
# `user@<uid>.service`, which is what makes `systemctl --user` (and therefore
# the entire step 3 of `codemux connect`) work at all. A `docker exec` shell
# has no session, so it would exercise the "no per-user systemd" fallback
# branch instead of the path a user on a VPS actually takes.

SSH_DIR=""
SSH_KEY=""
setup_ssh_key() {
    SSH_DIR=$(mktemp -d "${TMPDIR:-/tmp}/codemux-e2e-ssh.XXXXXX")
    SSH_KEY="${SSH_DIR}/id_ed25519"
    ssh-keygen -q -t ed25519 -N '' -C codemux-e2e -f "$SSH_KEY" </dev/null
}

SSH_OPTS=(
    -o StrictHostKeyChecking=no
    -o UserKnownHostsFile=/dev/null
    -o LogLevel=ERROR
    -o ConnectTimeout=10
    -o PasswordAuthentication=no
)

container_of() { printf 'codemux-e2e-%s' "$1"; }

container_ip() {
    docker inspect -f \
        "{{ (index .NetworkSettings.Networks \"${COMPOSE_PROJECT}\").IPAddress }}" \
        "$(container_of "$1")" 2>/dev/null
}

# Run as root inside the container (setup + inspection only — never the flow).
rexec() {
    local distro="$1"; shift
    docker exec "$(container_of "$distro")" "$@"
}

# Run the flow as the unprivileged user, over SSH, with CODEMUX_API_URL set —
# i.e. exactly what a person SSH'd into their box would type.
#
# Deliberately `bash -lc` so a login shell picks up /etc/profile.d (PATH for
# ~/.local/bin on the AppImage path) the same way an interactive login would.
uexec() {
    local distro="$1"; shift
    local ip; ip=$(container_ip "$distro")
    ssh "${SSH_OPTS[@]}" -i "$SSH_KEY" "${E2E_USER}@${ip}" \
        "CODEMUX_API_URL='${MOCK_API}' bash -lc $(printf '%q' "$*")" 2>&1
}

# Same, but with the password in the environment (the documented headless
# affordance `codemux login`/`connect` expose).
uexec_pw() {
    local distro="$1"; shift
    local ip; ip=$(container_ip "$distro")
    ssh "${SSH_OPTS[@]}" -i "$SSH_KEY" "${E2E_USER}@${ip}" \
        "CODEMUX_API_URL='${MOCK_API}' CODEMUX_PASSWORD='${E2E_PASSWORD}' bash -lc $(printf '%q' "$*")" 2>&1
}

# Curl from *inside the mock-api container* — proves the target's listener is
# reachable from another host on the network, which loopback-bound would not be.
mock_curl() {
    docker exec codemux-e2e-mock-api \
        python3 -c "
import json,sys,urllib.request
try:
    with urllib.request.urlopen('$1', timeout=8) as r:
        sys.stdout.write(r.read().decode('utf-8', 'replace'))
except Exception as e:
    sys.stdout.write('ERROR: %s' % e)
"
}

mock_requests() {
    docker exec codemux-e2e-mock-api cat /var/log/mock-api/requests.jsonl 2>/dev/null
}

# The persisted web-remote config, straight out of the settings table. Read
# with sqlite3 inside the container rather than by copying the file out: the
# store is in WAL mode, so a plain `docker cp` of the .db misses every recent
# write.
settings_row() {
    local distro="$1"
    rexec "$distro" sqlite3 "/home/${E2E_USER}/.config/codemux/codemux.db" \
        "select value from settings where key='web_remote.config'" 2>/dev/null | tr -d '\r\n'
}

# Poll until <port> is bound inside the container (or give up after ~60s).
wait_for_port() {
    local distro="$1" port="$2"
    for _ in $(seq 1 30); do
        if rexec "$distro" ss -ltn 2>/dev/null | grep -q ":${port} "; then
            return 0
        fi
        sleep 2
    done
    return 1
}

# ── Lifecycle ────────────────────────────────────────────────────────────────

compose() {
    ( cd "$HERE" && \
      CODEMUX_E2E_ARTIFACT_DIR="$ARTIFACT_DIR" \
      CODEMUX_E2E_REPO_SCRIPTS="${REPO_ROOT}/scripts" \
      CODEMUX_E2E_EMAIL="$E2E_EMAIL" \
      CODEMUX_E2E_TOKEN="$E2E_TOKEN" \
      CODEMUX_E2E_USER_ID="$E2E_USER_ID" \
      CODEMUX_E2E_USER="$E2E_USER" \
      docker compose "$@" )
}

cleanup() {
    local status=$?
    if [ "$KEEP" = "1" ]; then
        warn "--keep: leaving containers up. Tear down with:"
        warn "  cd ${HERE} && docker compose down -v"
    else
        info "tearing down containers…"
        compose down -v --remove-orphans >/dev/null 2>&1
    fi
    [ -n "$SSH_DIR" ] && rm -rf "$SSH_DIR"
    return $status
}
trap cleanup EXIT

# Wait until systemd inside the container has finished booting. `is-system-running`
# reports `degraded` in a container (masked units), which is fine — anything but
# `initializing`/`starting` means the boot transaction is done.
wait_for_boot() {
    local distro="$1" deadline=$((SECONDS + 90)) state=""
    while [ $SECONDS -lt $deadline ]; do
        state=$(rexec "$distro" systemctl is-system-running 2>/dev/null | tr -d '\r\n')
        case "$state" in
            running|degraded|maintenance) return 0 ;;
        esac
        sleep 2
    done
    warn "${distro}: systemd never finished booting (last state: ${state:-none})"
    return 1
}

wait_for_ssh() {
    local distro="$1" ip deadline=$((SECONDS + 60))
    ip=$(container_ip "$distro")
    [ -n "$ip" ] || { warn "${distro}: no container IP"; return 1; }
    while [ $SECONDS -lt $deadline ]; do
        if ssh "${SSH_OPTS[@]}" -i "$SSH_KEY" "${E2E_USER}@${ip}" true 2>/dev/null; then
            return 0
        fi
        sleep 2
    done
    warn "${distro}: ssh never came up at ${ip}"
    return 1
}

# Install the harness pubkey. Done as root over `docker exec` because it is
# setup, not part of the flow under test.
provision_ssh() {
    local distro="$1" pub
    pub=$(cat "${SSH_KEY}.pub")
    rexec "$distro" bash -c "
        install -d -m 700 -o ${E2E_USER} -g ${E2E_USER} /home/${E2E_USER}/.ssh &&
        printf '%s\n' '${pub}' > /home/${E2E_USER}/.ssh/authorized_keys &&
        chmod 600 /home/${E2E_USER}/.ssh/authorized_keys &&
        chown ${E2E_USER}:${E2E_USER} /home/${E2E_USER}/.ssh/authorized_keys
    " >/dev/null 2>&1
}

# ── The flow ─────────────────────────────────────────────────────────────────

run_distro() {
    local distro="$1"
    CURRENT_DISTRO="$distro"
    local artifact artifact_name ip uid

    banner "${distro}"

    artifact=$(artifact_for "$distro")
    if [ -z "$artifact" ] || [ ! -f "$artifact" ]; then
        skip "artifact available" "no artifact found for ${distro} under ${BUNDLE_DIR}"
        return
    fi
    artifact_name=$(basename "$artifact")
    info "artifact: ${artifact_name}"

    wait_for_boot "$distro" || { fail "systemd booted"; return; }
    pass "systemd booted"

    provision_ssh "$distro"
    wait_for_ssh "$distro" || { fail "ssh login as ${E2E_USER}"; return; }
    pass "ssh login as ${E2E_USER} (real logind session)"

    ip=$(container_ip "$distro")
    uid=$(rexec "$distro" id -u "$E2E_USER" | tr -d '\r\n')
    info "container ip: ${ip}   ${E2E_USER} uid: ${uid}"

    # The session must be a *real* one, or step 3 tests nothing.
    local userbus
    userbus=$(uexec "$distro" "systemctl --user show-environment | head -1; echo RC=\$?")
    assert_contains "systemctl --user works in the SSH session" "$userbus" "RC=0"

    # ── (a) install ──────────────────────────────────────────────────────
    banner "${distro} · step a — install.sh"
    local install_out
    install_out=$(uexec "$distro" \
        "CODEMUX_ARTIFACT=/artifacts/${artifact_name} NO_COLOR=1 sh /repo-scripts/install.sh")
    printf '%s\n' "$install_out" | sed 's/^/      /'
    assert_contains "install.sh reports success" "$install_out" "Codemux is installed."

    local caps version
    caps=$(uexec "$distro" "codemux capabilities")
    assert_contains "codemux capabilities runs after install" "$caps" '"version"'
    version=$(printf '%s' "$caps" | sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)
    local pkg_version
    pkg_version=$(sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "${REPO_ROOT}/src-tauri/tauri.conf.json" | head -n 1)
    assert_eq "installed version matches tauri.conf.json" "$version" "$pkg_version"

    # ── (b) login ────────────────────────────────────────────────────────
    banner "${distro} · step b — codemux login"
    local login_out
    login_out=$(uexec_pw "$distro" "codemux login --email ${E2E_EMAIL}; echo RC=\$?")
    printf '%s\n' "$login_out" | sed 's/^/      /'
    assert_contains "codemux login exits 0" "$login_out" "RC=0"
    assert_contains "codemux login confirms the account" "$login_out" "Signed in as ${E2E_EMAIL}"
    assert_contains "CODEMUX_PASSWORD warns" "$login_out" "warning: using the password from"

    local whoami_out
    whoami_out=$(uexec "$distro" "codemux whoami; echo RC=\$?")
    assert_contains "codemux whoami exits 0" "$whoami_out" "RC=0"
    assert_contains "codemux whoami names the account" "$whoami_out" "Signed in as ${E2E_EMAIL}"
    assert_contains "codemux whoami reports the user id" "$whoami_out" "$E2E_USER_ID"

    local signin_hits
    signin_hits=$(mock_requests | grep -c '"path": "/api/auth/desktop/signin"')
    if [ "${signin_hits:-0}" -ge 1 ]; then
        pass "mock API saw the signin POST" "${signin_hits} request(s)"
    else
        fail "mock API saw the signin POST" "none recorded"
    fi

    # ── (c) connect ──────────────────────────────────────────────────────
    banner "${distro} · step c — codemux connect"
    local connect_out
    connect_out=$(uexec "$distro" "codemux connect; echo RC=\$?")
    printf '%s\n' "$connect_out" | sed 's/^/      /'
    assert_contains "codemux connect exits 0" "$connect_out" "RC=0"
    assert_contains "connect reports remote access configured" "$connect_out" "Remote access configured (relay mode on"
    assert_contains "connect reports the background service" "$connect_out" "Background service ready"
    assert_not_contains "connect did not fall back to 'unsupported'" "$connect_out" "No background service was installed"

    local unit_path unit_body
    unit_path="/home/${E2E_USER}/.config/systemd/user/codemux.service"
    unit_body=$(rexec "$distro" cat "$unit_path" 2>&1)
    assert_contains "unit file written" "$unit_body" "ExecStart="
    assert_contains "unit runs \`serve\`" "$unit_body" " serve"
    assert_contains "unit carries CODEMUX_API_URL" "$unit_body" "Environment=CODEMUX_API_URL=${MOCK_API}"
    info "unit file:"; printf '%s\n' "$unit_body" | sed 's/^/      /'

    local is_active is_enabled linger
    is_active=$(uexec "$distro" "systemctl --user is-active codemux.service" | tr -d '\r\n')
    assert_eq "systemctl --user is-active codemux.service" "$is_active" "active"
    is_enabled=$(uexec "$distro" "systemctl --user is-enabled codemux.service" | tr -d '\r\n')
    assert_eq "systemctl --user is-enabled codemux.service" "$is_enabled" "enabled"
    linger=$(rexec "$distro" loginctl show-user "$E2E_USER" --property=Linger --value 2>/dev/null | tr -d '\r\n')
    assert_eq "linger enabled (survives logout)" "$linger" "yes"

    # The listener. Poll: `serve` boots the whole backend before it binds.
    local listening=""
    for _ in $(seq 1 30); do
        listening=$(rexec "$distro" ss -ltnp 2>/dev/null | grep ":${CODEMUX_PORT} ")
        [ -n "$listening" ] && break
        sleep 1
    done
    if [ -n "$listening" ]; then
        pass "port ${CODEMUX_PORT} listening" "$(printf '%s' "$listening" | awk '{print $4}' | head -1)"
        info "$(printf '%s' "$listening" | head -1)"
    else
        fail "port ${CODEMUX_PORT} listening" "nothing bound"
        info "journal:"; rexec "$distro" journalctl "_UID=${uid}" -n 40 --no-pager 2>&1 | sed 's/^/      /'
    fi

    local health_local
    health_local=$(uexec "$distro" "curl -fsS http://localhost:${CODEMUX_PORT}/api/health")
    assert_contains "GET /api/health on localhost" "$health_local" '"ok":true'
    info "health (local): ${health_local}"

    # The money shot: another container reaching this one. Only possible if
    # `serve` bound 0.0.0.0 rather than loopback.
    local health_cross
    health_cross=$(mock_curl "http://${ip}:${CODEMUX_PORT}/api/health")
    assert_contains "cross-container GET http://${ip}:${CODEMUX_PORT}/api/health" "$health_cross" '"ok":true'
    info "health (from mock-api container): ${health_cross}"

    # ── (4) the release web bundle is really embedded ────────────────────
    # The whole document, not a prefix: the inline splash <style> pushes the
    # hashed bundle references well past the first few hundred bytes.
    local root_html
    root_html=$(uexec "$distro" "curl -fsS http://localhost:${CODEMUX_PORT}/")
    assert_contains "GET / serves the embedded web UI (HTML)" "$root_html" "<!doctype html"
    assert_contains "embedded UI references a built asset bundle" "$root_html" "/assets/"

    # ── (d) pairing URL uses the container IP, not loopback ──────────────
    #
    # Asked the way a user would: `codemux remote pair` from a second SSH
    # session, which reaches the running unit over the control socket that
    # `codemux serve` now publishes.
    banner "${distro} · step d — pairing URL"

    local pair_out pair_url
    pair_out=$(uexec "$distro" "codemux remote pair")
    printf '%s\n' "$pair_out" | grep -E '^(Pairing link|Token|Endpoint|Expires):' | sed 's/^/      /'
    pair_url=$(printf '%s' "$pair_out" | sed -n 's/^Pairing link:[[:space:]]*//p' | head -1 | tr -d '\r')

    if [ -z "$pair_url" ]; then
        fail "codemux remote pair works against the running unit" "no 'Pairing link:' line"
        printf '%s\n' "$pair_out" | sed 's/^/      /' >&2
    else
        pass "codemux remote pair works against the running unit" "$pair_url"
        assert_contains "pairing URL host is the container IP (${ip})" "$pair_url" "//${ip}:${CODEMUX_PORT}"
        assert_not_contains "pairing URL is NOT loopback" "$pair_url" "127.0.0.1"
        assert_not_contains "pairing URL is NOT localhost" "$pair_url" "localhost"
        # The endpoint the CLI recommends should be the reachable one, and it
        # must say so rather than silently picking loopback.
        assert_contains "pair reports a non-loopback endpoint" "$pair_out" "Endpoint:      ${ip}"
    fi

    # ── (d2) a second `serve` refuses cleanly ────────────────────────────
    #
    # The mutual-exclusion guard depends on the control socket being visible
    # from another process. It must lose to `run_serve`'s own check, not to a
    # port collision deeper in the stack.
    local second_serve
    second_serve=$(uexec "$distro" "codemux serve; echo RC=\$?")
    printf '%s\n' "$second_serve" | sed 's/^/      /'
    assert_not_contains "second \`codemux serve\` exits non-zero" "$second_serve" "RC=0"
    assert_contains "second \`serve\` reports mutual exclusion" "$second_serve" "already running on this machine"
    assert_not_contains "second \`serve\` did NOT fail on a port collision" "$second_serve" "Address already in use"

    # ── (e) device registration reached the mock ─────────────────────────
    banner "${distro} · step e — device registration"
    local dev_line=""
    for _ in $(seq 1 30); do
        dev_line=$(mock_requests | grep '"path": "/api/devices"' | tail -1)
        [ -n "$dev_line" ] && break
        sleep 2
    done
    if [ -z "$dev_line" ]; then
        fail "POST /api/devices arrived at the mock API" "no request recorded within 60s"
        info "recorded requests:"; mock_requests | sed 's/^/      /'
        info "journal:"; rexec "$distro" journalctl "_UID=${uid}" -n 60 --no-pager 2>&1 | sed 's/^/      /'
    else
        pass "POST /api/devices arrived at the mock API"
        info "registration POST: ${dev_line}"
        assert_contains "registration carried the session bearer" "$dev_line" "\"bearer\": \"${E2E_TOKEN}\""
        assert_contains "registration body has nodeId" "$dev_line" '"nodeId"'
        assert_contains "registration body has deviceId" "$dev_line" '"deviceId"'
        assert_contains "registration reports platform linux" "$dev_line" '"platform": "linux"'
        # The name is the container hostname, i.e. a real local fact.
        assert_contains "registration names the host" "$dev_line" "codemux-e2e-${distro}"

        # The node id is the iroh identity a browser dials — it must be a real
        # 64-char hex key, not an empty string that happens to serialize.
        local node_id
        node_id=$(printf '%s' "$dev_line" | python3 -c 'import json,sys; print(json.load(sys.stdin)["body"]["nodeId"])' 2>/dev/null)
        if printf '%s' "$node_id" | grep -qE '^[0-9a-f]{64}$'; then
            pass "registration nodeId is a real iroh key" "$node_id"
        else
            fail "registration nodeId is a real iroh key" "got «${node_id}»"
        fi
    fi

    # The desktop's own view of registration, read back over the control
    # socket. `web_remote_enable` with no scope/port is the no-op read path —
    # its result carries the full live `WebRemoteStatus`, whose
    # `device_registered` / `iroh_node_id` / `device_id` are the registry facts
    # (see `WebRemoteStatus` in src-tauri/src/web_remote/mod.rs).
    local status_json reg_probe reg_registered reg_node reg_dev
    status_json=$(uexec "$distro" "codemux json web_remote_enable '{}'")
    reg_probe=$(printf '%s' "$status_json" | python3 -c '
import json,sys
try:
    s = json.load(sys.stdin)["data"]["status"]
    print("%s|%s|%s|%s" % (s["device_registered"], s["iroh_node_id"],
                           s["device_id"], s["relay_mode_enabled"]))
except Exception as e:
    print("PARSE_ERROR|%s||" % e)
' 2>/dev/null)
    IFS='|' read -r reg_registered reg_node reg_dev _ <<< "$reg_probe"
    info "registration status (registered|node|device): ${reg_probe}"
    assert_eq "desktop reports registration succeeded" "$reg_registered" "True"
    if printf '%s' "$reg_node" | grep -qE '^[0-9a-f]{64}$'; then
        pass "desktop reports the registered node id" "$reg_node"
    else
        fail "desktop reports the registered node id" "not a 64-hex key: «${reg_node}»"
    fi
    # The id the mock was told about and the id the desktop thinks it holds must
    # be the same one, or "registered" is describing somebody else's device.
    assert_contains "desktop's device id matches the one the mock recorded" \
        "$dev_line" "\"deviceId\": \"${reg_dev}\""
    assert_contains "desktop's node id matches the one the mock recorded" \
        "$dev_line" "\"nodeId\": \"${reg_node}\""

    # ── (e1b) the registration went to the mock, never to the real API ───
    #
    # Now that relay mode genuinely starts, the box really does talk to the
    # network: iroh sends discovery/holepunching UDP toward n0's public relays.
    # That is expected and non-blocking (registration succeeds above either
    # way). What must never happen is an *account API* call to the production
    # host, so this pins the live process down rather than trusting the unit
    # file alone: the running serve's own environment has to name the mock, and
    # `api.codemux.org` must not appear anywhere in that process's environment
    # or in the unit that started it.
    local serve_pid serve_env real_api_refs
    serve_pid=$(rexec "$distro" bash -c \
        "pgrep -u ${E2E_USER} -f 'codemux serve' | head -1" 2>/dev/null | tr -d '\r\n')
    if [ -n "$serve_pid" ]; then
        serve_env=$(rexec "$distro" bash -c \
            "tr '\\0' '\\n' < /proc/${serve_pid}/environ" 2>/dev/null)
        assert_contains "the running serve's own env points at the mock API" \
            "$serve_env" "CODEMUX_API_URL=${MOCK_API}"
        assert_not_contains "the running serve's env never names the production API" \
            "$serve_env" "api.codemux.org"
    else
        fail "found the running serve process" "no \`codemux serve\` pid for ${E2E_USER}"
    fi
    real_api_refs=$(rexec "$distro" bash -c \
        "grep -rl 'api\\.codemux\\.org' /home/${E2E_USER}/.config/systemd/user/ 2>/dev/null" \
        2>/dev/null | tr -d '\r')
    assert_eq "no unit file references the production API" "${real_api_refs:-none}" "none"

    # ── (e2) a serve restart does not disturb the persisted config ───────
    #
    # The regression that motivated this: `serve` used to persist an
    # un-restored default over the settings row on every boot, silently
    # turning relay mode back off. The row must come back byte-identical.
    banner "${distro} · step e2 — persisted config survives a serve restart"
    local row_before row_after
    row_before=$(settings_row "$distro")
    info "settings row before restart: ${row_before}"
    uexec "$distro" "systemctl --user restart codemux.service" >/dev/null
    wait_for_port "$distro" "$CODEMUX_PORT"
    sleep 3
    row_after=$(settings_row "$distro")
    info "settings row after  restart: ${row_after}"
    assert_eq "settings row is byte-stable across a serve restart" "$row_after" "$row_before"
    assert_contains "relay_mode_enabled stayed true" "$row_after" '"relay_mode_enabled":true'
    assert_contains "port was not reset" "$row_after" "\"port\":${CODEMUX_PORT}"
    assert_contains "bind_scope was not reset" "$row_after" '"bind_scope":"all"'

    # ── (e3) serve honours a persisted non-default port ─────────────────
    #
    # Config is applied on boot, not just when a flag is passed: change the
    # persisted port through the running instance, restart, and the unit must
    # come back on the new port with no `--port` anywhere.
    banner "${distro} · step e3 — serve binds the persisted port"
    local alt_port=4399
    uexec "$distro" "codemux remote enable --port ${alt_port}" >/dev/null 2>&1
    uexec "$distro" "systemctl --user restart codemux.service" >/dev/null
    wait_for_port "$distro" "$alt_port"
    local alt_listen
    alt_listen=$(rexec "$distro" ss -ltn 2>/dev/null | grep ":${alt_port} ")
    if [ -n "$alt_listen" ]; then
        pass "serve rebinds to the persisted port ${alt_port}" "$(printf '%s' "$alt_listen" | awk '{print $4}' | head -1)"
    else
        fail "serve rebinds to the persisted port ${alt_port}" "nothing bound on ${alt_port}"
        info "$(rexec "$distro" ss -ltn 2>/dev/null | grep -E ':(4377|4399) ')"
    fi
    assert_contains "persisted port is in the settings row" "$(settings_row "$distro")" "\"port\":${alt_port}"

    # Restore the default port for the remaining steps.
    uexec "$distro" "codemux remote enable --port ${CODEMUX_PORT}" >/dev/null 2>&1
    uexec "$distro" "systemctl --user restart codemux.service" >/dev/null
    wait_for_port "$distro" "$CODEMUX_PORT"
    assert_contains "port restored to ${CODEMUX_PORT}" "$(settings_row "$distro")" "\"port\":${CODEMUX_PORT}"

    # ── (f) status / off / idempotence ───────────────────────────────────
    banner "${distro} · step f — status, off, idempotence"
    local status_out
    status_out=$(uexec "$distro" "codemux connect status")
    printf '%s\n' "$status_out" | sed 's/^/      /'
    assert_contains "status names the account" "$status_out" "Signed in as ${E2E_EMAIL}"
    assert_contains "status reports enabled" "$status_out" "Enabled:      yes"
    # `codemux connect` persists relay_mode_enabled=true; the `serve` it starts
    # must hydrate that row rather than write a default back over it.
    assert_contains "status reports relay on" "$status_out" "Relay mode:   on"
    assert_contains "status reports the unit active" "$status_out" "codemux.service: installed, active, starts at boot"
    assert_contains "status reports linger" "$status_out" "Survives logout: yes"
    # The device id is minted by the first registration attempt, so its presence
    # is a second, independent witness that relay mode really started.
    assert_contains "status reports a device id" "$status_out" "Device id:"
    local status_device_id
    status_device_id=$(printf '%s' "$status_out" | sed -n 's/^  Device id:[[:space:]]*//p' | head -1 | tr -d '\r')
    if [ -n "$status_device_id" ]; then
        pass "status device id is non-empty" "$status_device_id"
    else
        fail "status device id is non-empty" "the 'Device id:' line carried no value"
    fi
    assert_contains "status names the device" "$status_out" "Device name:     codemux-e2e-${distro}"
    # `control_server_is_running()` must see the serve-managed instance — the
    # same fact the mutual-exclusion guard and `remote pair` key off.
    assert_contains "status sees the running instance" "$status_out" "Codemux running: yes"

    local off_out
    off_out=$(uexec "$distro" "codemux connect off; echo RC=\$?")
    printf '%s\n' "$off_out" | sed 's/^/      /'
    assert_contains "codemux connect off exits 0" "$off_out" "RC=0"
    assert_contains "off removed the service" "$off_out" "Background service stopped and removed"
    # Relay was genuinely on, so `off` has something real to turn off — and must
    # say so rather than "was already off".
    assert_contains "off turned relay off" "$off_out" "From-anywhere access is off"
    assert_not_contains "off did not report a no-op" "$off_out" "was already off"
    assert_contains "off leaves the session alone" "$off_out" "Still signed in as ${E2E_EMAIL}"
    assert_contains "relay is off in the settings row after \`connect off\`" \
        "$(settings_row "$distro")" '"relay_mode_enabled":false'

    local unit_gone
    unit_gone=$(rexec "$distro" test -e "$unit_path" && echo present || echo gone)
    assert_eq "unit file removed" "$unit_gone" "gone"
    # `is-active` answers `inactive` here — which *contains* "active", so this
    # has to be an exact comparison, not a substring one.
    local off_active
    off_active=$(uexec "$distro" "systemctl --user is-active codemux.service" | tr -d '\r\n')
    assert_ne "service no longer active" "$off_active" "active"

    local reconnect_out
    reconnect_out=$(uexec "$distro" "codemux connect; echo RC=\$?")
    assert_contains "re-running codemux connect works (idempotent)" "$reconnect_out" "RC=0"
    assert_contains "re-connect reinstalled the service" "$reconnect_out" "Background service ready"
    local re_active
    re_active=$(uexec "$distro" "systemctl --user is-active codemux.service" | tr -d '\r\n')
    assert_eq "service active again after re-connect" "$re_active" "active"

    local reinstall_out
    reinstall_out=$(uexec "$distro" \
        "CODEMUX_ARTIFACT=/artifacts/${artifact_name} NO_COLOR=1 sh /repo-scripts/install.sh")
    assert_contains "re-running install.sh is idempotent" "$reinstall_out" "Codemux is installed."
    assert_contains "re-install detected the existing install" "$reinstall_out" "found existing"

    # ── (g) reboot resilience ────────────────────────────────────────────
    banner "${distro} · step g — reboot resilience"
    info "restarting container…"
    docker restart "$(container_of "$distro")" >/dev/null
    wait_for_boot "$distro" || { fail "container rebooted"; return; }
    pass "container rebooted"

    # NOTE: no SSH login is performed before these assertions on purpose. If
    # the service only comes back because we logged in, linger is doing
    # nothing — the point is that it is up with nobody logged in at all.
    # `docker exec -u <user>` gets no PAM and creates no session; it just
    # borrows the already-running user manager's bus. If linger did not bring
    # that manager up, /run/user/<uid>/bus does not exist and this fails —
    # which is precisely the signal we want.
    local post_active=""
    for _ in $(seq 1 45); do
        post_active=$(docker exec -u "$E2E_USER" \
            -e XDG_RUNTIME_DIR="/run/user/${uid}" \
            "$(container_of "$distro")" \
            systemctl --user is-active codemux.service 2>/dev/null | tr -d '\r\n')
        [ "$post_active" = "active" ] && break
        sleep 2
    done
    assert_eq "unit auto-started after reboot (linger, no login)" "$post_active" "active"

    # Corroborate with logind: nobody is logged in over SSH. Counting *all*
    # sessions is too strict — Fedora's logind can carry a non-login session
    # object for the lingering user — so this counts only sessions logind
    # marks `Remote=yes`, which is exactly "somebody SSH'd in".
    local remote_sessions=0 sid
    for sid in $(rexec "$distro" loginctl list-sessions --no-legend 2>/dev/null | awk '{print $1}'); do
        if [ "$(rexec "$distro" loginctl show-session "$sid" -p Remote --value 2>/dev/null | tr -d '\r\n')" = "yes" ]; then
            remote_sessions=$((remote_sessions + 1))
        fi
    done
    assert_eq "no SSH login session exists after reboot" "$remote_sessions" "0"
    info "logind sessions after reboot: $(rexec "$distro" loginctl list-sessions --no-legend 2>/dev/null | tr '\n' ';')"
    info "user state: $(rexec "$distro" loginctl show-user "$E2E_USER" -p State -p Linger --value 2>/dev/null | tr '\n' ' ')"

    local post_listening=""
    for _ in $(seq 1 45); do
        post_listening=$(rexec "$distro" ss -ltnp 2>/dev/null | grep ":${CODEMUX_PORT} ")
        [ -n "$post_listening" ] && break
        sleep 2
    done
    if [ -n "$post_listening" ]; then
        pass "port ${CODEMUX_PORT} listening again after reboot"
        info "$(printf '%s' "$post_listening" | head -1)"
    else
        fail "port ${CODEMUX_PORT} listening again after reboot" "nothing bound"
        info "journal:"; rexec "$distro" journalctl "_UID=${uid}" -n 60 --no-pager 2>&1 | sed 's/^/      /'
    fi

    # And still reachable from off-box, which is the actual user-facing promise.
    ip=$(container_ip "$distro")
    local post_health
    post_health=$(mock_curl "http://${ip}:${CODEMUX_PORT}/api/health")
    assert_contains "still reachable cross-container after reboot" "$post_health" '"ok":true'
    info "health after reboot (from mock-api container): ${post_health}"
}

# ── Summary ──────────────────────────────────────────────────────────────────

print_summary() {
    banner "summary"
    local width=0 row step
    for row in "${RESULTS[@]}"; do
        step="${row#*|}"; step="${step%%|*}"
        [ ${#step} -gt $width ] && width=${#step}
    done
    local last_distro=""
    for row in "${RESULTS[@]}"; do
        local d s st detail
        IFS='|' read -r d s st detail <<< "$row"
        if [ "$d" != "$last_distro" ]; then
            printf '\n%s%s%s\n' "$C_BOLD" "$d" "$C_RESET"
            last_distro="$d"
        fi
        case "$st" in
            PASS)  printf '  %s✓%s %-*s %s%s%s\n' "$C_GREEN" "$C_RESET" "$width" "$s" "$C_DIM" "$detail" "$C_RESET" ;;
            FAIL)  printf '  %s✗%s %-*s %s%s%s\n' "$C_RED" "$C_RESET" "$width" "$s" "$C_RED" "$detail" "$C_RESET" ;;
            SKIP)  printf '  %s○%s %-*s %s%s%s\n' "$C_YELLOW" "$C_RESET" "$width" "$s" "$C_DIM" "$detail" "$C_RESET" ;;
        esac
    done

    local total=${#RESULTS[@]}
    printf '\n'
    if [ "$FAILURES" -eq 0 ]; then
        printf '%s✓ %d/%d assertions passed%s\n' "$C_GREEN$C_BOLD" "$total" "$total" "$C_RESET"
    else
        printf '%s✗ %d of %d assertions FAILED%s\n' "$C_RED$C_BOLD" "$FAILURES" "$total" "$C_RESET"
    fi
}

# ── Main ─────────────────────────────────────────────────────────────────────

# Stage the artifacts into one directory the compose bind-mount can point at.
ARTIFACT_DIR=$(mktemp -d "${TMPDIR:-/tmp}/codemux-e2e-artifacts.XXXXXX")
stage_artifacts() {
    local distro artifact staged=0
    for distro in $DISTROS; do
        artifact=$(artifact_for "$distro")
        if [ -n "$artifact" ] && [ -f "$artifact" ]; then
            cp -n "$artifact" "${ARTIFACT_DIR}/" 2>/dev/null || true
            staged=$((staged + 1))
        fi
    done
    [ "$staged" -gt 0 ] || fatal "no artifacts found under ${BUNDLE_DIR} — run \`npm run tauri -- build --bundles deb,rpm,appimage\` first (or pass --artifact)"
    # mktemp -d is 0700, and the uid that owns it does not exist (or is a
    # different person) inside the containers — the unprivileged test user must
    # be able to read the bind mount.
    chmod 0755 "$ARTIFACT_DIR"
    chmod 0644 "$ARTIFACT_DIR"/* 2>/dev/null || true
    info "staged artifacts:"; ls -la "$ARTIFACT_DIR" | tail -n +2 | sed 's/^/      /'
}

main() {
    banner "codemux one-command remote bootstrap — E2E"
    info "repo:     ${REPO_ROOT}"
    info "distros:  ${DISTROS}"

    setup_ssh_key
    stage_artifacts

    banner "bringing up the mock API + targets"
    if [ "$REBUILD_IMAGES" = "1" ]; then
        compose build --no-cache || fatal "docker compose build failed"
    else
        compose build || fatal "docker compose build failed"
    fi
    # `up` every service so the network + mock exist even for a single distro;
    # the unused target simply idles.
    compose up -d mock-api $DISTROS || fatal "docker compose up failed"

    # Mock API liveness before anything depends on it.
    local ok=""
    for _ in $(seq 1 30); do
        ok=$(mock_curl "http://mock-api:8787/_e2e/health")
        printf '%s' "$ok" | grep -q '"ok": true' && break
        sleep 1
    done
    CURRENT_DISTRO="harness"
    assert_contains "mock API is up" "$ok" '"ok": true'

    local distro
    for distro in $DISTROS; do
        run_distro "$distro"
    done

    print_summary
    [ "$FAILURES" -eq 0 ] || exit 1
}

main
