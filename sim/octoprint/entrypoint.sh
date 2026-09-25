#!/usr/bin/env bash
# Starts OctoPrint for the simulator stack without the image's s6 init.
#
# The image's own init runs haproxy on :80 and OctoPrint on a hard-coded
# :5000. Under host networking both would collide with other local
# services, so this runs `octoprint serve` directly on the simulator's port.
#
# On an empty data volume it seeds the config and one admin user whose API
# key is fixed (FARM3D_SIM_OCTOPRINT_API_KEY). That key is a published test
# fixture, not a secret: the server only listens on 127.0.0.1.
set -euo pipefail

basedir=/octoprint/octoprint
port="${FARM3D_SIM_OCTOPRINT_UPSTREAM_PORT:?}"
api_key="${FARM3D_SIM_OCTOPRINT_API_KEY:?}"

# The image ships a placeholder config.yaml, so a marker file (not the
# config's presence) says whether this volume was seeded.
if [ ! -f "$basedir/.farm3d-seeded" ]; then
    mkdir -p "$basedir"
    cp /farm3d-sim/config.yaml "$basedir/config.yaml"
    octoprint --basedir "$basedir" user add farm3d \
        --password farm3d-sim --admin >/dev/null
    # `octoprint user` has no API-key subcommand, so the key goes straight
    # into users.yaml, which is where OctoPrint keeps per-user keys.
    python - "$basedir/users.yaml" "$api_key" <<'PY'
import sys, yaml
path, key = sys.argv[1], sys.argv[2]
with open(path) as f:
    users = yaml.safe_load(f)
users["farm3d"]["apikey"] = key
with open(path, "w") as f:
    yaml.safe_dump(users, f)
PY
    touch "$basedir/.farm3d-seeded"
    echo "farm3d-sim: seeded $basedir"
fi

exec octoprint serve --iknowwhatimdoing --host 127.0.0.1 --port "$port" \
    --basedir "$basedir"
