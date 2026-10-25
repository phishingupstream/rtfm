# Developing RTFM

The standalone CLI lives in [rust/](../rust/). For first use and agent integration, see the [README](../README.md); [AGENTS.md](../AGENTS.md) defines change ownership and repository contracts.

## Download a release

[GitHub Releases](https://github.com/phishingupstream/rtfm/releases) provide a Linux x86-64 binary requiring glibc 2.36 or newer. Download `rtfm-1.4.0-linux-x86_64.tar.gz` and `SHA256SUMS` from the same release, then:

```sh
set -eu
sha256sum -c SHA256SUMS
tar -xzf rtfm-1.4.0-linux-x86_64.tar.gz
install -Dm755 rtfm-1.4.0-linux-x86_64/rtfm "$HOME/.local/bin/rtfm"
export PATH="$HOME/.local/bin:$PATH"
rtfm --version
```

Git is needed for Git sources. The archive includes the product license and third-party notices. Only Linux x86-64 is verified for this release; macOS is unverified and Windows is unsupported.

## Install from source

Supported and tested on Linux x86-64. The implementation uses Unix filesystem and process APIs; Windows is not supported, and macOS is not currently verified. Building requires Rust/Cargo 1.99 or newer and a C compiler/linker for bundled SQLite. Git and CA certificates are needed for HTTPS Git sources. On Debian/Ubuntu, install `build-essential git ca-certificates` and a Rust toolchain before continuing.

Clone the repository and install:

```sh
set -eu
git clone https://github.com/phishingupstream/rtfm.git
cd rtfm
cargo install --path rust --locked
export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
rtfm --version
```

The executable is installed under `${CARGO_HOME:-$HOME/.cargo}/bin`. SQLite is bundled; no database service is required. Re-run the command after checking out an updated release. Configuration and cached snapshots are retained independently of the executable. If `rtfm` is not found, check the PATH above; compiler/linker errors usually mean the toolchain or C build tools are missing.

## Build and run a container

Docker with Buildx can build the same executable without a host Rust installation. From the repository root:

```sh
set -eu
docker buildx bake --load
docker run --rm rtfm-cli:local --version
docker volume create rtfm-data
docker run --rm -v rtfm-data:/data -e RTFM_CONFIG=/data/config.toml -e RTFM_CACHE_DIR=/data/cache rtfm-cli:local init --source /usr/local/share/rtfm/sources.toml
docker run --rm -v rtfm-data:/data -e RTFM_CONFIG=/data/config.toml -e RTFM_CACHE_DIR=/data/cache rtfm-cli:local update fastapi
docker run --rm -v rtfm-data:/data -e RTFM_CONFIG=/data/config.toml -e RTFM_CACHE_DIR=/data/cache rtfm-cli:local search lifespan --library fastapi
```

Keep the volume mounted at the same path: the index records absolute snapshot paths. Repeat the environment and mount options for each invocation. The image runs one CLI command and exits; it has no server or listening port. `IMAGE_REPOSITORY` and `CI_COMMIT_SHA` configure the Bake image name and tag; publication is explicit with `--push`.

## Install a verified binary

If you have a verified image built for your Linux architecture, extract its binary with Docker. This path requires a host compatible with the image's Debian Bookworm runtime (glibc 2.36 or newer); otherwise use the container or compile from source.

```sh
set -eu
RTFM_IMAGE=rtfm-cli:local
rtfm_container=$(docker create "$RTFM_IMAGE")
trap 'docker rm "$rtfm_container" >/dev/null' EXIT
mkdir -p "$HOME/.local/bin"
docker cp "$rtfm_container:/usr/local/bin/rtfm" "$HOME/.local/bin/rtfm"
export PATH="$HOME/.local/bin:$PATH"
rtfm --version
```

Without a Docker daemon, use `crane` and `tar` to extract an image by digest:

```sh
set -eu
: "${RTFM_IMAGE:?Set RTFM_IMAGE to a verified repository@sha256:digest}"
rtfm_install_dir=$(mktemp -d)
trap 'rm -rf "$rtfm_install_dir"' EXIT
crane export "$RTFM_IMAGE" "$rtfm_install_dir/image.tar"
tar -xf "$rtfm_install_dir/image.tar" -C "$rtfm_install_dir" usr/local/bin/rtfm
install -Dm755 "$rtfm_install_dir/usr/local/bin/rtfm" "$HOME/.local/bin/rtfm"
"$HOME/.local/bin/rtfm" --version
```

For rollback, repeat with an earlier verified image or install from an earlier source release. Preserve configuration and cache state.

## Develop and verify

The CLI writes results to stdout and progress/errors to stderr; redirect either stream when retaining verification output.

The CLI lives in [rust/](../rust/). With Rust, rustfmt, Clippy, a C compiler and Git installed, run:

```bash
set -e
cd rust
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked
```

To run these checks in disposable containers, first [build `rtfm-cli:local`](#build-and-run-a-container), then run from the repository root:

```sh
set -eu
docker run --rm -v "$PWD:/source:ro" rust:1.99-bookworm sh -ec 'cp -a /source /tmp/rtfm; cd /tmp/rtfm/rust; rustup component add rustfmt clippy; cargo fmt --check; cargo clippy --locked --all-targets -- -D warnings; cargo test --locked; cd ..; sh tests/cli/install.sh'
docker run --rm -v "$PWD:/source:ro" --entrypoint sh rtfm-cli:local /source/tests/cli/smoke.sh
```

The installation test builds and installs into a fresh temporary prefix, then exercises local setup, catalog import and offline search. The image smoke test exercises the packaged executable. Neither test needs credentials or external source repositories. Cargo needs registry access to build dependencies unless they are already cached. [AGENTS.md](../AGENTS.md) describes change ownership.

For performance comparisons, run [the offline benchmark](../tests/cli/benchmark.py) with Python 3 in an isolated environment. Supply two release binaries, an existing configuration, and a writable copy of its populated cache; keep the snapshot roots recorded in that cache accessible. For example, from the checkout:

```bash
python3 tests/cli/benchmark.py /work/baseline-rtfm /work/candidate-rtfm \
  --config /config/config.toml --cache-dir /work/cache \
  --library fastapi --query "dependency injection" --code-query solve_dependencies \
  --rounds 9 --output /evidence/benchmark.json
```

The benchmark warms each command, alternates binary order, and records median milliseconds, individual samples, and output hashes; differing output fails the run. It performs offline reads without refreshing sources. Use a library and queries present in the supplied cache, profile a release build before editing, and compare one optimization at a time (R-2/R-6/R-10).

