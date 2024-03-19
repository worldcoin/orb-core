#!/bin/bash
set -eu

GEN_MODE="lcov"

while getopts 'H' opt; do
    case "$opt" in
    H)
        GEN_MODE="html"
        ;;
    esac
done
shift "$(($OPTIND - 1))"

REPO_ROOT="$(dirname "$(dirname "$(readlink -fm "$0")")")"
echo "Repo root dir detected: ${REPO_ROOT}"

export RUSTFLAGS="-C instrument-coverage"
export COVERAGE_DIR="${REPO_ROOT}/target/coverage"
export LLVM_PROFILE_FILE="${COVERAGE_DIR}/report-%p-%m.profraw"

# Cleanup our mess in case we want to run coverage again
function finish {
    rm -rf ${COVERAGE_DIR}
}
trap finish EXIT

# Check if we have a latest version of grcov
GRCOV_BIN="${HOME}/.cargo/bin/grcov"
if [[ ! -e ${GRCOV_BIN} ]]; then
    GRCOV_BIN=$(which grcov)
fi
echo "grcov lives in: ${GRCOV_BIN}"
${GRCOV_BIN} --version

# Run all tests to gather coverage
(
    cd ${REPO_ROOT}
    cargo test --tests
)

# grcov requires us to find where the test binaries live
BINARY_PATH=$(
    cd ${REPO_ROOT}
    cargo test --tests --no-run --message-format=json 2>/dev/null | jq -r "select(.profile.test == true) | .filenames[]" | grep -v dSYM - | head -n1 | awk -F 'debug' '{print $1 FS}'
)
echo "grcov binary path detected: ${BINARY_PATH}"

${GRCOV_BIN} ${COVERAGE_DIR} --binary-path ${BINARY_PATH} -s ${REPO_ROOT} -t ${GEN_MODE} --llvm --branch --ignore-not-existing --ignore "/*" -o lcov.info
