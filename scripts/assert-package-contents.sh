#!/bin/sh
set -eu

bundle_dir=${1:-src-tauri/target/release/bundle}
inventory=$(mktemp)
trap 'rm -f "$inventory"' EXIT

single_bundle() {
    if [ "$#" -ne 1 ] || [ ! -f "$1" ]; then
        printf 'expected exactly one bundle matching %s\n' "$*" >&2
        exit 1
    fi
    printf '%s\n' "$1"
}

assert_inventory() {
    format=$1
    grep -q 'usr/bin/farm3d$' "$inventory" || {
        printf '%s inventory does not contain usr/bin/farm3d\n' "$format" >&2
        exit 1
    }
    if grep -q 'usr/bin/gen-catalog$' "$inventory"; then
        printf '%s inventory contains dev-only usr/bin/gen-catalog\n' "$format" >&2
        exit 1
    fi
}

deb=$(single_bundle "$bundle_dir"/deb/*.deb)
ar p "$deb" data.tar.gz | tar -tzf - > "$inventory"
assert_inventory deb

rpm=$(single_bundle "$bundle_dir"/rpm/*.rpm)
bsdtar -tf "$rpm" > "$inventory"
assert_inventory rpm

appimage=$(single_bundle "$bundle_dir"/appimage/*.AppImage)
7z l -ba "$appimage" > "$inventory"
assert_inventory appimage
