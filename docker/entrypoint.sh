#!/bin/sh
set -eu

mkdir -p /graph/uploads
if [ ! -f /graph/Config.toml ]; then
    cp /opt/graph/Config.toml /graph/Config.toml
fi

exec /usr/local/bin/graph "$@"
