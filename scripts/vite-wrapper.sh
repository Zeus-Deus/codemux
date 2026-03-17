#!/bin/bash
RESTART_DELAY=2
MAX_QUICK_CRASHES=5
QUICK_CRASH_WINDOW=10

# Durable log under project .codemux (Vite and watchers ignore it)
LOG_DIR=".codemux"
LOG="${LOG_DIR}/vite-wrapper.log"
mkdir -p "$LOG_DIR"

# Append a line to the log and optionally echo to stderr
log_msg() {
    echo "$1" >> "$LOG"
}
log_both() {
    echo "$1" | tee -a "$LOG" >&2
}

pgid_of() {
    ps -o pgid= -p "$1" 2>/dev/null | tr -d '[:space:]'
}

pid_alive() {
    [ -n "$1" ] && kill -0 "$1" 2>/dev/null
}

wrapper_pid=$$
wrapper_ppid=$PPID
wrapper_pgid=$(pgid_of "$wrapper_pid")
start_ts=$(date +%s)
restart_count=0
crash_count=0
last_crash_time=0
current_vite_pid=""
current_vite_pgid=""
wrapper_signal=""
wrapper_exiting=0

log_wrapper_exit() {
    local exit_code="$1"
    local reason="$2"
    if [ "$wrapper_exiting" -eq 1 ]; then
        return
    fi
    wrapper_exiting=1
    log_msg "[$(date -Iseconds)] wrapper_exit reason=${reason} exit_code=${exit_code} signal=${wrapper_signal:-none} wrapper_pid=${wrapper_pid} wrapper_ppid=${wrapper_ppid} wrapper_pgid=${wrapper_pgid} vite_pid=${current_vite_pid:-none} vite_pgid=${current_vite_pgid:-none} restart_count=${restart_count}"
}

forward_signal_to_vite() {
    local sig="$1"
    local forwarded="false"
    if pid_alive "$current_vite_pid"; then
        if kill "-$sig" "$current_vite_pid" 2>/dev/null; then
            forwarded="true"
        fi
    fi
    log_msg "[$(date -Iseconds)] wrapper_signal signal=${sig} forwarded=${forwarded} wrapper_pid=${wrapper_pid} wrapper_ppid=${wrapper_ppid} wrapper_pgid=${wrapper_pgid} vite_pid=${current_vite_pid:-none} vite_pgid=${current_vite_pgid:-none}"
}

handle_signal() {
    local sig="$1"
    local exit_code="$2"
    wrapper_signal="$sig"
    forward_signal_to_vite "$sig"
    log_wrapper_exit "$exit_code" "signal_${sig}"
    exit "$exit_code"
}

trap 'handle_signal TERM 143' TERM
trap 'handle_signal INT 130' INT
trap 'handle_signal HUP 129' HUP
trap 'handle_signal QUIT 131' QUIT
trap 'log_wrapper_exit "$?" "exit_trap"' EXIT

log_msg "[$(date -Iseconds)] wrapper_pid=$wrapper_pid wrapper_ppid=$wrapper_ppid wrapper_pgid=${wrapper_pgid:-unknown} cwd=$(pwd) TAURI_DEV_HOST=${TAURI_DEV_HOST:-<unset>} start_ts=$start_ts log_start"

while true; do
    restart_count=$((restart_count + 1))
    launch_ts=$(date +%s)
    log_msg "[$(date -Iseconds)] restart_count=$restart_count launch_ts=$launch_ts Starting Vite dev server..."
    log_both "[vite-wrapper] Starting Vite dev server... (restart $restart_count, log: $LOG)"

    vite dev >> "$LOG" 2>&1 &
    current_vite_pid=$!
    current_vite_pgid=$(pgid_of "$current_vite_pid")
    log_msg "[$(date -Iseconds)] vite_spawn restart_count=$restart_count vite_pid=$current_vite_pid vite_pgid=${current_vite_pgid:-unknown}"

    wait "$current_vite_pid"
    EXIT_CODE=$?
    end_ts=$(date +%s)
    uptime=$((end_ts - launch_ts))
    now=$end_ts

    if [ $EXIT_CODE -gt 128 ]; then
        SIG=$((EXIT_CODE - 128))
        log_msg "[$(date -Iseconds)] vite_exit exit_code=$EXIT_CODE signal=$SIG uptime_sec=$uptime"
        log_both "[vite-wrapper] Vite killed by signal $SIG (exit code $EXIT_CODE)"
    else
        log_msg "[$(date -Iseconds)] vite_exit exit_code=$EXIT_CODE uptime_sec=$uptime"
        log_both "[vite-wrapper] Vite exited with code $EXIT_CODE"
    fi

    # User or Tauri requested shutdown (SIGINT=130, SIGTERM=143) - do not restart
    if [ $EXIT_CODE -eq 130 ] || [ $EXIT_CODE -eq 143 ]; then
        log_wrapper_exit "$EXIT_CODE" "vite_exit_signal"
        exit $EXIT_CODE
    fi

    # Track rapid crash loops - if crashing too fast too many times, give up
    if [ $((now - last_crash_time)) -lt $QUICK_CRASH_WINDOW ]; then
        crash_count=$((crash_count + 1))
    else
        crash_count=1
    fi
    last_crash_time=$now

    if [ $crash_count -ge $MAX_QUICK_CRASHES ]; then
        log_msg "[$(date -Iseconds)] wrapper_give_up=true rapid_crash_count=$crash_count window_seconds=$QUICK_CRASH_WINDOW last_exit_code=$EXIT_CODE last_signal=$([ $EXIT_CODE -gt 128 ] && echo $((EXIT_CODE - 128)) || echo 0)"
        log_both "[vite-wrapper] Vite crashed $MAX_QUICK_CRASHES times in ${QUICK_CRASH_WINDOW}s, giving up."
        log_wrapper_exit 1 "rapid_crash_giveup"
        exit 1
    fi

    log_msg "[$(date -Iseconds)] restarting_in=${RESTART_DELAY}s crash=$crash_count of $MAX_QUICK_CRASHES"
    log_both "[vite-wrapper] Restarting in ${RESTART_DELAY}s (crash $crash_count of $MAX_QUICK_CRASHES)..."
    current_vite_pid=""
    current_vite_pgid=""
    sleep $RESTART_DELAY
done
