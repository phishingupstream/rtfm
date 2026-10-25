# Using RTFM

See the [README](../README.md) for installation and a first lookup. Use `rtfm --help` or `rtfm <command> --help` for command syntax.

## Populate and search the cache

From the repository root with `rtfm` on `PATH`, initialize the optional catalog on a fresh configuration:

```bash
set -e
rtfm init --source sources.toml
rtfm update fastapi
rtfm search "dependency injection" --library fastapi --top-k 5
rtfm versions fastapi
rtfm path fastapi
```

`init` creates configuration without fetching and refuses to overwrite an existing file. The catalog enables a subset of its sources; edit the configuration to enable others. React, ChromaDB and Langfuse publish docs separately from implementation: use `react-code`, `chromadb-code` or `langfuse-code` with `rtfm search-code` for their implementation repositories. Their revisions are deliberately independent of the documentation repository. `update NAME` fetches and indexes every configured ref for that enabled source. `update` without a name populates all enabled sources. After population, `search`, `search-code`, `show`, `path`, `versions`, `sources`, and `status` use local state.

`rtfm search QUERY` ranks documentation chunks with BM25; `rtfm show <result-id>` reads a complete documentation chunk. `rtfm search-code QUERY` searches configured source files directly in the selected snapshot without indexing code chunks. Matching is line-oriented and case-sensitive; code queries are literal by default, with `--regex` for regular expressions. Code search respects source exclusions and skips binary files, non-UTF-8 files, and files larger than 2 MiB. Matches use deterministic library/path/line order rather than relevance ranking. Both commands support `--library`, `--version`, `--top-k`, and `--json`; code search also supports `--path` glob and `--context` for surrounding lines (default 2, maximum 20). Human code results include numbered context lines; JSON preserves the matching line range and excerpt. Read code results using their returned path and line range. `search --code` was removed; migrate to `search-code`.

```bash
rtfm search-code 'def model_validate' --library pydantic --path 'pydantic/**/*.py' --context 3 --top-k 5
rtfm search-code 'async def .*lifespan' --library fastapi --regex --json
```

`--json` also provides structured chunk, version, source, and status output. Plain `search --json` retains its result-array contract; opt into `search --diagnostics --json` for an envelope containing results and coverage diagnostics. Diagnostics explain missing snapshots or empty documentation coverage without refreshing sources. Results identify provenance and exact revisions; documentation relevance scores only rank matches, and `show` omits a search score.

## Match the project's dependency version

A source can retain several refs and every revision previously indexed. Git accepts `HEAD`, branch names or `branch:<name>`, `tag:<name>`, and `commit:<sha>`. Branches refresh to their latest fetched commit; acquired tag and commit pins stay fixed. Match project lockfiles or installed metadata to the cached version before applying documentation.

```bash
set -e
rtfm use fastapi --ref tag:0.115.0
rtfm update fastapi
rtfm search "lifespan" --library fastapi --version tag:0.115.0
rtfm versions fastapi
```

The tag above is an example of selecting a specific release. `use` changes the selected ref and adds it to configured refs; fetching remains explicit. `--version` also accepts an exact retained revision printed by `versions`, so older snapshots remain searchable after a branch update. Without `--version`, searches use the selected ref, or the source's first configured ref.

For a project-specific default, run `rtfm use fastapi --ref tag:0.115.0 --project` from the project root. This writes its selection to `.rtfm.toml`. RTFM finds the nearest `.rtfm.toml` while walking up from the current directory and merges it over the user configuration; project source definitions and selections override matching user entries. Commit project selections when they express shared dependency choices.

## Discover dependency coverage

When dependency versions or cache coverage need investigation, run `rtfm deps . --json` from a project to inventory direct Python, npm and Cargo dependencies and inspect documentation and code coverage separately. It reads `pyproject.toml`, `package.json` and `Cargo.toml`, using `uv.lock`, `package-lock.json` and `Cargo.lock` where available; hidden directories, experiments and generated dependency/build trees are skipped. Unsupported lock formats and unresolved constraints remain coverage gaps rather than guessed versions. Coverage uses explicit `ecosystem:name@exact-version` bindings, not similar library names; `cached` means content is available for the bound source/ref, not that a manually declared repository ref has been proven to match the package release. Inspect JSON evidence and source provenance before applying it. Use `--package ecosystem:name` to filter the report to the relevant direct dependency. Unbound source suggestions are name-based leads for inspection, not coverage or proof of a matching release.

```bash
set -e
rtfm deps . --package pypi:fastapi --json
rtfm discover pypi:fastapi@0.115.0 --json
rtfm add pypi:fastapi@0.115.0 --project
```

`discover` queries PyPI, npm or crates.io for exact-version metadata and candidate sources without configuring or downloading snapshots. Repository URLs are candidates, not inferred release tags. GitHub browser URLs retain their original link and unresolved ref/path hint separately from the clone root; explicit npm repository directories scope suggested paths. Human output prints the proposed source name and shell-ready add/update commands. JSON includes the proposed `source_name` and, when a published artifact is available, structured `next_actions` with `argv` arrays for configuration and explicit update; inspect these before executing them. Suggested selectors include common source extensions such as `.vue` and `.svelte`; repository subdirectory hints help scope monorepo paths but do not establish a release ref or guarantee that files exist. The package form of `add` configures the registry's published source artifact and available checksum; it prints the source name to use with `rtfm update NAME`. Neither `deps` nor `add` populates the cache. Published artifacts can omit upstream documentation, so inspect their coverage after updating and add a separate documentation source if needed.

Use `rtfm add NAME LOCATION --project --package pypi:fastapi@0.115.0 --role docs --kind git --ref tag:0.115.0 --docs 'docs/**'` to declare a separate source binding after verifying its release correspondence. `--role docs`, `code` or `both` (the default) controls which coverage binding is written; custom NAME LOCATION requires `--package` when `--role` is explicit; `--docs` and `--code` still control actual searchable paths. `--project` writes the current directory's `.rtfm.toml`; without it, `add` writes the effective configuration file (nearest project overlay or user configuration), unless `--config` or `RTFM_CONFIG` explicitly selects a destination. Package mappings and source definitions remain configuration only until an explicit update.

## Add or import sources

`rtfm add NAME LOCATION` adds configuration without fetching. Use `--kind git` (default), `local`, `url`, or `archive`; repeat `--ref`, `--docs`, and `--code` for multiple values. Documentation and code selectors are relative glob patterns; docs selectors control indexing and code selectors bound direct source search. Repeat `--exclude` to omit matching paths, and use `--checksum sha256:<hex>` or `--checksum sha512-base64:<base64>` to verify archive bytes before extraction. The TOML source configuration also supports `exclude` patterns and `enabled = false`; see [sources.toml](../sources.toml) for catalog examples.

Import an existing working tree without moving, editing, or deleting the original:

```bash
set -e
rtfm add pydantic-local ./checkouts/pydantic --kind local --docs 'docs/**' --docs '*.md' --code 'pydantic/**/*.py'
rtfm update pydantic-local
rtfm search-code "model_validate" --library pydantic-local
```

Substitute the existing source directory's actual name. Local sources copy the full tree into an owned content-addressed snapshot, excluding `.git` metadata. A single local file can also be imported. For a local Git repository with revision history, use `--kind git --ref tag:<release>` instead; RTFM reads it as an origin and owns its own bare repository and stable detached checkouts sharing those Git objects. The original working tree remains untouched in either case.

URL sources support individual text/HTML documents, `llms-full.txt`, and `llms.txt` indexes with linked Markdown documents. Archive sources support tar, tar.gz, and ZIP and preserve a wrapper directory, so selectors must include it. Unsupported formats and archive links fail explicitly. Git snapshots include recursive submodules at the commits recorded by their parent repositories. A snapshot is published only after the complete checkout succeeds; failed fetches leave the last indexed version available. Source snapshots contain the complete supported tree; selectors control documentation indexing and code search rather than what is retained.

## Configuration and refresh behavior

Relative local paths in a source catalog resolve against that catalog’s directory, including when imported into a different configuration location. `rtfm init` without `--source` creates an empty configuration; the shipped catalog is optional.

Configuration precedence is `--config`, `RTFM_CONFIG`, the nearest `.rtfm.toml`, then `$XDG_CONFIG_HOME/rtfm/config.toml` (default `~/.config/rtfm/config.toml`). Cache precedence is `--cache-dir`, `RTFM_CACHE_DIR`, then `$XDG_CACHE_HOME/rtfm` (default `~/.cache/rtfm`). The cache contains `index.sqlite3` and `sources/<name>/revisions/<revision>/`; Git sources also retain `objects.git`. Do not edit cached snapshots.

`rtfm update-if-due` and `rtfm update --if-due` refresh already populated, enabled refs after `update_interval_seconds` has elapsed (default 900). They skip never-cached refs, so a new source needs an explicit `rtfm update NAME`. Searches never trigger an update. A failed refresh reports errors and preserves the previous indexed version; run `rtfm update NAME` to diagnose or retry it, then inspect `rtfm versions NAME` and a focused search.

Use `rtfm --help` and `rtfm <command> --help` for available flags, examples, and recovery guidance. If a selected version is unavailable, run `rtfm versions NAME`, then either choose a listed ref/revision or configure the required ref with `rtfm use NAME --ref REF` and explicitly run `rtfm update NAME`.

If a search has no matches, check `rtfm sources`, `rtfm versions NAME`, and the chosen ref before changing the query. Missing versions require explicit population; a narrow documentation selector can exclude the desired content. If a cached snapshot was modified, preserve the changed files and diagnose the reported path rather than deleting the source or silently accepting changed provenance.

## Coding agents

The portable [docs skill](../plugin/skills/docs/SKILL.md) resolves uncertainty about APIs, configuration, errors, and version differences using the CLI. Use it with an agent that supports portable skills; the CLI is also usable directly without an agent integration.

The optional [Claude/Codex plugin](../plugin/README.md) adds concise cache context and a bounded `update-if-due` check at startup. The agent chooses searches from the task and checks dependency versions against returned provenance.

## Human and machine command contract

A package identity (`pypi:fastapi@0.115.0`) identifies an exact registry release; a source name (`fastapi`) identifies a configured acquisition and selector definition. A ref is a configured label; a revision is immutable snapshot identity. `current` in versions means the latest published snapshot of a ref, while its independent `selected` boolean/marker identifies the snapshot currently chosen by effective configuration. `sources` exposes acquisition location, configuration origin, selected ref and documentation/code/exclusion selectors in both human and JSON output. Cache-local search IDs identify documentation chunks; use source/revision/path/lines as durable external identity. Reindexing the same revision with new selectors can retire IDs; a stale ID fails rather than identifying a different chunk.

| Command | Purpose and identity | Read/write scope | Network and empty outcome |
|---|---|---|---|
| `search` / `search-code` | Docs ranking / literal or regex source lookup | Effective config and selected snapshots; no refresh | Offline; no matches succeeds |
| `show` | Full chunk by cache-local result ID | Cache only | Offline; unknown ID fails |
| `path` | Complete snapshot by source and optional ref/revision | Effective config and cache | Offline; missing selection fails |
| `deps` | Direct dependency inventory and independent docs/code coverage | Requested root, effective project config and cache | Offline; empty inventory succeeds |
| `discover` | Exact package metadata and candidates | Registry only; no configuration write | Network; absent artifact remains evidence |
| `sources` | Configuration, selection and cache coverage | Effective config and cache | Offline; empty list succeeds |
| `versions` | Retained refs/revisions; current is per ref | Cache and effective config for a named source | Offline; unknown/disabled names fail; empty broad list succeeds |
| `status` | Cache counts and paths | Cache | Offline; empty cache succeeds |
| `init` | Create empty config or import a catalog | Effective configuration destination | Offline; existing file fails |
| `add` | Define custom source or published package artifact | Effective configuration destination | Package form queries registry; never fetches snapshots |
| `use` | Select source ref/revision; does not fetch | Effective configuration destination | Offline; repeated selection is unchanged |
| `update` | Acquire enabled configured refs and index docs | Effective config; cache publication | Explicit network/acquisition; partial failure preserves prior snapshots and fails |
| `update-if-due` | Integration compatibility alias of `update --if-due` | Already populated enabled refs | Bounded explicit refresh; absent config, lock contention and not-due refs succeed unchanged |

Explicitly targeting a source requires it to be configured and enabled, including retained-revision inspection. Broad lookup scans enabled configured sources only; cache-only removed sources are excluded. An explicit revision across sources searches those with that exact revision; it fails when none qualify. Explicit project selections with missing snapshots fail before lookup rather than silently falling back.

The common workflow is `deps` → `discover` → `add` → `update` → `search` or `search-code`. `init` is optional setup, while `add` writes a source definition. Searches return bounded excerpts; `show` retrieves the complete stored documentation chunk, and `path` supports complete source inspection. Search limits accept 1–100 results. Documentation chunks contain at most 2,400 bytes, with returned heading metadata bounded to 512 bytes and `heading_truncated` indicating shortening. Code excerpts contain at most 8,192 bytes, with individual lines capped at 1,024 bytes and windows centered around matching text when possible; `text_truncated` and `excerpt_end_line` identify shortened excerpts. Code `start_line`/`end_line` describe the original context span, while `match_lines` identifies the exact matching lines. `excerpt_spans` records each displayed source line and its zero-based, end-exclusive UTF-8 byte range (`line`, `start_byte`, `end_byte`); a long regex match may extend beyond the displayed window. Human output marks partial lines and prints these ranges. Source search skips files over 2 MiB, binary files and non-UTF-8 files; documentation indexing also skips files over 2 MiB. Documentation query words are literal OR alternatives rather than raw FTS syntax: quotes do not make phrases, and words such as AND/OR are ordinary search terms.

`init`, `add`, and `use` write the effective configuration file: explicit `--config`, `RTFM_CONFIG`, nearest project overlay, then user configuration. `--project` on add/use explicitly writes the current directory `.rtfm.toml`; combining it with explicit configuration fails. Overlay writes retain only local entries and inherit unrelated user settings. Generated update actions preserve absolute config/cache paths, argument boundaries and working directory.

Global `--output json` opts into schema version 1 on every command. Existing command `--json` payloads (including default search arrays) remain compatible; their operational errors remain stderr text. The versioned envelope is `{schema_version:1,ok,data,error,next_actions,context}`. `context` contains absolute `config`, `cache_dir`, and `cwd` after argument parsing; parser failures have empty unresolved context (`{}`). Actions contain executable `argv` arrays and `cwd`. Success has `error:null`; errors have stable `code`, human `message`, and structured `details`. Consumers must use codes/details rather than parse messages. Unknown fields may be added within version 1; incompatible changes require a new schema version.

Search envelope `data` contains `results`, `diagnostics`, and `has_more`. Dependency envelope data contains `root`, `manifests`, `exclusions`, and coverage `results`; its context configuration is the one resolved from that requested root, which can differ from the caller’s working directory. Mutations identify `destination`, `changed`, and source/reference where relevant; update additionally returns `updated`, `skipped`, and `failures`, including successful work when another ref fails or is interrupted. Codes include `argument_invalid`, `scope_conflict`, `config_invalid`, `source_unknown`, `source_disabled`, `version_missing`, `update_locked`, `acquisition_failed`, `io_failed`, and fallback `operation_failed`.

Primary output goes to stdout, progress to stderr. Versioned errors go to stdout as one envelope; parser failures exit 2, operational failures exit 1, successful reads/mutations and no-match/empty/no-op outcomes exit 0. Interrupted work exits with 128 plus signal number. Closed stdout exits 0 without panic; other output errors fail. Help/version requests remain ordinary text, and bare invocation prints help successfully. `--version` includes build revision (or `development` for an unlabelled build).
