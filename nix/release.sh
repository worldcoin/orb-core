#!/usr/bin/env bash

set -eux

TARGET_DIR="${CARGO_TARGET_DIR:-target}"
PROFILE="release"
PROFILE_DIR="${TARGET_DIR}/${CARGO_BUILD_TARGET}/${PROFILE}"

function for_each_bin() {
    fd . "${PROFILE_DIR}" --exact-depth 1 --type executable --exec "${@}"
}

cd "$(dirname "${0}")/.."

cargo build --release "$@"

for_each_bin patchelf --set-interpreter /lib/ld-linux-aarch64.so.1 '{}'
for_each_bin echo '* Release binary is available: {}'
