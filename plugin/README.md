# RTFM integration

Give your agent access to the docs and source for your project's dependencies. Install the standalone `rtfm` executable on the agent's `PATH`, then use your agent's skill or plugin installer to load this directory. The portable [docs skill](skills/docs/SKILL.md) guides dependency discovery, cache updates and lookup when API, configuration, error or version details need evidence.

```bash
rtfm deps . --package pypi:fastapi --json
rtfm status
rtfm sources
rtfm versions fastapi
rtfm search "lifespan" --library fastapi --version master --top-k 5
rtfm search-code "lifespan" --library fastapi --context 3
rtfm show RESULT_ID
```

Replace `RESULT_ID` with an ID returned by documentation search. Use `rtfm discover ecosystem:name@exact-version` for explicit registry metadata lookup and `rtfm add ecosystem:name@exact-version --project` to configure a published artifact; then explicitly update the printed source name. Separate sources can bind to the same package with `add NAME LOCATION --project --package SPEC --role docs|code|both`; verify repository release correspondence before binding. See the [dependency coverage guide](../docs/usage.md#discover-dependency-coverage) for coverage evidence and supported manifests.

For RTFM 1.4+, global `--output json` exposes versioned outcomes and contextual recovery actions; check `ok`, then inspect `data` or `error`. Execute authorized actions using their argument arrays and working directory, and check search `data.has_more` and excerpt truncation. Legacy `--json` payloads remain supported.

Inspect dependency inventory only when version or coverage is uncertain; `--package` targets the relevant dependency. Discovery JSON supplies reviewable argument arrays and explicit update actions; unbound source suggestions remain unproven leads.

Choose a configured ref or cached revision shown by `rtfm versions`; `master` above is an example. Searches use the local cache. `rtfm update <library>` explicitly populates or refreshes a source; see `rtfm --help` and subcommand help for configuration and commands.

The Claude/Codex plugin's startup hook prints a short cache summary, then runs `rtfm update-if-due` for previously cached sources. When GNU `timeout` is available the refresh is bounded to eight seconds, with forced process-group termination after one further second. If `timeout` is unavailable the automatic refresh is skipped; an incomplete or skipped refresh warns without blocking the session. The plugin requires the standalone binary; if it is unavailable the hook exits quietly. There are no per-file or post-edit hooks.

In Codex, open `/hooks` after installation or update to review and trust the current RTFM startup hook. Plugin installation does not grant hook trust; see [Codex hook policy](https://developers.openai.com/plugins/build/plugins#configure-installed-server-policy).

See the [README](../README.md#install) for installation and the [usage guide](../docs/usage.md) for cache configuration, source definitions and recovery.
