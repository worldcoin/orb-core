#!/bin/sh

cd "$(dirname $0)/.."
set -x

docker build \
       --build-arg CACHIX_AUTH_TOKEN \
       --build-arg GIT_HUB_TOKEN \
       --build-arg USER_ID=$(id -u) \
       --build-arg GROUP_ID=$(id -g) \
       --file docker/Dockerfile \
       --tag orb-core \
       .
