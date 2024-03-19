#!/bin/bash

set -eux

PROFILE="dev-release"
PROFILE_DIR="target/${CARGO_BUILD_TARGET}/${PROFILE}"

function for_each_bin() {
    fd . "${PROFILE_DIR}" --exact-depth 1 --type executable --exec "${@}"
}

cd "$(dirname "${0}")/.."

cargo build --profile "${PROFILE}" "$@"

for_each_bin patchelf --set-interpreter /lib/ld-linux-aarch64.so.1 '{}'
for_each_bin echo '* Dev-Release binary is available: {}'
