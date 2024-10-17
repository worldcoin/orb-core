#!/bin/bash
set -ex

BIN=/home/worldcoin/orb-core
sudo setcap cap_net_raw+ep "${BIN}"
sudo systemctl stop worldcoin-core || true
sudo systemctl stop worldcoin-control-api || true
pkill orb-core || true
pkill nvargus-daemon || true
source /home/worldcoin/venv/bin/activate

export DBUS_SESSION_BUS_ADDRESS=unix:path=/tmp/worldcoin_bus_socket
export ORB_ID=$(/usr/local/bin/orb-id)
export RUST_BACKTRACE=full
export CURRENT_BOOT_SLOT=$(sudo /usr/bin/get-slot)

cd /home/worldcoin
nice -n -3 ${BIN}
