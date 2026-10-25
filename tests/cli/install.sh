#!/bin/sh
# R-1/R-7/R-9/R-17: fresh source installation and offline setup, from repo root.
set -eu
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cargo install --path rust --locked --root "$work/install"
export PATH="$work/install/bin:$PATH"
export HOME="$work/home" XDG_CONFIG_HOME="$work/config" XDG_CACHE_HOME="$work/cache"
unset RTFM_CONFIG RTFM_CACHE_DIR
mkdir -p "$HOME"
rtfm --version
# The shipped catalog must import without needing any private environment.
rtfm --config "$work/catalog.toml" init --source sources.toml
rtfm --config "$work/catalog.toml" sources --json > "$work/sources.json"
grep 'https://github.com/fastapi/fastapi' "$work/sources.json"
# Exercise the newly installed executable, not Cargo's build-tree binary.
RTFM_BIN="$work/install/bin/rtfm" sh tests/cli/smoke.sh
# Import a relative catalog into the default user location.
mkdir -p "$work/example/docs"
printf '%s\n' '# API' '### First' 'one' '### Second' 'catalogtoken' > "$work/example/docs/guide.md"
printf '%s\n' '[sources.demo]' 'kind="local"' 'location="docs"' > "$work/example/catalog.toml"
cd "$work"
rtfm init --source example/catalog.toml
rtfm update demo
rtfm search catalogtoken --library demo --json > result.json
grep '"heading": "API > Second"' result.json
rtfm path demo
printf '%s\n' 'Source installation and offline setup passed.'
