//! A local, versioned documentation and source cache. Search never uses the network.
macro_rules! println { ($($arg:tt)*) => { crate::output::text(format_args!("{}\n", format_args!($($arg)*))) }; }
mod cache;
mod chunker;
mod code_search;
mod config;
mod deps;
mod diagnostics;
mod output;
mod packages;
mod registry;
mod selection;
mod store;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use config::{Config, Source, SourceKind};
use serde::Serialize;
use std::{
    fs,
    num::NonZeroUsize,
    path::{Path, PathBuf},
};
use store::{Hit, Store};

#[derive(Clone, Copy, ValueEnum)]
enum OutputFormat {
    Human,
    Json,
}

#[derive(Parser)]
#[command(
    version = build_version(),
    about = "Search versioned documentation and source offline",
    after_help = "Examples:\n  rtfm search 'dependency injection' --library fastapi\n  rtfm deps . --json\n  rtfm init --source sources.toml\n  rtfm update fastapi\n  rtfm versions fastapi\n  rtfm search-code 'def solve_dependencies' --library fastapi\n\nLegacy --json keeps existing payloads and text errors. --output json returns a versioned envelope, including errors. Search and inspection are offline; discover, registry add and update may use the network."
)]
struct Cli {
    /// Versioned machine envelope; legacy command --json payloads stay unchanged.
    #[arg(long, global = true, value_enum, default_value = "human")]
    output: OutputFormat,
    /// Configuration file; otherwise nearest .rtfm.toml or user config.
    #[arg(long, global = true, env = "RTFM_CONFIG")]
    config: Option<PathBuf>,
    /// Cache directory containing immutable snapshots and index.sqlite3.
    #[arg(long, global = true, env = "RTFM_CACHE_DIR")]
    cache_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Create configuration, optionally from a source catalog. Does not fetch.
    #[command(
        after_help = "Example: rtfm init --source sources.toml\nWithout a catalog, creates an empty config. Add a source next; init never fetches."
    )]
    Init {
        /// Import an existing TOML source catalog.
        #[arg(long)]
        source: Option<PathBuf>,
    },
    /// Add a source definition. Fetch it with `update NAME`.
    #[command(
        after_help = "Examples:\n  rtfm add pypi:pydantic@2.14.1 --project\n  rtfm add custom ./checkout --kind local --package npm:demo@1.0.0 --role code --code 'src/**' --project\nAdding writes configuration only. Run the printed update command to fetch."
    )]
    Add {
        /// Source name with LOCATION, or ecosystem:name@exact-version without LOCATION.
        name: String,
        /// Repository URL, archive URL, documentation URL, or local path for a custom source.
        location: Option<String>,
        /// Acquisition kind for explicit NAME LOCATION; package form queries its registry.
        #[arg(long, value_enum, default_value = "git")]
        kind: SourceKind,
        /// Git ref or local label; repeat to retain multiple refs.
        #[arg(long = "ref", default_value = "HEAD")]
        refs: Vec<String>,
        #[arg(long)]
        /// Relative documentation globs; repeat; defaults to Markdown/MDX paths.
        docs: Vec<String>,
        #[arg(long)]
        /// Relative code globs; repeat; custom sources default to no code paths.
        code: Vec<String>,
        #[arg(long)]
        /// Save to the current directory .rtfm.toml without copying inherited settings.
        project: bool,
        /// Bind this source to an ecosystem:name@exact-version.
        #[arg(long)]
        package: Option<String>,
        #[arg(long, value_parser = ["docs", "code", "both"])]
        /// Which package coverage binding to write (default both); custom sources require --package.
        role: Option<String>,
        /// Relative glob exclusions; repeat for multiple patterns.
        #[arg(long)]
        exclude: Vec<String>,
        #[arg(long)]
        /// Archive digest: sha256:HEX or sha512-BASE64.
        checksum: Option<String>,
    },
    /// Look up exact package metadata and candidate sources (network, no fetching).
    #[command(
        after_help = "Examples:\n  rtfm discover pypi:pydantic@2.14.1 --json\n  rtfm discover npm:@nuxt/ui@4.11.3 --json\n  rtfm discover crates.io:tokio@1.53.1 --json\nJSON includes suggested paths and next_actions argv; repository URLs do not prove release correspondence."
    )]
    Discover {
        /// Exact package: pypi:name@version, npm:@scope/name@version, or crates.io:name@version.
        package: String,
        /// Preserve the legacy JSON payload format.
        #[arg(long)]
        json: bool,
    },
    /// Report direct dependency docs/code coverage offline; excludes experiments and build trees.
    #[command(after_help = "Example: rtfm deps . --package pypi:fastapi --json")]
    Deps {
        /// Project root to inventory recursively, offline.
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Restrict to ecosystem:name or ecosystem:name@exact-version (including scoped npm).
        #[arg(long)]
        package: Option<String>,
        /// Preserve the legacy JSON payload format.
        #[arg(long)]
        json: bool,
    },
    /// List configured sources and cached versions.
    #[command(after_help = "Example: rtfm sources --json")]
    Sources {
        /// Preserve the legacy JSON payload format.
        #[arg(long)]
        json: bool,
    },
    /// Fetch enabled configured refs; index documentation and retain complete source trees.
    #[command(
        after_help = "Examples: rtfm update fastapi; rtfm update --if-due\nNAME targets one enabled source; omission targets all enabled sources. Due mode skips never-cached refs."
    )]
    Update {
        /// Configured source name; omit for all enabled sources.
        name: Option<String>,
        /// Refresh only previously cached refs after the configured interval.
        #[arg(long)]
        if_due: bool,
    },
    /// Refresh already cached sources when the configured check interval has elapsed.
    #[command(hide = true)]
    UpdateIfDue,
    /// Show all retained versions, including immutable revision IDs.
    #[command(after_help = "Example: rtfm versions fastapi --json")]
    Versions {
        /// Source name; omit to list every retained source.
        name: Option<String>,
        /// Preserve the legacy JSON payload format.
        #[arg(long)]
        json: bool,
    },
    /// Select a default cached/configured ref for a source.
    #[command(
        after_help = "Example: rtfm use fastapi --ref tag:0.115.0 --project\nWrites the effective config by default; --project writes the current directory overlay. Fetching remains explicit."
    )]
    Use {
        /// Configured enabled source name (not a package identity).
        name: String,
        /// Fetchable ref label or an exact retained revision from versions.
        #[arg(long = "ref")]
        reference: String,
        /// Write the current directory .rtfm.toml; conflicts with explicit config.
        #[arg(long)]
        project: bool,
    },
    /// Print a cached snapshot directory for inspection with ordinary tools.
    #[command(after_help = "Example: rtfm path fastapi --version tag:0.115.0")]
    Path {
        /// Configured enabled source name.
        name: String,
        /// Ref label or exact immutable revision from `versions`.
        #[arg(long)]
        version: Option<String>,
    },
    /// Search documentation with BM25; literal query words are OR alternatives.
    #[command(after_help = "Example: rtfm search lifespan --library fastapi --top-k 5")]
    Search {
        /// Literal words to find; FTS operators and punctuation are not query syntax.
        query: String,
        #[arg(long, hide = true)]
        code: bool,
        /// Configured source name; omit for enabled selected sources.
        #[arg(long, alias = "lib")]
        library: Option<String>,
        /// Ref label or exact immutable revision from `versions`.
        #[arg(long)]
        version: Option<String>,
        /// Maximum returned results (1–100); default 5.
        #[arg(short = 'n', long = "limit", alias = "top-k", default_value = "5", value_parser = result_limit)]
        limit: NonZeroUsize,
        /// Preserve the legacy JSON payload format.
        #[arg(long)]
        json: bool,
        /// Explain coverage and empty results; with --json return {results, diagnostics}.
        #[arg(long)]
        diagnostics: bool,
    },
    /// Search configured source files offline using literal text or a regular expression.
    #[command(
        after_help = "Example: rtfm search-code 'def model_validate' --library pydantic --path 'pydantic/**/*.py'"
    )]
    SearchCode {
        /// Case-sensitive literal text, or per-line regex with --regex.
        query: String,
        /// Configured source name; omit for enabled selected sources.
        #[arg(long, alias = "lib")]
        library: Option<String>,
        /// Ref label or exact immutable revision from `versions`.
        #[arg(long)]
        version: Option<String>,
        /// Restrict relative file paths; repeat for multiple glob patterns.
        #[arg(long = "path")]
        paths: Vec<String>,
        /// Interpret the query as a per-line regular expression.
        #[arg(long)]
        regex: bool,
        /// Surrounding lines per match (maximum 20).
        #[arg(short = 'C', long, default_value = "2", value_parser = clap::value_parser!(u8).range(0..=20))]
        context: u8,
        /// Maximum returned results (1–100); default 5.
        #[arg(short = 'n', long = "limit", alias = "top-k", default_value = "5", value_parser = result_limit)]
        limit: NonZeroUsize,
        /// Preserve the legacy JSON payload format.
        #[arg(long)]
        json: bool,
        /// Explain coverage and empty results; with --json return {results, diagnostics}.
        #[arg(long)]
        diagnostics: bool,
    },
    /// Print a complete cached search chunk with its provenance.
    #[command(
        after_help = "Example: rtfm show 1 --json (replace 1 with an ID returned by search)"
    )]
    Show {
        /// Cache-local documentation result ID from search.
        id: i64,
        /// Preserve the legacy JSON payload format.
        #[arg(long)]
        json: bool,
    },
    /// Show local cache counts without network access.
    #[command(after_help = "Example: rtfm status --json")]
    Status {
        /// Preserve the legacy JSON payload format.
        #[arg(long)]
        json: bool,
    },
}
fn result_limit(value: &str) -> std::result::Result<NonZeroUsize, String> {
    let n: usize = value.parse().map_err(|_| "expected 1..100".to_string())?;
    if (1..=100).contains(&n) {
        Ok(NonZeroUsize::new(n).unwrap())
    } else {
        Err("expected 1..100".into())
    }
}
fn build_version() -> &'static str {
    static VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    VERSION.get_or_init(|| {
        format!(
            "{} ({})",
            env!("CARGO_PKG_VERSION"),
            option_env!("RTFM_BUILD_REVISION").unwrap_or("development")
        )
    })
}
fn main() {
    if let Err(error) = cache::install_signal_handlers() {
        eprintln!("rtfm: {error:#}");
        std::process::exit(1);
    }
    let args: Vec<_> = std::env::args_os().collect();
    let option_args = &args[..args
        .iter()
        .position(|arg| arg == "--")
        .unwrap_or(args.len())];
    let machine = option_args
        .windows(2)
        .any(|a| a[0] == "--output" && a[1] == "json")
        || option_args.iter().any(|a| a == "--output=json");
    output::configure(machine, serde_json::json!({}));
    if args.len() == 1 {
        use clap::CommandFactory;
        let mut help = Vec::new();
        let _ = Cli::command().write_help(&mut help);
        output::write(format_args!("{}\n", String::from_utf8_lossy(&help)));
        std::process::exit(output::finish(None, false));
    }
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(error)
            if matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) =>
        {
            output::configure(false, serde_json::json!({}));
            output::write(format_args!("{error}"));
            std::process::exit(output::finish(None, false));
        }
        Err(error) => std::process::exit(output::finish(Some(&anyhow::Error::new(error)), true)),
    };
    let result = run(cli);
    let result = cache::check_interrupted().and(result);
    std::process::exit(output::finish(result.as_ref().err(), false));
}
fn run(cli: Cli) -> Result<()> {
    let explicit_config = cli.config.is_some();
    let config_path = absolute(cli.config.unwrap_or_else(config::config_path))?;
    let cache_root = absolute(cli.cache_dir.unwrap_or_else(config::cache_path))?;
    output::configure(
        matches!(cli.output, OutputFormat::Json),
        serde_json::json!({"config":config_path,"cache_dir":cache_root,"cwd":std::env::current_dir()?}),
    );
    if explicit_config
        && matches!(
            &cli.command,
            Command::Add { project: true, .. } | Command::Use { project: true, .. }
        )
    {
        return Err(output::Failure::new(
            "scope_conflict",
            "--project conflicts with --config/RTFM_CONFIG; choose one destination",
            serde_json::json!({"config":config_path}),
        )
        .into());
    }
    match cli.command {
        Command::Init { source } => {
            let _lock = config::ConfigLock::acquire(&config_path)?;
            if config_path.exists() {
                bail!(
                    "{} already exists; edit it or choose --config",
                    config_path.display()
                );
            }
            let config = source
                .as_deref()
                .map(Config::load)
                .transpose()?
                .unwrap_or_default();
            config.save(&config_path)?;
            output::data(
                serde_json::json!({"destination":config_path,"changed":true,"sources":config.sources.len()}),
            );
            println!(
                "Created {} ({} sources). Add sources with `rtfm add NAME LOCATION`, or update a configured source.",
                config_path.display(),
                config.sources.len()
            );
        }
        Command::Discover { package, json } => {
            let mut discovery = registry::discover(&package)?;
            for action in &mut discovery.next_actions {
                let args: Vec<_> = action
                    .argv
                    .iter()
                    .skip(1)
                    .filter(|a| a.as_str() != "--project")
                    .map(String::as_str)
                    .collect();
                action.argv = contextual(&config_path, &cache_root, &args);
                output::action(action.argv.iter().skip(5).cloned().collect());
            }
            if json || output::machine() {
                print_json(&discovery)?
            } else {
                println!(
                    "{}:{}@{}",
                    discovery.package.ecosystem, discovery.package.name, discovery.package.version
                );
                for candidate in discovery.candidates {
                    println!("{}: {}", candidate.kind, candidate.url);
                    for evidence in candidate.evidence {
                        println!("  {evidence}");
                    }
                }
                println!("source: {}", discovery.source_name);
                for action in discovery.next_actions {
                    println!("{}", shell_command(&action.argv));
                }
                for evidence in discovery.evidence {
                    println!("{evidence}");
                }
            }
        }
        Command::Deps {
            path,
            package,
            json,
        } => {
            if let Some(filter) = &package {
                packages::validate_filter(filter)?;
            }
            let path = fs::canonicalize(&path).map_err(|error| output::Failure::new("io_failed",format!("locate dependency inventory root {}: {error}; provide an existing project directory",path.display()),serde_json::json!({"path":path,"kind":format!("{:?}",error.kind())})))?;
            let target = if !explicit_config {
                project_config_from(&path).unwrap_or(config_path.clone())
            } else {
                config_path.clone()
            };
            output::configure(
                output::machine(),
                serde_json::json!({"config":target,"cache_dir":cache_root,"cwd":std::env::current_dir()?}),
            );
            let config = if target.exists() {
                Config::load(&target)?
            } else {
                Config::default()
            };
            let store = Store::open(&cache_root)?;
            let inventory = deps::inventory(&path)?;
            let report =
                packages::coverage(&inventory.dependencies, &config, &store, package.as_deref())?;
            if output::machine() {
                print_json(
                    &serde_json::json!({"root":inventory.root,"manifests":inventory.manifests,"exclusions":inventory.exclusions,"results":report}),
                )?;
            } else if json {
                print_json(&report)?
            } else {
                for row in report {
                    println!(
                        "{}@{}  docs={} code={}  {}",
                        row.package,
                        row.version.as_deref().unwrap_or("unresolved"),
                        row.docs.status,
                        row.code.status,
                        row.manifest.display()
                    );
                    for candidate in &row.possible_sources {
                        println!(
                            "  possible unbound source: {} @ {} (release correspondence unproven)",
                            candidate.source, candidate.reference
                        );
                    }
                    if row.version.is_none() {
                        println!("  {}", row.evidence);
                    }
                    for (kind, part) in [("docs", &row.docs), ("code", &row.code)] {
                        if let Some(evidence) = &part.evidence {
                            println!(
                                "  {kind}: {} @ {} ({evidence})",
                                part.source.as_deref().unwrap_or("?"),
                                part.reference.as_deref().unwrap_or("?")
                            );
                        }
                    }
                    if let Some(command) = row.discover {
                        if row.docs.status != "cached" || row.code.status != "cached" {
                            println!("  {command}");
                        }
                    }
                }
            }
        }
        Command::Add {
            name,
            location,
            kind,
            refs,
            docs,
            code,
            project,
            package,
            role,
            exclude,
            checksum,
        } => {
            if location.is_some() && role.is_some() && package.is_none() {
                return Err(output::Failure::new(
                    "argument_invalid",
                    "--role requires --package for a custom source",
                    serde_json::json!({"argument":"--role"}),
                )
                .into());
            }
            let role = role.as_deref().unwrap_or("both");
            let target = if project {
                std::env::current_dir()?.join(".rtfm.toml")
            } else {
                config_path.clone()
            };
            let _lock = config::ConfigLock::acquire(&target)?;
            // Save only the target overlay, never materialize inherited user configuration.
            let mut config = if target.exists() {
                Config::load_raw(&target)?
            } else {
                Config::default()
            };
            let (name, mut source, spec, evidence) = if let Some(location) = location {
                config::validate_name(&name)?;
                let spec = package
                    .as_deref()
                    .map(registry::PackageSpec::parse)
                    .transpose()?;
                (
                    name,
                    Source {
                        kind,
                        location,
                        checksum,
                        refs,
                        docs: if docs.is_empty() {
                            config::default_docs()
                        } else {
                            docs
                        },
                        code,
                        exclude,
                        enabled: true,
                    },
                    spec,
                    "declared project mapping; release correspondence requires verification"
                        .to_string(),
                )
            } else {
                if kind != SourceKind::Git || refs != ["HEAD"] {
                    bail!("registry artifacts use their published version; omit --kind and --ref, or provide NAME LOCATION for a custom source")
                }
                if package.is_some() {
                    bail!("omit --package when adding an ecosystem-qualified package directly")
                }
                let discovery = registry::discover(&name)?;
                let artifact=discovery.candidates.iter().find(|c|c.kind=="artifact").context("no published source artifact; use discover candidates and add NAME LOCATION --package SPEC with explicit paths/ref")?;
                let source_name = registry::source_name(&discovery.package);
                (
                    source_name,
                    Source {
                        kind: SourceKind::Archive,
                        location: artifact.url.clone(),
                        checksum: checksum.or(artifact.checksum.clone()),
                        refs: vec![discovery.package.version.clone()],
                        docs: if docs.is_empty() {
                            artifact.docs_paths.clone()
                        } else {
                            docs
                        },
                        code: if code.is_empty() {
                            artifact.code_paths.clone()
                        } else {
                            code
                        },
                        exclude,
                        enabled: true,
                    },
                    Some(discovery.package),
                    format!("registry artifact: {}", artifact.evidence.join("; ")),
                )
            };
            if config.sources.contains_key(&name) {
                return Err(output::Failure::new(
                    "source_exists",
                    format!("source {name} already exists; edit {}", target.display()),
                    serde_json::json!({"source":name,"destination":target}),
                )
                .into());
            }
            if source.kind == SourceKind::Local
                || (matches!(source.kind, SourceKind::Git | SourceKind::Archive)
                    && Path::new(&source.location).exists())
            {
                source.location = fs::canonicalize(&source.location)?
                    .to_string_lossy()
                    .into_owned();
            }
            if let Some(spec) = spec {
                let key = format!("{}:{}@{}", spec.ecosystem, spec.name, spec.version);
                let binding = config::Binding {
                    source: name.clone(),
                    reference: source.refs[0].clone(),
                    evidence,
                };
                let mapping = config.packages.entry(key).or_default();
                if role != "code" {
                    mapping.docs = Some(binding.clone())
                }
                if role != "docs" {
                    mapping.code = Some(binding)
                }
            }
            config.sources.insert(name.clone(), source);
            config.save(&target)?;
            output::data(serde_json::json!({"destination":target,"source":name,"changed":true}));
            let action = contextual(&target, &cache_root, &["update", &name]);
            output::action_at(&target, &cache_root, vec!["update".into(), name.clone()]);
            println!(
                "Added {name} to {}. Run {} to fetch and index it.",
                target.display(),
                shell_command(&action)
            );
        }
        Command::Update { name, if_due } => {
            update(&config_path, &cache_root, name.as_deref(), if_due)?
        }
        Command::UpdateIfDue => update(&config_path, &cache_root, None, true)?,
        Command::Use {
            name,
            reference,
            project,
        } => {
            let target = if project {
                std::env::current_dir()?.join(".rtfm.toml")
            } else {
                config_path.clone()
            };
            let _lock = config::ConfigLock::acquire(&target)?;
            let effective = Config::load(&config_path)?;
            let mut source = effective.source(&name)?.clone();
            let immutable = Store::open(&cache_root)?
                .versions(Some(&name))?
                .iter()
                .any(|v| v.revision == reference);
            let mut config = if target.exists() {
                Config::load_raw(&target)?
            } else {
                Config::default()
            };
            let destination_effective = if target.exists() {
                Config::load(&target)?
            } else {
                Config::default().effective(&target)?
            };
            let copy_source = destination_effective
                .sources
                .get(&name)
                .map(serde_json::to_value)
                .transpose()?
                != Some(serde_json::to_value(&source)?);
            let add_ref = !immutable && !source.refs.contains(&reference);
            let changed = destination_effective.selections.get(&name) != Some(&reference)
                || copy_source
                || add_ref;
            if add_ref {
                source.refs.push(reference.clone());
            }
            if add_ref || copy_source {
                config.sources.insert(name.clone(), source);
            }
            config.selections.insert(name.clone(), reference.clone());
            config.save(&target)?;
            output::data(
                serde_json::json!({"destination":target,"source":name,"reference":reference,"changed":changed}),
            );
            output::action_at(&target, &cache_root, vec!["update".into(), name.clone()]);
            println!("{name}: selected {reference} in {}", target.display());
        }
        command => {
            let store = Store::open(&cache_root)?;
            match command {
                Command::Status { json } => {
                    let (libraries, versions, chunks) = store.counts()?;
                    if json || output::machine() {
                        print_json(
                            &serde_json::json!({"libraries":libraries,"versions":versions,"chunks":chunks,"cache_dir":cache_root,"config":config_path}),
                        )?;
                    } else {
                        println!("{libraries} libraries, {versions} cached versions, {chunks} chunks\nCache: {}",cache_root.display());
                    }
                }
                Command::Versions { name, json } => {
                    let config = if config_path.exists() {
                        Config::load(&config_path)?
                    } else {
                        Config::default()
                    };
                    if let Some(name) = &name {
                        config.source(name)?;
                    }
                    let versions = store.version_summaries(name.as_deref())?;
                    let selected = |v: &store::Version| {
                        config
                            .sources
                            .get(&v.library)
                            .filter(|s| s.enabled)
                            .is_some_and(|s| {
                                let reference =
                                    config.selections.get(&v.library).unwrap_or(&s.refs[0]);
                                v.revision == *reference || (v.version == *reference && v.current)
                            })
                    };
                    if json || output::machine() {
                        let rows: Vec<serde_json::Value> = versions
                            .iter()
                            .map(|v| {
                                let mut row = serde_json::to_value(v)?;
                                row["selected"] = serde_json::json!(selected(&v.snapshot));
                                Ok(row)
                            })
                            .collect::<Result<_>>()?;
                        print_json(&rows)?;
                    } else if versions.is_empty() {
                        println!("No cached versions. Run `rtfm update NAME`.");
                    } else {
                        for summary in &versions {
                            let v = &summary.snapshot;
                            println!(
                                "{}\t{}\t{}\t{}{}\t{} chunks",
                                v.library,
                                v.version,
                                v.revision,
                                if v.current { "current" } else { "retained" },
                                if selected(v) { ", selected" } else { "" },
                                summary.chunks
                            );
                        }
                    }
                }
                Command::Sources { json } => {
                    let config = Config::load(&config_path)?;
                    let versions = store.versions(None)?;
                    let origins = Config::source_origins(&config_path);
                    if json || output::machine() {
                        let rows: Vec<_> = config.sources.iter().map(|(name,s)| serde_json::json!({"name":name,"kind":s.kind,"location":s.location,"refs":s.refs,"docs":s.docs,"code":s.code,"exclude":s.exclude,"selected":config.selections.get(name).unwrap_or(&s.refs[0]),"config_origin":origins.as_ref().ok().map(|origin| origin(name)),"enabled":s.enabled,"cached_versions":versions.iter().filter(|v|v.library==*name).count()})).collect();
                        print_json(&rows)?;
                    } else {
                        let origin = origins?;
                        for (name, source) in &config.sources {
                            let selected = config.selections.get(name).unwrap_or(&source.refs[0]);
                            let origin = origin(name);
                            println!("{name}\t{:?}\t{}\t{}\t{} cached\n  selected: {selected}\n  config: {}\n  location: {}\n  docs: {:?}\n  code: {:?}\n  exclude: {:?}",source.kind,source.refs.join(","),if source.enabled { "enabled" } else { "disabled" },versions.iter().filter(|v|v.library==*name).count(),origin.display(),source.location,source.docs,source.code,source.exclude);
                        }
                    }
                }
                Command::Path { name, version } => {
                    let config = Config::load(&config_path)?;
                    let selected =
                        selection::resolve(&config, &store, Some(&name), version.as_deref())?;
                    let found = &selected[0];
                    if output::machine() {
                        print_json(
                            &serde_json::json!({"path":found.root,"source":name,"revision":found.revision}),
                        )?;
                    } else {
                        println!("{}", found.root.display());
                    }
                }
                Command::Search {
                    query,
                    code,
                    library,
                    version,
                    limit,
                    json,
                    diagnostics,
                } => {
                    if code {
                        bail!("source search moved to `rtfm search-code QUERY`; remove --code for documentation search");
                    }
                    let config = Config::load(&config_path)?;
                    let selected = selection::resolve(
                        &config,
                        &store,
                        library.as_deref(),
                        version.as_deref(),
                    )?;
                    let mut hits = store.search_selected(&query, &selected, limit.get() + 1)?;
                    let has_more = hits.len() > limit.get();
                    hits.truncate(limit.get());
                    let report = if diagnostics || (!json && hits.is_empty()) {
                        Some(diagnostics::inspect(
                            &store,
                            &config,
                            library.as_deref(),
                            version.as_deref(),
                            "docs",
                            !hits.is_empty(),
                            &[],
                        )?)
                    } else {
                        None
                    };
                    if !json && diagnostics {
                        if let Some(r) = &report {
                            println!(
                                "{}: {} docs chunks; code available={}. {}",
                                r.status, r.documentation_chunks, r.code_available, r.suggestion
                            );
                        }
                    }
                    if json || output::machine() {
                        if output::machine() {
                            print_json(
                                &serde_json::json!({"results":hits,"diagnostics":report,"has_more":has_more}),
                            )?
                        } else if diagnostics {
                            print_json(&serde_json::json!({"results":hits,"diagnostics":report}))?
                        } else {
                            print_json(&hits)?
                        }
                    } else if hits.is_empty() {
                        println!("{}", report.as_ref().unwrap().suggestion);
                    } else {
                        for hit in hits {
                            print_hit(&hit);
                        }
                    }
                }
                Command::SearchCode {
                    query,
                    library,
                    version,
                    paths,
                    regex,
                    context,
                    limit,
                    json,
                    diagnostics,
                } => {
                    let config = Config::load(&config_path)?;
                    let mut hits = code_search::search(
                        &config,
                        selection::resolve(
                            &config,
                            &store,
                            library.as_deref(),
                            version.as_deref(),
                        )?,
                        code_search::Options {
                            query: &query,
                            library: library.as_deref(),
                            version: version.as_deref(),
                            paths: &paths,
                            regex,
                            context: context as usize,
                            limit: limit.get() + 1,
                        },
                    )?;
                    let has_more = hits.len() > limit.get();
                    hits.truncate(limit.get());
                    let report = if diagnostics || (!json && hits.is_empty()) {
                        Some(diagnostics::inspect(
                            &store,
                            &config,
                            library.as_deref(),
                            version.as_deref(),
                            "code",
                            !hits.is_empty(),
                            &paths,
                        )?)
                    } else {
                        None
                    };
                    if !json && diagnostics {
                        if let Some(r) = &report {
                            println!(
                                "{}: {} docs chunks; code available={}. {}",
                                r.status, r.documentation_chunks, r.code_available, r.suggestion
                            );
                        }
                    }
                    if json || output::machine() {
                        if output::machine() {
                            print_json(
                                &serde_json::json!({"results":hits,"diagnostics":report,"has_more":has_more}),
                            )?
                        } else if diagnostics {
                            print_json(&serde_json::json!({"results":hits,"diagnostics":report}))?
                        } else {
                            print_json(&hits)?
                        }
                    } else if hits.is_empty() {
                        println!("{}", report.as_ref().unwrap().suggestion);
                    } else {
                        for hit in hits {
                            println!(
                                "{} {} {}:{}-{}\nrevision: {}\nsource: {}\nlocal: {}\n{}",
                                hit.library,
                                hit.version,
                                hit.path,
                                hit.start_line,
                                hit.end_line,
                                hit.revision,
                                hit.provenance,
                                hit.local_path.display(),
                                numbered(&hit.text, hit.start_line)
                            );
                            if hit.text_truncated {
                                println!("[partial source lines; UTF-8 byte ranges are zero-based, end-exclusive]");
                                for span in &hit.excerpt_spans {
                                    println!(
                                        "  line {}: bytes {}..{}",
                                        span.line, span.start_byte, span.end_byte
                                    );
                                }
                                println!(
                                    "[inspect {} for complete lines]",
                                    hit.local_path.display()
                                );
                            }
                        }
                    }
                }
                Command::Show { id, json } => {
                    let hit = store.show(id)?;
                    if json || output::machine() {
                        let mut value = serde_json::to_value(&hit)?;
                        value.as_object_mut().unwrap().remove("score");
                        print_json(&value)?;
                    } else {
                        print_hit(&hit);
                    }
                }
                _ => unreachable!(),
            }
        }
    }
    Ok(())
}
fn update(config_path: &Path, cache_root: &Path, name: Option<&str>, due: bool) -> Result<()> {
    if due && !config_path.is_file() {
        output::data(
            serde_json::json!({"updated":[],"failures":[],"skipped":"configuration_missing","changed":false}),
        );
        return Ok(());
    }
    let config = Config::load(config_path)?;
    fs::create_dir_all(cache_root)?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(cache_root.join("update.lock"))?;
    match fs2::FileExt::try_lock_exclusive(&lock) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
            if due {
                output::data(
                    serde_json::json!({"updated":[],"failures":[],"skipped":"update_locked","changed":false}),
                );
                return Ok(());
            }
            return Err(output::Failure::new(
                "update_locked",
                "another update is already running; search remains available offline",
                serde_json::json!({}),
            )
            .into());
        }
        Err(error) => return Err(error).context("lock cache for update"),
    }
    if let Some(name) = name {
        config.source(name)?;
    }
    let mut store = Store::open(cache_root)?;
    let mut failures = Vec::new();
    let mut updated = 0;
    let mut outcomes = Vec::new();
    let mut skipped = Vec::new();
    for (library, source) in &config.sources {
        if !source.enabled || name.is_some_and(|name| name != library) {
            continue;
        }
        let mut references = source.refs.clone();
        if let Some(reference) = config.selections.get(library) {
            if !references.contains(reference) {
                references.push(reference.clone());
            }
        }
        for reference in &references {
            let previous = store.last_update(library, reference)?;
            let checked = store.last_check(library, reference)?.or(previous);
            if due
                && (previous.is_none()
                    || checked.is_some_and(|last| {
                        store::now().saturating_sub(last) < config.update_interval_seconds()
                    }))
            {
                skipped.push(serde_json::json!({"source":library,"reference":reference,"reason":"not_due_or_never_cached"}));
                continue;
            }
            store.record_check(library, reference)?;
            eprintln!("Updating {library} {reference}...");
            let result = (|| -> Result<(usize, bool)> {
                let retained = store
                    .versions(Some(library))?
                    .into_iter()
                    .find(|v| v.revision == *reference);
                let snapshot = if let Some(version) = retained {
                    let snapshot = cache::Snapshot {
                        revision: version.revision,
                        root: version.root,
                        provenance: version.provenance,
                    };
                    cache::verify(&snapshot)?;
                    snapshot
                } else {
                    cache::fetch(source, &cache_root.join("sources").join(library), reference)?
                };
                let publication_changed = !store.versions(Some(library))?.iter().any(|v| {
                    v.version == *reference && v.current && v.revision == snapshot.revision
                });
                use sha2::{Digest, Sha256};
                let signature = format!(
                    "{:x}",
                    Sha256::digest(serde_json::to_vec(&(3, &source.docs, &source.exclude))?)
                );
                if store.needs_index(library, &snapshot.revision, &signature)? {
                    let result = chunker::read(
                        &snapshot.root,
                        source,
                        store.cached_files(library, &signature)?,
                    )?;
                    let count = result.chunks.len();
                    eprintln!("Reused {} unchanged file(s)", result.reused_files);
                    store.index(
                        library,
                        reference,
                        &snapshot,
                        &result.chunks,
                        &result.files,
                        &signature,
                    )?;
                    Ok((count, true))
                } else {
                    store.index(library, reference, &snapshot, &[], &[], &signature)?;
                    Ok((
                        store.chunk_count(library, &snapshot.revision)?,
                        publication_changed,
                    ))
                }
            })();
            // I/O can fail before an acquisition's next cancellation checkpoint.
            let result = result.map_err(|error| cache::check_interrupted().err().unwrap_or(error));
            match result {
                Ok((count, changed)) => {
                    updated += 1;
                    outcomes.push(
                        serde_json::json!({"source":library,"reference":reference,"chunks":count,"changed":changed}),
                    );
                    eprintln!("Indexed {library} {reference}: {count} chunks");
                }
                Err(error) => {
                    if error.downcast_ref::<cache::Interrupted>().is_some() {
                        failures.push(serde_json::json!({"source":library,"reference":reference,"code":"interrupted","message":format!("{error:#}")}));
                        output::data(
                            serde_json::json!({"destination":config_path,"updated":outcomes,"failures":failures,"skipped":skipped,"changed":outcomes.iter().any(|o|o["changed"]==true)}),
                        );
                        output::action(vec!["update".into(), library.clone()]);
                        return Err(error);
                    }
                    eprintln!("Failed {library} {reference}: {error:#}");
                    let code = if error.downcast_ref::<cache::ProcessTimeout>().is_some()
                        || error
                            .downcast_ref::<reqwest::Error>()
                            .is_some_and(reqwest::Error::is_timeout)
                    {
                        "timeout"
                    } else if error.downcast_ref::<std::io::Error>().is_some() {
                        "io_failed"
                    } else {
                        "acquisition_failed"
                    };
                    failures.push(serde_json::json!({"source":library,"reference":reference,"code":code,"message":format!("{error:#}")}));
                    output::action(vec!["update".into(), library.clone()]);
                }
            }
        }
    }
    output::data(
        serde_json::json!({"destination":config_path,"updated":outcomes,"failures":failures,"skipped":skipped,"changed":outcomes.iter().any(|o|o["changed"]==true)}),
    );
    if !failures.is_empty() {
        return Err(output::Failure::new(
            "acquisition_failed",
            format!(
                "{} version(s) failed; previous cache retained",
                failures.len()
            ),
            serde_json::json!({"failures":failures}),
        )
        .into());
    }
    if !due {
        println!("Updated {updated} version(s).");
    }
    Ok(())
}
fn print_json(value: &impl Serialize) -> Result<()> {
    if output::machine() {
        output::data(serde_json::to_value(value)?);
    } else {
        println!("{}", serde_json::to_string_pretty(value)?);
    }
    Ok(())
}
fn print_hit(hit: &Hit) {
    println!(
        "[{}] {} {} {} {}:{}-{}\nrevision: {}\nsource: {}\nlocal: {}\n{}",
        hit.id,
        hit.library,
        hit.version,
        hit.kind,
        hit.path,
        hit.start_line,
        hit.end_line,
        hit.revision,
        hit.provenance,
        hit.local_path.display(),
        hit.text
    );
    if hit.heading_truncated {
        println!("[heading truncated]");
    }
}

fn absolute(path: PathBuf) -> Result<PathBuf> {
    Ok(if path.is_absolute() {
        path
    } else {
        std::env::current_dir()?.join(path)
    })
}

fn contextual(config: &Path, cache: &Path, args: &[&str]) -> Vec<String> {
    let mut argv = vec![
        "rtfm".into(),
        "--config".into(),
        config.display().to_string(),
        "--cache-dir".into(),
        cache.display().to_string(),
    ];
    argv.extend(args.iter().map(|s| s.to_string()));
    argv
}
fn project_config_from(root: &Path) -> Option<PathBuf> {
    root.ancestors()
        .map(|p| p.join(".rtfm.toml"))
        .find(|p| p.is_file())
}

fn numbered(text: &str, start: usize) -> String {
    text.lines()
        .enumerate()
        .map(|(i, line)| format!("{}: {}", start + i, line))
        .collect::<Vec<_>>()
        .join("\n")
}

// Quote argv for a POSIX shell; JSON keeps the original argument boundaries.
fn shell_command(argv: &[String]) -> String {
    argv.iter()
        .map(|arg| {
            if !arg.is_empty()
                && arg
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&c))
            {
                arg.clone()
            } else {
                format!("'{}'", arg.replace('\'', "'\"'\"'"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod cli_tests {
    use super::shell_command;

    #[test]
    fn next_action_shell_quoting_preserves_argument_boundaries() {
        let argv = [
            "rtfm",
            "add",
            "npm:@scope/name@1.2.3",
            "--project",
            "",
            "a'b",
            "$(touch file)",
        ];
        assert_eq!(
            shell_command(&argv.map(String::from)),
            "rtfm add npm:@scope/name@1.2.3 --project '' 'a'\"'\"'b' '$(touch file)'"
        );
    }
}
