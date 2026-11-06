#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
# File managers provide no interactive terminal for the chat or password prompts.
if [ "${1:-}" != "--terminal" ] && { [ ! -t 0 ] || [ ! -t 1 ]; }; then
    if command -v konsole >/dev/null 2>&1; then
        exec konsole -e "$ROOT/start.sh" --terminal
    elif command -v xdg-terminal-exec >/dev/null 2>&1; then
        exec xdg-terminal-exec "$ROOT/start.sh" --terminal
    elif command -v gnome-terminal >/dev/null 2>&1; then
        exec gnome-terminal -- "$ROOT/start.sh" --terminal
    else
        echo "Bitte start.sh in einem Terminal ausführen." >&2
        exit 1
    fi
fi
if [ "${1:-}" = "--terminal" ]; then
    shift
fi
# Keep a terminal opened by the launcher readable when startup fails.
finish() {
    code=$?
    if [ "$code" -ne 0 ] && [ -t 0 ]; then
        printf '\nStart fehlgeschlagen. Eingabetaste zum Schließen … '
        read -r ignored || true
    fi
}
trap finish EXIT
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
"$BIN" chat "$VAULT"
