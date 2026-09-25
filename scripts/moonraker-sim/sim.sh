#!/usr/bin/env bash
# A local Klipper + Moonraker stack for A0.1 (#9) rehearsal runs.
#
# Two containers share the host network and one runtime directory:
#   klipper   - simulavr (an emulated atmega644p running real Klipper MCU
#               firmware) plus klippy, talking to it over a pty. They share
#               a container because a pty does not cross container
#               boundaries without a privileged /dev mount.
#   moonraker - the real Moonraker, talking to klippy over a Unix socket
#
# Moonraker listens on 127.0.0.1:${MOONRAKER_SIM_PORT:-7125}. It is a real
# Moonraker and a real Klipper; only the MCU and its sensors are emulated.
# See docs/verification/a0-1-moonraker-live-validation.md for what that does
# and does not count as evidence for.
#
# Usage: scripts/moonraker-sim/sim.sh <command>
#   build                 build the simulavr image (once; takes a few minutes)
#   up [trusted|apikey] [full|no-bed|multi-tool]
#                         start the stack (default: trusted full). no-bed
#                         drops [heater_bed] to test a missing object;
#                         multi-tool adds [extruder1].
#   down                  stop and remove the stack
#   status                container state plus Moonraker's server.info
#   api-key               print Moonraker's API key (apikey mode)
#   stop|start|restart <klipper|moonraker>
#                         drive the disconnect/reconnect scenarios
#   logs <klipper|moonraker|simulavr>
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
engine="${CONTAINER_ENGINE:-podman}"
port="${MOONRAKER_SIM_PORT:-7125}"
runtime="${MOONRAKER_SIM_DIR:-${XDG_RUNTIME_DIR:-/tmp}/farm3d-moonraker-sim}"
# Pinned so a rehearsal is reproducible. Override to try other versions.
# The simulavr image carries its own klippy, built from the Klipper commit
# the pinned prind checkout names.
moonraker_image="${MOONRAKER_IMAGE:-docker.io/mkuf/moonraker:v0.11.0-1-g1cfb0c4}"
simulavr_image="${SIMULAVR_IMAGE:-localhost/farm3d-simulavr:latest}"
prind_commit="${PRIND_COMMIT:-80600f7d6f0d28d4d249728a054ac546a22743b9}"
prefix="farm3d-sim"

user_args=(--user "$(id -u):$(id -g)")
if [ "$engine" = "podman" ]; then
    user_args+=(--userns=keep-id)
fi

run() {
    local name="$1"
    shift
    # Each directory is mounted on its own: the images declare them as
    # VOLUMEs, which would otherwise shadow one parent bind mount.
    local mounts=()
    for dir in config run logs gcodes database; do
        mounts+=(-v "$runtime/$dir:/opt/printer_data/$dir")
    done
    "$engine" run -d --name "$prefix-$name" --network host "${user_args[@]}" \
        "${mounts[@]}" "$@" >/dev/null
    echo "started $prefix-$name"
}

start_klipper() {
    # shellcheck disable=SC2016 # expanded inside the container
    run klipper -w /opt --entrypoint /bin/bash "$simulavr_image" -c '
        klipper/scripts/avrsim.py -p printer_data/run/simulavr.tty klipper/out/klipper.elf \
            >printer_data/logs/simulavr.log 2>&1 &
        for _ in $(seq 1 100); do [ -L printer_data/run/simulavr.tty ] && break; sleep 0.1; done
        exec venv/bin/python klipper/klippy/klippy.py -I printer_data/run/klipper.tty \
            -a printer_data/run/klipper.sock printer_data/config/printer.cfg \
            -l printer_data/logs/klippy.log'
}

start_moonraker() {
    run moonraker "$moonraker_image" -d /opt/printer_data
}

wait_for_file() {
    for _ in $(seq 1 60); do
        [ -e "$1" ] && return 0
        sleep 0.5
    done
    echo "error: $1 did not appear" >&2
    return 1
}

server_info() {
    curl -fsS --max-time 5 "http://127.0.0.1:$port/server/info" || true
}

case "${1:-}" in
build)
    src="${MOONRAKER_SIM_BUILD_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/farm3d/prind}"
    if [ ! -d "$src/.git" ]; then
        git clone https://github.com/mkuf/prind.git "$src"
    fi
    git -C "$src" fetch --quiet origin "$prind_commit" || true
    git -C "$src" checkout --quiet "$prind_commit"
    # Host networking: rootless podman builds cannot always create a tap device.
    "$engine" build --network=host --target build-simulavr \
        -t "$simulavr_image" "$src/docker/klipper"
    ;;
up)
    mode="${2:-trusted}"
    case "$mode" in
    trusted) trusted="0.0.0.0/0" ;;
    # A documentation-only range (RFC 5737) that no client comes from.
    apikey) trusted="192.0.2.0/24" ;;
    *)
        echo "error: mode must be trusted or apikey" >&2
        exit 2
        ;;
    esac
    mkdir -p "$runtime"/{config,run,logs,gcodes,database}
    case "${3:-full}" in
    full) cp "$here/printer.cfg" "$runtime/config/printer.cfg" ;;
    # A printer without a heated bed: heater_bed is absent from Moonraker's
    # answers, which is the "missing object" case A0.1 must check.
    no-bed)
        awk '/^\[heater_bed\]/ { skip = 1; next } /^\[/ { skip = 0 } !skip' \
            "$here/printer.cfg" >"$runtime/config/printer.cfg"
        ;;
    # A second toolhead, [extruder1], for the multi-tool case (decision B2).
    multi-tool)
        cat "$here/printer.cfg" "$here/extruder1.cfg" >"$runtime/config/printer.cfg"
        ;;
    *)
        echo "error: variant must be full, no-bed, or multi-tool" >&2
        exit 2
        ;;
    esac
    sed -e "s|@PORT@|$port|" -e "s|@TRUSTED@|$trusted|" \
        "$here/moonraker.conf.in" >"$runtime/config/moonraker.conf"
    start_klipper
    wait_for_file "$runtime/run/klipper.sock"
    start_moonraker
    for _ in $(seq 1 60); do
        info="$(server_info)"
        if [ -n "$info" ]; then
            echo "moonraker is up on 127.0.0.1:$port ($mode mode)"
            echo "$info"
            exit 0
        fi
        sleep 1
    done
    if [ "$mode" = "apikey" ]; then
        echo "moonraker started in apikey mode; /server/info needs the key, so it cannot be polled here"
        exit 0
    fi
    echo "error: moonraker did not answer on port $port; see: $0 logs moonraker" >&2
    exit 1
    ;;
down)
    for name in moonraker klipper; do
        "$engine" rm -f "$prefix-$name" >/dev/null 2>&1 || true
    done
    rm -rf "$runtime"
    echo "stack removed"
    ;;
status)
    "$engine" ps -a --filter "name=$prefix-" --format '{{.Names}}\t{{.Status}}'
    server_info
    echo
    ;;
api-key)
    python3 - "$runtime/database/moonraker-sql.db" <<'PY'
import sqlite3, sys
row = sqlite3.connect(sys.argv[1]).execute(
    "SELECT password FROM authorized_users WHERE username='_API_KEY_USER_'"
).fetchone()
print(row[0] if row else "")
PY
    ;;
stop | start | restart)
    service="${2:-}"
    case "$service" in
    klipper | moonraker) ;;
    *)
        echo "error: name klipper or moonraker" >&2
        exit 2
        ;;
    esac
    if [ "$1" = "start" ]; then
        "$engine" rm -f "$prefix-$service" >/dev/null 2>&1 || true
        "start_$service"
    elif [ "$1" = "stop" ]; then
        "$engine" rm -f "$prefix-$service" >/dev/null
        echo "stopped $prefix-$service"
    else
        "$engine" rm -f "$prefix-$service" >/dev/null
        "start_$service"
    fi
    ;;
logs)
    case "${2:-}" in
    klipper) cat "$runtime/logs/klippy.log" ;;
    moonraker) cat "$runtime/logs/moonraker.log" ;;
    simulavr) cat "$runtime/logs/simulavr.log" ;;
    *)
        echo "error: name klipper, moonraker, or simulavr" >&2
        exit 2
        ;;
    esac
    ;;
*)
    sed -n '2,28p' "$0"
    exit 2
    ;;
esac
