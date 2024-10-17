#!/bin/sh

cd "$(dirname $0)/.."
set -e

ALL="deny fmt clippy test build build_livestream_client doc check_debug_report_version"
if [ "$#" -eq 0 ]; then
  eval set -- $ALL
fi

while [ "$#" -gt 0 ]; do
  CMD=$1
  shift
  case "$CMD" in
  --server)
    SERVER=1
    export TERM=dumb
    ;;
  deny)
    echo "+++ Checking licenses and advisories with $(tput bold)cargo deny$(tput sgr0)"
    (
      set -x
      cargo deny check
    )
    ;;
  fmt)
    echo "+++ Checking code formatting with $(tput bold)rustfmt$(tput sgr0)"
    (
      set -x
      cargo fmt --all --check
    )
    ;;
  clippy)
    echo "+++ Running $(tput bold)clippy$(tput sgr0) lints"
    if [ "$SERVER" = "1" ]; then
      (
        set -x
        nix build --print-build-logs --no-link '.#clippy'
      )
    else
      (
        set -x
        cargo clippy --workspace --tests -- --deny warnings
      )
    fi
    ;;
  check_debug_report_version)
    echo "+++ Checking Signup Data version"
    if [ "$SERVER" = "1" ]; then
      (
        set -x
        nix build --print-build-logs '.#check_debug_report_version'
      )
    else
      (
        set -x
        cargo run --bin debug-report-schema check-version
      )
    fi
    ;;
  test)
    echo "+++ Running $(tput bold)cargo$(tput sgr0) tests"
    if [ "$SERVER" = "1" ]; then
      (
        set -x
        nix build --print-build-logs --no-link '.#test'
      )
    else
      (
        set -x
        nix/native.sh cargo test --workspace -- --include-ignored
      )
    fi
    ;;
  build)
    echo "+++ Building final $(tput bold)binaries$(tput sgr0)"
    (
      set -x
      nix build --print-build-logs '.#build'
    )
    ;;
  build_livestream_client)
    echo "+++ Building final $(tput bold)livestream-client binaries$(tput sgr0)"
    (
      set -x
      nix build --print-build-logs '.#build_livestream_client'
    )
    ;;
  doc)
    echo "+++ Building $(tput bold)rustdoc$(tput sgr0) documentation"
    if [ "$SERVER" = "1" ]; then
      (
        set -x
        nix build --print-build-logs --no-link '.#doc'
      )
    else
      if [ "$#" -gt 0 ] && [ "$1" = "--open" ]; then
        CARGO_DOC_ARGS=$1
        shift
      fi
      (
        set -x
        cargo doc --workspace --document-private-items --no-deps $CARGO_DOC_ARGS
      )
    fi
    ;;
  *)
    echo "$(tput bold)Unexpected command: $CMD$(tput sgr0)" >&2
    echo "Available commands: $ALL" >&2
    exit 1
    ;;
  esac
done
