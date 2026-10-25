#!/bin/sh
# Packaged binary acceptance: no network or model files.
set -eu
rtfm_bin=${RTFM_BIN:-rtfm}
"$rtfm_bin" --version
if [ -n "${RTFM_EXPECTED_COMMIT:-}" ]; then
    "$rtfm_bin" --version | grep -F "$RTFM_EXPECTED_COMMIT"
fi
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
export RTFM_CONFIG="$work/config.toml" RTFM_CACHE_DIR="$work/cache"
mkdir -p "$work/upstream/docs" "$work/upstream/src"
printf '%s\n' '# Frobnication' 'The tracerbullet documentation describes frobnication.' > "$work/upstream/docs/guide.md"
printf '%s\n' 'fn codeonlysymbol() {}' > "$work/upstream/src/lib.rs"
"$rtfm_bin" init
"$rtfm_bin" add fixture "$work/upstream" --kind local --docs 'docs/**' --code 'src/**' --package npm:fixturedep@1.0.0
printf '%s\n' '{"dependencies":{"fixturedep":"1.0.0"}}' > "$work/package.json"
"$rtfm_bin" update fixture
"$rtfm_bin" search tracerbullet --lib fixture | grep tracerbullet
if "$rtfm_bin" search codeonlysymbol --lib fixture | grep -q codeonlysymbol; then
    echo 'code leaked into docs search' >&2; exit 1
fi
"$rtfm_bin" search-code codeonlysymbol --lib fixture | grep codeonlysymbol
"$rtfm_bin" versions fixture
"$rtfm_bin" path fixture
"$rtfm_bin" status

"$rtfm_bin" deps "$work" --json > "$work/coverage.json"
[ "$(grep -c '"status": "cached"' "$work/coverage.json")" = 2 ]

# Invalid mutations must preserve both configuration bytes and published content.
cp "$RTFM_CONFIG" "$work/before.toml"
if "$rtfm_bin" use fixture --ref '' > "$work/invalid.out" 2> "$work/invalid.err"; then
    echo 'empty ref was accepted' >&2; exit 1
fi
cmp "$RTFM_CONFIG" "$work/before.toml"
if "$rtfm_bin" add bad "$work/upstream" --kind local --docs '['; then
    echo 'invalid selector was accepted' >&2; exit 1
fi
cmp "$RTFM_CONFIG" "$work/before.toml"
"$rtfm_bin" search tracerbullet --lib fixture | grep tracerbullet

# Removing the origin must leave the last good publication available.
mv "$work/upstream" "$work/retained-original"
if "$rtfm_bin" update fixture; then
    echo 'missing origin update succeeded' >&2; exit 1
fi
"$rtfm_bin" search tracerbullet --lib fixture | grep tracerbullet
"$rtfm_bin" search-code codeonlysymbol --lib fixture | grep codeonlysymbol

# Preserve machine samples for JSON assertions in the CI tooling container.
if [ -n "${RTFM_ACCEPTANCE_OUTPUT:-}" ]; then
    mkdir -p "$RTFM_ACCEPTANCE_OUTPUT"
    "$rtfm_bin" --output json status > "$RTFM_ACCEPTANCE_OUTPUT/status.json"
    "$rtfm_bin" --output json search utterlymissing > "$RTFM_ACCEPTANCE_OUTPUT/empty.json"
    "$rtfm_bin" --output json search-code codeonlysymbol > "$RTFM_ACCEPTANCE_OUTPUT/code.json"
    if "$rtfm_bin" --output json use fixture --ref '' > "$RTFM_ACCEPTANCE_OUTPUT/invalid.json"; then
        echo 'machine mutation accepted empty ref' >&2; exit 1
    fi
fi
