# RTFM: `man` for agents.

Find your project’s dependencies. Cache their docs and source. Ready to `grep`, managed by your agent.

Agents write better code when they read the fucking manual. They do that far more often when the right docs and source are already there, ready to search.

## Install

With Rust/Cargo 1.99 or newer, a C compiler/linker, Git and CA certificates installed:

```sh
set -eu
git clone https://github.com/phishingupstream/rtfm.git
cd rtfm
cargo install --path rust --locked
export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
rtfm --version
```

Linux x86-64 is tested; Windows is unsupported and macOS is unverified. You can also [install a prebuilt release](docs/development.md#download-a-release). See [development](docs/development.md) for build prerequisites, container use and installation recovery.

## Get started

From your project root, run `rtfm deps .` to inspect direct Python, npm and Cargo dependencies and their cache coverage.

This example caches FastAPI 0.115.0. Choose the library and release your project uses. If `fastapi` is already configured, inspect `rtfm sources` and [select the required ref](docs/usage.md#match-the-projects-dependency-version).

```sh
set -eu
rtfm add fastapi https://github.com/fastapi/fastapi --project \
  --ref tag:0.115.0 --package pypi:fastapi@0.115.0 \
  --docs 'docs/en/docs/**' --code 'fastapi/**/*.py'
rtfm update fastapi
rtfm search "dependency injection" --library fastapi
rtfm search-code 'solve_dependencies' --library fastapi
rtfm path fastapi
```

`add` writes `.rtfm.toml`; commit it to keep the project’s source selection. `update` fetches the source and indexes its docs. Searches then work offline and return revisions, paths and line numbers. `path` prints the complete snapshot directory for direct inspection.

## Use it with your agent

Load the bundled [docs skill](plugin/skills/docs/SKILL.md) through your agent’s skill installer, with `rtfm` on its `PATH`. The skill guides dependency discovery, source selection, updates and lookup. The optional [Claude/Codex plugin](plugin/README.md) adds cache context and a bounded refresh at session start.

The [usage guide](docs/usage.md) covers package discovery, separate docs/code sources, version selection, configuration and recovery. See [development](docs/development.md#develop-and-verify) for tests, or `rtfm --help` for commands.

## License

[CC BY-NC 4.0](LICENSE) — free for non-commercial use with attribution. Commercial use requires a separate paid license from the copyright holder. Third-party dependencies retain their own licenses.
