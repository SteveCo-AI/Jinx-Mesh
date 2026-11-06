#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
umask 077
VAULT_DIR="$ROOT/private"
VAULT="$VAULT_DIR/jinx.db"
BIN="$ROOT/bin/saeugling-linux-x86_64"

if [ ! -x "$BIN" ]; then
    echo "Jinx-Binärdatei fehlt: $BIN" >&2
    exit 1
fi

mkdir -p -m 700 "$VAULT_DIR"
chmod 700 "$VAULT_DIR"
if [ ! -e "$VAULT" ]; then
    "$BIN" init "$VAULT"
fi
exec "$BIN" chat "$VAULT"
