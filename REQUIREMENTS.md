# RTFM Requirements

## Standalone CLI

These requirements define the standalone CLI. Retired server requirements are outside this distribution.

| ID | Active requirement |
|---|---|
| R-1 | Installable standalone Rust CLI; no server, embeddings, reranker, or model dependency. |
| R-2 | Persistent local SQLite FTS5 BM25 documentation index; offline search/read/status with bounded useful excerpts, source paths, revision and line ranges. |
| R-3 | `search` searches documentation with BM25; `search-code QUERY` searches configured source paths directly in retained snapshots, literally by default with `--regex` opt-in. Support `--library`, `--version`, `--path` glob, `--context`, `--top-k`, and `--json`; return matching line ranges and provenance. Preserve whole source for inspection; do not index code chunks. Removed `search --code` reports migration to `search-code`. |
| R-4 | Git subprocess fetching and immutable revision checkouts; list/select cached refs and exact revisions without changing existing readers. |
| R-5 | Local files/directories, source archives and web documentation snapshots retain hashes and provenance without claiming Git history. |
| R-6 | Fetch/update selected sources; unchanged content reuses indexing; failures preserve the previous published index and selection. |
| R-7 | User configuration and project .rtfm.toml selections; pinned refs do not silently upgrade; explicit uncached-version errors. |
| R-8 | Throttled locked update-if-due and small local agent integration; explicit refresh reports failures, startup warnings do not block sessions. |
| R-9 | Import existing third-party source without modifying or discarding original cache contents; retain unknown provenance explicitly. |
| R-10 | Isolated fixture tests cover versions, docs/code filtering, source deletion/change, failure recovery, and concurrent access; packaged binary smoke test and retained CI evidence. |
| R-11 | Retire HTTP service and vector infrastructure only after installed CLI and agent integration work; preserve retained service data for rollback. |
| R-12 | Discover ecosystem-qualified exact packages through PyPI, npm, and crates.io metadata; expose artifact, repository and documentation candidates with evidence, proposed source name, and exact add/update commands in human and JSON output when an artifact exists; never assume a release maps to a Git tag. Explicit discovery/add may access registries; search and dependency coverage remain offline. |
| R-13 | Inventory direct project dependencies from manifests and lockfiles; report documentation and code coverage independently, including unresolved versions and declared versus registry-backed mappings. Project mappings and extra sources override shared defaults without copying inherited configuration. |
| R-14 | Explicitly add registry artifacts or agent-selected source/path overrides and fetch through update; verify supplied artifact checksums before publication, retain source provenance and immutable snapshots, and reuse the shared cache across projects. |
| R-15 | Targeted dependency filters and explicitly unverified existing-source suggestions reduce lookup noise. Discovery exposes safe structured next actions and suggested path metadata; opt-in search diagnostics distinguish absent coverage from no matches without changing default JSON arrays. CLI help explains identities and recovery; source excerpts have line numbers and show omits ranking-only scores. |

## CLI workflow contract

R-16: Preserve legacy `--json` payloads. Global `--output json` opts into schema version 1 with success/data, typed errors, contextual argv recovery, and structured mutation outcomes. Parser errors exit 2; operational errors exit 1; empty successful reads, due no-ops and closed stdout exit 0. Configuration writes use the effective selected file; `--project` explicitly writes the current directory overlay and conflicts with explicit configuration. Searches are bounded to 100 results with bounded excerpts and explicit truncation; `show` retrieves full chunks. Help and README define identities, scope, network boundaries and recovery across every command.

## Public distribution

R-17: Source installation and container builds work without private infrastructure, credentials or a private registry. Document supported platforms and prerequisites, include the CC BY-NC 4.0 license, and verify installation and offline setup in disposable containers. Relative catalog paths retain their original meaning when imported elsewhere (R-7/R-9). Heading breadcrumbs follow actual Markdown levels, including skipped levels (R-2). Dependency reporting uses one inventory per invocation (R-13).
