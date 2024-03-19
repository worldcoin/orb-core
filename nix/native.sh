#!/bin/sh

cd "$(dirname "$0")/.." || exit
set -x

nix develop '.#native' \
  --no-warn-dirty \
  --command "$@"
