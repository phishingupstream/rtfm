# RTFM integration

Use the standalone `rtfm` executable on `PATH` to search local documentation and source. The portable `docs` skill searches when API, configuration, error, or version details need evidence and checks cached revisions against the project's dependency version.

```bash
rtfm deps . --package pypi:fastapi --json
rtfm status
rtfm sources
rtfm versions fastapi
rtfm search "lifespan" --library fastapi --version master --top-k 5
rtfm search-code "lifespan" --library fastapi --context 3
rtfm show <result-id>
```

Use `rtfm discover ecosystem:name@exact-version` for explicit registry metadata lookup and `rtfm add ecosystem:name@exact-version --project` to configure a published artifact; then explicitly update the printed source name. Separate sources can bind to the same package with `add NAME LOCATION --project --package SPEC --role docs|code|both`; verify repository release correspondence before binding. See the [dependency coverage runbook](../README.md#discover-dependency-coverage) for coverage evidence and supported manifests.

For RTFM 1.4+, global `--output json` exposes versioned outcomes and contextual recovery actions; check `ok`, then inspect `data` or `error`. Execute authorized actions using their argument arrays and working directory, and check search `data.has_more` and excerpt truncation. Legacy `--json` payloads remain supported.

Inspect dependency inventory only when version or coverage is uncertain; `--package` targets the relevant dependency. Discovery JSON supplies reviewable argument arrays and explicit update actions; unbound source suggestions remain unproven leads.

Choose a configured ref or cached revision shown by `rtfm versions`; `master` above is an example. Searches use the local cache. `rtfm update <library>` explicitly populates or refreshes a source; see `rtfm --help` and subcommand help for configuration and commands.

The Claude/Codex plugin's startup hook prints a short cache summary, then runs `rtfm update-if-due` for previously cached sources. When GNU `timeout` is available the refresh is bounded to eight seconds, with forced process-group termination after one further second. If `timeout` is unavailable the automatic refresh is skipped; an incomplete or skipped refresh warns without blocking the session. The plugin requires the standalone binary; if it is unavailable the hook exits quietly. There are no per-file or post-edit hooks.

In Codex, open `/hooks` after installation or update to review and trust the current RTFM startup hook. Plugin installation does not grant hook trust; see [Codex hook policy](https://developers.openai.com/plugins/build/plugins#configure-installed-server-policy).

The bundled portable skill is in [skills/docs/SKILL.md](skills/docs/SKILL.md). Use your agent’s skill or plugin installer to load this directory; install the `rtfm` executable separately and make it available on the agent’s PATH.

For installation, cache configuration, source definitions, and recovery, use the [RTFM runbook](../README.md).
