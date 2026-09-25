#!/usr/bin/env bash
# Runs simulavr (an emulated atmega644p running real Klipper MCU firmware)
# and klippy in one container. They share a container because the pty
# between them does not cross container boundaries without a privileged
# /dev mount. Moonraker reaches klippy through the Unix socket on the shared
# `run` volume.
#
# Starts as root only to hand the fresh named volumes to the image's
# unprivileged user (uid 1000, the same uid Moonraker's image uses), then
# drops to that user.
set -euo pipefail

data=/opt/printer_data

if [ "$(id -u)" = 0 ]; then
    mkdir -p "$data/run" "$data/logs" "$data/gcodes"
    chown 1000:1000 "$data/run" "$data/logs" "$data/gcodes"
    exec setpriv --reuid=1000 --regid=1000 --clear-groups "$0" "$@"
fi

# A restart reuses the volume; stale endpoints would make the waits below
# pass too early.
rm -f "$data/run/simulavr.tty" "$data/run/klipper.sock" "$data/run/klipper.tty"

/opt/klipper/scripts/avrsim.py -p "$data/run/simulavr.tty" /opt/klipper/out/klipper.elf \
    >"$data/logs/simulavr.log" 2>&1 &

for _ in $(seq 1 100); do
    [ -e "$data/run/simulavr.tty" ] && break
    sleep 0.1
done

# FARM3D_SIM_PRINTER_CFG picks the simulated printer (a file in this
# directory), so one image serves every Moonraker variant.
exec /opt/venv/bin/python /opt/klipper/klippy/klippy.py \
    -I "$data/run/klipper.tty" -a "$data/run/klipper.sock" \
    "/farm3d-sim/${FARM3D_SIM_PRINTER_CFG:-printer.cfg}"
