---
name: docs
description: Resolve uncertain third-party APIs, configuration, errors, version differences, or documentation/source cache coverage. Use when project evidence is insufficient or cache coverage is requested; mentioning a library alone does not require lookup.
---

# Documentation lookup

Use project evidence first. Before applying third-party evidence to the project, match its source/ref to the dependency version. With RTFM 1.4+, use global `--output json` for structured outcomes, errors, execution context and recovery actions; legacy `--json` remains available. Check `ok` before reading `data`, execute authorized `next_actions` using their `argv` and `cwd`, and inspect `data.has_more` and result `text_truncated`/`excerpt_end_line` before treating results as complete. Choose the unresolved question:

- **Dependency version or cache coverage:** `rtfm deps . --package ecosystem:name --json`. Prefixes: `pypi:`, `npm:`, `crates.io:`. For missing coverage, run `rtfm discover ecosystem:name@version --json` (metadata only). Inspect candidates and suggested paths; report or execute returned `next_actions` as authorized. Package identities and source names differ.
- **API or implementation:** `rtfm search` searches docs; `rtfm search-code` searches literal source text. Use subcommand `--help` for flags, `rtfm show` or returned paths for context. Refine weak matches once with exact symbols; scores rank matches, not evidence quality.
- **Version correspondence:** `rtfm sources`, `rtfm versions NAME`, and `rtfm path NAME --version REF` establish cached provenance. Omit `--version` only when the selected ref is appropriate. Cached content, name suggestions and repository URLs do not prove release correspondence. Cite paths, lines and revisions. Code establishes computation; units, intent and release history need separate evidence.
- **Failure or stale coverage:** preserve stderr and check command success before treating empty output as no matches. Use `rtfm add --help` for bindings, `rtfm use --help` for ref selection, and `rtfm update --help` for explicit refresh syntax. Searches stay offline; fetch explicitly. If matching evidence remains unavailable, consult version-matched upstream evidence and state the remaining gap.
