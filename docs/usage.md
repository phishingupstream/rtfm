# Using RTFM

Start with the [agent example and setup](../README.md#use-it-with-your-agent). The [docs skill](../plugin/skills/docs/SKILL.md) guides the agent's lookup workflow; the [plugin guide](../plugin/README.md) covers startup hooks. For direct use, `rtfm --help` and `rtfm <command> --help` provide syntax and examples.

## Discover dependency coverage

Documentation and code coverage are independent. A package artifact may contain source without upstream docs; add a separate documentation source when needed. See `rtfm add --help` for package bindings and roles, and the [source catalog](../sources.toml) for selector examples.

`cached` means content is available for a binding. It does not prove that a manually declared repository ref matches the package release. Check the project's dependency version against the source's provenance before using it. Unresolved versions and missing coverage remain gaps; similar names and repository URLs are not proof of a match.

## Configuration and refresh behavior

Run `add` or `use` with `--project` from your project root to save selections in `.rtfm.toml`. Commit that file when the selections belong to the project. Its entries override matching user settings and inherit the rest.

Configuration precedence is `--config`, `RTFM_CONFIG`, the nearest `.rtfm.toml`, then `$XDG_CONFIG_HOME/rtfm/config.toml` (default `~/.config/rtfm/config.toml`). Cache precedence is `--cache-dir`, `RTFM_CACHE_DIR`, then `$XDG_CACHE_HOME/rtfm` (default `~/.cache/rtfm`). Relative source paths resolve against the catalog's directory.

Searches stay offline. Due refresh uses `update_interval_seconds` (default `900`) and only refreshes previously populated refs; new sources need an explicit update. Branches advance on refresh; acquired tag and commit pins stay fixed. Previous revisions remain available, and failed updates preserve the last usable snapshot.

Snapshots retain complete source trees; selectors limit indexing and search, not retention. Local imports copy the originals. Do not edit cached snapshots or the originals to fix a lookup.

## When a lookup fails

| Situation | Next step |
|---|---|
| Missing or wrong version | Inspect `rtfm versions NAME`; use `rtfm use --help` to select the required ref, then explicitly update it. |
| No matches | Check `rtfm sources`, the selected version and selectors. Search `--diagnostics` distinguishes missing coverage from an empty result. |
| Failed refresh | Preserve stderr and existing snapshots; retry `rtfm update NAME`. If a snapshot was modified, preserve the changes and investigate the reported path. |
| Incomplete excerpt | Use `rtfm show` for a complete docs chunk or `rtfm path` to inspect the source directly. |

For agent integrations, follow the [docs skill's machine-output and recovery rules](../plugin/skills/docs/SKILL.md). The [output implementation](../rust/src/output.rs) and [CLI contract tests](../rust/tests/cli_contract.rs) define the detailed interface; command errors must not be treated as empty search results.
