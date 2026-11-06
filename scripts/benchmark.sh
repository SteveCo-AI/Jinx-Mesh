#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT"
cargo build --locked --release --example measure -j 2
exec /usr/bin/time -f 'process_max_rss_kib=%M elapsed_wall_seconds=%e' \
    target/release/examples/measure
