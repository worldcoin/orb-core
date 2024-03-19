#!/bin/sh

cd "$(dirname "$0")/.." || exit
set -x

nix develop '.#cross' \
  --no-warn-dirty \
  --command "$@"
