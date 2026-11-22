#!/bin/sh
set -eu

PUID=${PUID:-1000}
PGID=${PGID:-1000}

mkdir -p /graph/uploads
if [ ! -f /graph/Config.toml ]; then
    cp /opt/graph/Config.toml /graph/Config.toml
fi
chmod u+rwx /graph /graph/uploads
chown -R "$PUID:$PGID" /graph

exec setpriv --reuid="$PUID" --regid="$PGID" --clear-groups /usr/local/bin/graph "$@"
