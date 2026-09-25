#!/bin/sh
set -eu

bundle_dir=${1:-src-tauri/target/release/bundle}
inventory=$(mktemp)
payload=$(mktemp -d)
trap 'rm -f "$inventory"; rm -rf "$payload"' EXIT

single_bundle() {
    if [ "$#" -ne 1 ] || [ ! -f "$1" ]; then
        printf 'expected exactly one bundle matching %s\n' "$*" >&2
        exit 1
    fi
    printf '%s\n' "$1"
}

assert_inventory() {
    format=$1
    grep -Eq '(^|[[:space:]])(\./)?usr/bin/farm3d$' "$inventory" || {
        printf '%s inventory does not contain usr/bin/farm3d\n' "$format" >&2
        exit 1
    }
    grep -Eq 'printer-catalog\.json$' "$inventory" || {
        printf '%s inventory does not contain printer-catalog.json\n' "$format" >&2
        exit 1
    }
    forbidden='(^|/)(gen-catalog|fake-orca|node_modules|src-tauri|src|tests?|target|\.git|legacy|snapshots)(/|$)|(^|/)(Cargo\.(toml|lock)|package(-lock)?\.json|justfile|credentials\.json|settings\.json|printers\.json|farm3d\.sqlite3([.-].*)?|farm3d\.lock)$'
    if grep -Eqi "$forbidden" "$inventory"; then
        printf '%s inventory contains source, tests, build output, metadata, credentials, legacy data, or developer tooling\n' "$format" >&2
        grep -Ei "$forbidden" "$inventory" >&2
        exit 1
    fi
}

assert_payload() {
    format=$1
    if grep -R -a -E -q 'F0_FIXTURE_SENTINEL|F1_(SUBMITTED_)?SECRET|F1_DB_FAILURE_SENTINEL|F1_CLEANUP_SENTINEL' "$payload"; then
        printf '%s payload contains a test credential sentinel\n' "$format" >&2
        exit 1
    fi
    rm -rf "$payload"
    mkdir "$payload"
}

for tool in ar tar bsdtar 7z; do
    command -v "$tool" >/dev/null 2>&1 || {
        printf 'required package inspection tool is unavailable: %s\n' "$tool" >&2
        exit 1
    }
done

deb=$(single_bundle "$bundle_dir"/deb/*.deb)
ar p "$deb" data.tar.gz | tar -tzf - > "$inventory"
assert_inventory deb
ar p "$deb" data.tar.gz | tar -xzf - -C "$payload"
assert_payload deb

rpm=$(single_bundle "$bundle_dir"/rpm/*.rpm)
bsdtar -tf "$rpm" > "$inventory"
assert_inventory rpm
bsdtar -xf "$rpm" -C "$payload"
assert_payload rpm

appimage=$(single_bundle "$bundle_dir"/appimage/*.AppImage)
7z l -ba "$appimage" > "$inventory"
assert_inventory appimage
7z x -y -o"$payload" "$appimage" >/dev/null
assert_payload appimage
