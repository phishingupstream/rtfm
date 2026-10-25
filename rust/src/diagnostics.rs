//! Opt-in offline search coverage diagnostics, separate from ranked results.
use crate::{config::Config, store::Store};
use anyhow::Result;
use serde::Serialize;

#[derive(Serialize)]
pub struct Diagnostics {
    pub status: &'static str,
    pub cached_sources: usize,
    pub documentation_chunks: usize,
    pub code_available: bool,
    pub suggestion: &'static str,
}
pub fn inspect(
    store: &Store,
    config: &Config,
    library: Option<&str>,
    reference: Option<&str>,
    kind: &str,
    matched: bool,
    paths: &[String],
) -> Result<Diagnostics> {
    let versions = crate::selection::resolve(config, store, library, reference)?;
    let cached_sources = versions.len();
    let documentation_chunks = store.documentation_count(&versions)?;
    let mut code_available = false;
    for v in &versions {
        let name = &v.library;
        let source = config.source(name)?;
        if code_available {
            continue;
        }
        let build = |patterns: &[String]| -> Result<globset::GlobSet> {
            let mut b = globset::GlobSetBuilder::new();
            for p in patterns {
                b.add(globset::Glob::new(p)?);
            }
            Ok(b.build()?)
        };
        let included = build(&source.code)?;
        let excluded = build(&source.exclude)?;
        let restricted = build(paths)?;
        for e in walkdir::WalkDir::new(&v.root)
            .into_iter()
            .filter_entry(|e| e.file_name() != ".git")
        {
            let e = e?;
            if !e.file_type().is_file() {
                continue;
            }
            let path = e.path().strip_prefix(&v.root)?;
            if !included.is_match(path)
                || excluded.is_match(path)
                || (!paths.is_empty() && !restricted.is_match(path))
                || e.metadata()?.len() > 2 * 1024 * 1024
            {
                continue;
            }
            let bytes = std::fs::read(e.path())?;
            if !bytes.contains(&0) && std::str::from_utf8(&bytes).is_ok() {
                code_available = true;
                break;
            }
        }
    }
    let (status, suggestion) = if matched {
        ("matches", "Read the returned source paths and revisions.")
    } else if cached_sources == 0 {
        (
            "no_cached_sources",
            "Inspect sources and versions; explicitly update the required source.",
        )
    } else if kind == "docs" && documentation_chunks == 0 {
        (
            "no_documentation",
            if code_available {
                "Source files are available; try search-code or configure a documentation source."
            } else {
                "No indexed documentation in this scope; inspect selectors and update coverage."
            },
        )
    } else if kind == "code" && !code_available {
        ("no_source_files","No readable source files match the configured selectors and path filter; inspect paths and coverage.")
    } else {
        (
            "no_matches",
            "No matches in cached content; refine the query or path filter.",
        )
    };
    Ok(Diagnostics {
        status,
        cached_sources,
        documentation_chunks,
        code_available,
        suggestion,
    })
}
