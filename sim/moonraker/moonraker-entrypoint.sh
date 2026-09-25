#!/usr/bin/env bash
# Runs the real Moonraker with a config rendered from this directory's
# tracked template, so `sim/simctl variant moonraker <mode>` can switch
# between a trusted (loopback) and an API-key-enforcing Moonraker without
# ever touching a tracked file: it writes a mode marker into the writable
# `run` volume this container shares with klipper, and this entrypoint
# re-renders the config from that marker every time it (re)starts.
#
# FARM3D_SIM_MOONRAKER_CONF picks the base config (a file in this
# directory), so one image serves both the single-extruder and
# four-toolhead Moonraker.
set -euo pipefail

data=/opt/printer_data
base="/farm3d-sim/${FARM3D_SIM_MOONRAKER_CONF:-moonraker.conf}"
rendered="$data/run/moonraker.rendered.conf"

mode="$(cat "$data/run/variant" 2>/dev/null || true)"
case "$mode" in
# A documentation-only range (RFC 5737) that no client is ever really
# from, so every request needs the API key.
apikey) trusted="192.0.2.0/24" ;;
*) trusted="127.0.0.0/8" ;;
esac

sed "s|@TRUSTED@|$trusted|" "$base" >"$rendered"

exec /opt/venv/bin/python moonraker/moonraker/moonraker.py -d "$data" -c "$rendered"
