#!/bin/sh
# Type-check (and lint) the backend for Windows from Linux, without a mingw
# toolchain. `cargo check` never links, so the C that build scripts compile
# (bundled SQLite) only needs to exist: a stub compiler writes empty object
# files, and llvm-ar/llvm-windres stand in for the mingw archiver and
# resource compiler. The stub is not named `cc`: the host linker is `cc`,
# and host build scripts must still link for real. Nothing this produces
# can run.
set -eu

target=x86_64-pc-windows-gnu
for tool in llvm-ar llvm-windres; do
    command -v "$tool" >/dev/null || {
        printf 'check-windows needs %s (LLVM) on PATH\n' "$tool" >&2
        exit 1
    }
done
rustup target list --installed | grep -qx "$target" || {
    printf 'check-windows needs the Rust target: rustup target add %s\n' "$target" >&2
    exit 1
}

stubs=$(mktemp -d)
trap 'rm -rf "$stubs"' EXIT
cat >"$stubs/stub-cc" <<'STUB'
#!/bin/sh
prev=""
for a in "$@"; do
    case "$a" in
        -Fo*) : >"${a#-Fo}" ;;
        -o) ;;
        -o*) : >"${a#-o}" ;;
    esac
    [ "$prev" = "-o" ] && : >"$a"
    prev="$a"
done
exit 0
STUB
chmod +x "$stubs/stub-cc"
ln -s "$(command -v llvm-windres)" "$stubs/x86_64-w64-mingw32-windres"
ln -s "$(command -v llvm-ar)" "$stubs/x86_64-w64-mingw32-ar"

PATH="$stubs:$PATH" \
CC_x86_64_pc_windows_gnu="$stubs/stub-cc" \
AR_x86_64_pc_windows_gnu="$(command -v llvm-ar)" \
    cargo "${1:-check}" --manifest-path src-tauri/Cargo.toml --target "$target" \
    --all-targets --features test-support
