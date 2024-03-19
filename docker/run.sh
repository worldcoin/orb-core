#!/bin/sh

cd "$(dirname $0)/.."
set -x

docker run \
       --volume $PWD:/home/worldcoin/orb-core \
       --volume ${CARGO_HOME:-$HOME/.cargo}/registry:/home/worldcoin/.cargo/registry \
       --volume ${CARGO_HOME:-$HOME/.cargo}/git:/home/worldcoin/.cargo/git \
       --rm \
       --tty \
       --interactive \
       --env CARGO_TERM_COLOR=always \
       orb-core \
       "$@"
