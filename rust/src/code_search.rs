//! File-based source lookup over complete cached snapshots; never uses FTS or network.
use crate::{config::Config, store::Version};
use anyhow::{Context, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::Serialize;
use std::{collections::BTreeSet, fs, path::PathBuf};
use walkdir::WalkDir;

pub struct Options<'a> {
    pub query: &'a str,
    pub library: Option<&'a str>,
    pub version: Option<&'a str>,
    pub paths: &'a [String],
    pub regex: bool,
    pub context: usize,
    pub limit: usize,
}
#[derive(Serialize)]
pub struct ExcerptSpan {
    pub line: usize,
    pub start_byte: usize,
    pub end_byte: usize,
}
#[derive(Serialize)]
pub struct Hit {
    pub library: String,
    pub version: String,
    pub revision: String,
    pub provenance: String,
    pub path: String,
    pub local_path: PathBuf,
    pub start_line: usize,
    pub end_line: usize,
    pub match_lines: Vec<usize>,
    pub text: String,
    pub text_truncated: bool,
    pub excerpt_end_line: usize,
    pub excerpt_spans: Vec<ExcerptSpan>,
}
fn patterns(values: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for value in values {
        builder.add(Glob::new(value).with_context(|| format!("invalid path pattern {value}"))?);
    }
    Ok(builder.build()?)
}
pub fn search(config: &Config, versions: Vec<Version>, options: Options<'_>) -> Result<Vec<Hit>> {
    if options.limit == 0 {
        return Err(crate::output::Failure::new(
            "argument_invalid",
            "result limit must be greater than zero",
            serde_json::json!({"limit":options.limit}),
        )
        .into());
    }
    if options.context > 20 {
        return Err(crate::output::Failure::new(
            "argument_invalid",
            "source context cannot exceed 20 lines",
            serde_json::json!({"context":options.context}),
        )
        .into());
    }
    if options.query.is_empty() || options.query.contains(['\n', '\r']) {
        return Err(crate::output::Failure::new(
            "query_invalid",
            "source query must be nonempty and contain no newline; matching is line-oriented",
            serde_json::json!({}),
        )
        .into());
    }
    let expression = options
        .regex
        .then(|| regex::Regex::new(options.query))
        .transpose()
        .with_context(|| {
            crate::output::Failure::new(
                "query_invalid",
                "invalid source regular expression",
                serde_json::json!({}),
            )
        })?;
    let paths = patterns(options.paths)?;
    let selected =
        crate::selection::resolve_versions(config, versions, options.library, options.version)?;
    let mut hits = Vec::new();
    let mut seen = BTreeSet::new();
    for version in &selected {
        let source = &config.sources[&version.library];
        if !seen.insert((&version.library, &version.revision)) {
            continue;
        }
        let code = patterns(&source.code)?;
        let excluded = patterns(&source.exclude)?;
        for entry in WalkDir::new(&version.root)
            .sort_by_file_name()
            .into_iter()
            .filter_entry(|e| e.file_name() != ".git")
        {
            crate::cache::check_interrupted()?;
            let entry = entry
                .with_context(|| format!("inspect cached snapshot {}", version.root.display()))?;
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry
                .path()
                .strip_prefix(&version.root)?
                .to_string_lossy()
                .replace('\\', "/");
            if !code.is_match(&path)
                || excluded.is_match(&path)
                || (!options.paths.is_empty() && !paths.is_match(&path))
            {
                continue;
            }
            // Match the documented text-file boundary used by documentation indexing.
            if entry.metadata()?.len() > 2 * 1024 * 1024 {
                continue;
            }
            let bytes = fs::read(entry.path())
                .with_context(|| format!("read {}", entry.path().display()))?;
            if bytes.contains(&0) {
                continue;
            }
            let Ok(text) = String::from_utf8(bytes) else {
                continue;
            };
            if expression.is_none() && !text.contains(options.query) {
                continue;
            }
            let mut lines = Vec::new();
            let mut source_lines = text.lines();
            let mut ranges: Vec<(usize, usize, Vec<usize>)> = Vec::new();
            for i in 0.. {
                if i == lines.len() {
                    let Some(line) = source_lines.next() else {
                        break;
                    };
                    lines.push(line);
                }
                let line = lines[i];
                if !expression
                    .as_ref()
                    .map_or_else(|| line.contains(options.query), |re| re.is_match(line))
                {
                    continue;
                }
                // Look ahead far enough to clip context correctly at EOF.
                let needed = (i + options.context + 1).saturating_sub(lines.len());
                lines.extend(source_lines.by_ref().take(needed));
                let start = i.saturating_sub(options.context);
                let end = i.saturating_add(options.context).min(lines.len() - 1);
                if let Some(previous) = ranges
                    .last_mut()
                    .filter(|r| start <= r.1 + 1 && end - r.0 < 120)
                {
                    previous.1 = end;
                    previous.2.push(i + 1);
                } else {
                    if ranges.len() >= options.limit - hits.len() {
                        break;
                    }
                    ranges.push((start, end, vec![i + 1]));
                }
            }
            for (start, end, match_lines) in ranges {
                // Keep each source line recognizable even when a generated line is huge.
                // Bound the whole excerpt, retaining UTF-8 and exact line coordinates.
                let mut excerpt = String::new();
                let mut text_truncated = false;
                let mut excerpt_spans = Vec::new();
                for (offset, line) in lines[start..=end].iter().enumerate() {
                    if offset > 0 {
                        excerpt.push('\n');
                    }
                    // Reserve space for every line, including late matches after large context lines.
                    let remaining_lines = end - start + 1 - offset;
                    let available = 8192usize.saturating_sub(excerpt.len());
                    let budget = ((available - (remaining_lines - 1)) / remaining_lines).min(1024);
                    let matched = expression.as_ref().map_or_else(
                        || line.find(options.query),
                        |re| re.find(line).map(|m| m.start()),
                    );
                    let mut from = if line.len() > budget {
                        matched.unwrap_or(0).saturating_sub(budget / 4)
                    } else {
                        0
                    };
                    while !line.is_char_boundary(from) {
                        from -= 1;
                    }
                    let mut to = line.len().min(from + budget);
                    while !line.is_char_boundary(to) {
                        to -= 1;
                    }
                    excerpt.push_str(&line[from..to]);
                    excerpt_spans.push(ExcerptSpan {
                        line: start + offset + 1,
                        start_byte: from,
                        end_byte: to,
                    });
                    text_truncated |= from > 0 || to < line.len();
                }
                hits.push(Hit {
                    library: version.library.clone(),
                    version: version.version.clone(),
                    revision: version.revision.clone(),
                    provenance: version.provenance.clone(),
                    path: path.clone(),
                    local_path: entry.path().to_owned(),
                    start_line: start + 1,
                    end_line: end + 1,
                    match_lines,
                    text: excerpt,
                    text_truncated,
                    excerpt_end_line: end + 1,
                    excerpt_spans,
                });
                if hits.len() >= options.limit {
                    return Ok(hits);
                }
            }
        }
    }
    Ok(hits)
}
