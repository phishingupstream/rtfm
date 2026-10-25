# RTFM: `man` for agents.

Find your project’s dependencies. Cache their docs and source. Ready to `grep`, managed by your agent.

Agents write better code when they read the fucking manual. They do that far more often when the right docs and source are already there, ready to search.

## Use it with your agent

After [installation](#install), ask your agent to use RTFM as part of a feature request. For a FastAPI project:

> Load the model once when this FastAPI app starts and release it on shutdown. Use RTFM to check the lifecycle APIs for the FastAPI version in this project.

The docs skill guides the agent through checking the dependency version, fetching missing reference material and reading the relevant docs or implementation. The agent can cite the source revision and lines behind its choice.

## Install

Install the [prebuilt Linux binary](docs/development.md#download-a-release) or [build from source](docs/development.md#install-from-source), and put `rtfm` on your agent’s `PATH`. Load the bundled [docs skill](plugin/skills/docs/SKILL.md) through your agent’s skill installer. The optional [Claude/Codex plugin](plugin/README.md) also adds cache context and a bounded refresh at session start.

Linux x86-64 is tested; Windows is unsupported and macOS is unverified.

For direct CLI use, start with `rtfm --help`; the [usage guide](docs/usage.md) covers source selection, configuration and recovery. See [development](docs/development.md#develop-and-verify) for tests.

## License

[CC BY-NC 4.0](LICENSE) — free for non-commercial use with attribution. Commercial use requires a separate paid license from the copyright holder. Third-party dependencies retain their own licenses.
