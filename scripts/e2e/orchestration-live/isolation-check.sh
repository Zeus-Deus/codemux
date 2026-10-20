#!/usr/bin/env bash
# Runs INSIDE the sandbox (up.sh calls it through in_sandbox): proves the host
# paths a live run must never reach are absent or read-only. One PASS/FAIL
# line per check; exit status is the verdict. Arguments come from the host,
# which knows the real paths:
#
#   ORCH_LIVE_WRITABLE="<mount>:<mount>…" \
#   isolation-check.sh <uid> <probe-name> <claude-auth-mount> <path-that-must-be-read-only>…
#
# ORCH_LIVE_WRITABLE lists the mount points that may be writable; an entry
# ending in "/" also allows every mount below it.
#
# up.sh then checks from the host side that the probe files written to the
# sandbox's /tmp and /var/tmp did not land in the host's.
set -u
uid="$1" probe="$2" auth="$3"; shift 3
IFS=: read -r -a WRITABLE <<<"${ORCH_LIVE_WRITABLE:-}"
rc=0
ok() { echo "PASS: $*"; }
bad() { echo "FAIL: $*"; rc=1; }

# Desktop services and the user's real Codemux instances.
for path in "/run/user/$uid/codemux.sock" "/run/user/$uid/codemux-dev.sock" "/run/user/$uid/bus" \
  /run/dbus/system_bus_socket "/run/user/$uid/keyring" "/run/user/$uid/gnupg" "/run/user/$uid" \
  /tmp/.X11-unix; do
  if [ -e "$path" ]; then bad "$path is reachable"; else ok "$path is not reachable"; fi
done
for var in DISPLAY WAYLAND_DISPLAY DBUS_SESSION_BUS_ADDRESS CODEMUX_SOCKET CLAUDECODE; do
  if [ -n "${!var:-}" ]; then bad "\$$var is set"; else ok "\$$var is unset"; fi
done
case "${XDG_RUNTIME_DIR:-}" in
  "/run/user/$uid"|"") bad "XDG_RUNTIME_DIR is '${XDG_RUNTIME_DIR:-}'" ;;
  *) ok "XDG_RUNTIME_DIR is private ($XDG_RUNTIME_DIR)" ;;
esac

# Private scratch dirs: writable here (the probes are checked host-side).
for dir in /tmp /var/tmp /dev/shm; do
  if echo probe >"$dir/$probe" 2>/dev/null; then ok "$dir is writable (private tmpfs)"; else bad "$dir is not writable"; fi
done

# Every writable mount is one the harness made on purpose; the host's own
# mounts (/, /home, /data, …) are all read-only. Field 5 of mountinfo is the
# mount point, field 6 its per-mount options.
while read -r _ _ _ _ point opts _; do
  case ",$opts," in *,rw,*) ;; *) continue ;; esac
  point="$(printf '%b' "${point//\\/\\0}")" # undo mountinfo's octal escapes
  allowed=0
  for want in "${WRITABLE[@]}"; do
    case "$want" in
      */) case "$point/" in "$want"*) allowed=1; break ;; esac ;;
      *) [ "$point" = "$want" ] && { allowed=1; break; } ;;
    esac
  done
  if [ "$allowed" = 1 ]; then ok "writable mount $point is expected"; else bad "unexpected writable mount: $point"; fi
done </proc/self/mountinfo

# Named host paths: absent, or present but read-only. Never truncates: a
# directory gets a probe file, anything else an append-mode open.
for path in "$@"; do
  if [ ! -e "$path" ]; then
    ok "$path is not visible"
  elif [ -d "$path" ]; then
    if ( : >"$path/$probe" ) 2>/dev/null; then rm -f "$path/$probe"; bad "$path is writable"; else ok "$path is read-only"; fi
  elif ( : >>"$path" ) 2>/dev/null; then
    bad "$path is writable"
  else
    ok "$path is read-only"
  fi
done
# The real HOME is not visible (a private one is mounted over it).
for path in "$HOME/.ssh" "$HOME/.gnupg" "$HOME/.config/codemux" "$HOME/.local/share/codemux"; do
  if [ -e "$path" ]; then bad "real $path is visible"; else ok "real $path is not visible"; fi
done

# Claude: the sandbox's ~/.claude is private; the real one is only at $auth,
# where everything but the credential entries is covered read-only.
if [ -e "$HOME/.claude/.credentials.json" ]; then bad "the private ~/.claude holds a .credentials.json"; else ok "the private ~/.claude holds no credentials"; fi
if [ -d "$auth" ]; then
  for name in history.jsonl settings.json .claude.json projects sessions; do
    [ -e "$auth/$name" ] || continue
    if [ -d "$auth/$name" ]; then
      if mkdir "$auth/$name/$probe" 2>/dev/null; then rmdir "$auth/$name/$probe"; bad "real ~/.claude/$name/ is writable"; else ok "real ~/.claude/$name/ is read-only"; fi
      if [ -n "$(ls -A "$auth/$name" 2>/dev/null)" ]; then bad "real ~/.claude/$name/ contents are visible"; fi
    elif ( : >>"$auth/$name" ) 2>/dev/null; then
      bad "real ~/.claude/$name is writable"
    else
      ok "real ~/.claude/$name is read-only"
    fi
  done
fi


# DNS still resolves (resolv.conf may point into the private /run).
if getent hosts api.anthropic.com >/dev/null 2>&1; then ok "DNS resolves"; else bad "DNS does not resolve"; fi
exit "$rc"
