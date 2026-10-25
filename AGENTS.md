# RTFM agent guidance

RTFM is a standalone Rust CLI for a versioned local documentation and source cache. Read [README.md](README.md) for installation and first use, and [the usage guide](docs/usage.md) for source management, configuration and recovery; use `rtfm --help` and subcommand help for syntax. [REQUIREMENTS.md](REQUIREMENTS.md) defines approved product behavior; trace behavior changes and verification to its IDs and update it before adding or removing capabilities.

## Change ownership

- `rust/src/main.rs`: CLI orchestration and explicit update boundaries.
- `rust/src/config.rs`: source definitions, user/project selection, and path precedence.
- `rust/src/deps.rs`: offline direct-dependency inventory and version evidence.
- `rust/src/registry.rs`: exact-version registry metadata and artifact candidates; repository URLs do not establish release refs.
- `rust/src/packages.rs`: explicit package-to-source bindings and independent docs/code coverage.
- `rust/src/cache.rs`: source acquisition, immutable snapshots, and provenance.
- `rust/src/chunker.rs`: documentation selectors, chunk text, headings, and line ranges.
- `rust/src/diagnostics.rs`: opt-in offline role coverage and empty-search explanations.
- `rust/src/code_search.rs`: direct literal/regex snapshot search, code selectors, context, and matching line ranges.
- `rust/src/store.rs`: SQLite FTS5 persistence, transactional publication, retained versions, and ranking.
- `rust/tests/cli.rs` and `tests/cli/smoke.sh`: compiled CLI behavior and packaged acceptance.
- `sources.toml`: initial catalog; source selectors affect indexing, not complete snapshot retention.
- `plugin/`: bundled agent integration; keep its docs skill, hooks and CLI examples consistent with the executable.

## Contracts

- Dependency inventory and coverage stay offline; registry discovery is explicit and never infers release correspondence from repository names or tags. Follow [the usage guide](docs/usage.md#discover-dependency-coverage) for package bindings and project overrides.
- Search and inspection operate offline. Network/source refresh belongs to explicit `update` or the bounded session-start `update-if-due` integration. Do not add daemons, model services, embeddings, or a separate search server.
- Preserve complete source trees and previously published revisions. Never move, delete, edit, or live-patch imported originals or third-party libraries. Local imports make owned copies; Git origins use installed Git and owned detached checkouts sharing cached objects; recursively populated submodules follow the recorded gitlinks. Publish completed snapshots atomically. Failed acquisition/indexing leaves the last good version searchable.
- Match dependency versions to configured refs or exact cached revisions. Keep branch refresh distinct from immutable tag/commit pins, and preserve project `.rtfm.toml` selection semantics. Searches return revision, provenance, local path, and exact line range; `search` ranks documentation with BM25; `search-code` scans configured source files literally unless `--regex` is explicit. Do not index code chunks.
- Keep configuration and cache independent of former server storage. Defaults, override precedence, update interval, and failure recovery have one operational authority in [the usage guide](docs/usage.md#configuration-and-refresh-behavior).

## Verification

Run the Cargo checks and container installation tests in [the development guide](docs/development.md#develop-and-verify). Preserve observable offline lookup, version selection, docs/code separation, complete acquisition and failure-preservation tests. Use repository-owned Bake targets for container builds. Keep README, requirements, source catalog and plugin consistent. Public releases contain the tested source commit, license, supported-platform binary and checksums.
