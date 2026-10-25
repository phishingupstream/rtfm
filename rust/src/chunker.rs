//! Bounded text chunks with exact source line ranges and Markdown headings.
use crate::config::Source;
use anyhow::{Context, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use sha2::{Digest, Sha256};
use std::{fs, path::Path};
use walkdir::WalkDir;

#[derive(Debug)]
pub struct Chunk {
    pub path: String,
    pub kind: &'static str,
    pub heading: String,
    pub start_line: usize,
    pub end_line: usize,
    pub text: String,
}
pub struct FileDigest {
    pub path: String,
    pub digest: String,
}
pub struct ReadResult {
    pub chunks: Vec<Chunk>,
    pub files: Vec<FileDigest>,
    pub reused_files: usize,
}

fn patterns(patterns: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(Glob::new(pattern).with_context(|| format!("invalid path pattern {pattern}"))?);
    }
    Ok(builder.build()?)
}
pub fn read<F>(root: &Path, source: &Source, mut reuse: F) -> Result<ReadResult>
where
    F: FnMut(&str, &str) -> Result<Option<Vec<Chunk>>>,
{
    let docs = patterns(&source.docs)?;
    let excluded = patterns(&source.exclude)?;
    let mut skipped = 0usize;
    let mut chunks = Vec::new();
    let mut files = Vec::new();
    let mut reused_files = 0;
    for entry in WalkDir::new(root)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|e| e.file_name() != ".git")
    {
        crate::cache::check_interrupted()?;
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry
            .path()
            .strip_prefix(root)?
            .to_string_lossy()
            .replace('\\', "/");
        if excluded.is_match(&path) {
            continue;
        }
        if !docs.is_match(&path) {
            continue;
        }
        // Large generated files and binary payloads are not useful search documents.
        if entry.metadata()?.len() > 2 * 1024 * 1024 {
            skipped += 1;
            continue;
        }
        let bytes =
            fs::read(entry.path()).with_context(|| format!("read {}", entry.path().display()))?;
        if bytes.contains(&0) {
            skipped += 1;
            continue;
        }
        let digest = format!("{:x}", Sha256::digest(&bytes));
        let Ok(text) = String::from_utf8(bytes) else {
            skipped += 1;
            continue;
        };
        if let Some(cached) = reuse(&path, &digest)? {
            chunks.extend(cached);
            reused_files += 1;
        } else {
            chunks.extend(chunk(&text, &path, "docs"));
        }
        files.push(FileDigest { path, digest });
    }
    if skipped > 0 {
        eprintln!("Skipped {skipped} selected file(s): binary, non-UTF-8, or larger than 2 MiB");
    }
    Ok(ReadResult {
        chunks,
        files,
        reused_files,
    })
}
pub fn chunk(text: &str, path: &str, kind: &'static str) -> Vec<Chunk> {
    let mut result = Vec::new();
    let mut headings: Vec<(usize, String)> = Vec::new();
    let mut fence: Option<char> = None;
    let mut body = String::new();
    let mut start = 1;
    let mut end = 0;
    let emit =
        |result: &mut Vec<Chunk>, body: &mut String, start, end, headings: &[(usize, String)]| {
            if !body.trim().is_empty() {
                result.push(Chunk {
                    path: path.into(),
                    kind,
                    heading: headings
                        .iter()
                        .map(|(_, text)| text.as_str())
                        .collect::<Vec<_>>()
                        .join(" > "),
                    start_line: start,
                    end_line: end,
                    text: std::mem::take(body),
                });
            } else {
                body.clear();
            }
        };
    for (offset, line) in text.lines().enumerate() {
        let number = offset + 1;
        let trimmed = line.trim_start();
        let marker = if trimmed.starts_with("```") {
            Some('`')
        } else if trimmed.starts_with("~~~") {
            Some('~')
        } else {
            None
        };
        let level = if kind == "docs" && fence.is_none() {
            line.chars().take_while(|c| *c == '#').count()
        } else {
            0
        };
        let heading = level > 0 && level <= 6 && line.as_bytes().get(level) == Some(&b' ');
        if (heading || body.len() + line.len() + 1 > 2400 || number - start >= 60)
            && !body.is_empty()
        {
            emit(&mut result, &mut body, start, end, &headings);
            start = number;
        }
        if heading {
            while headings
                .last()
                .is_some_and(|(previous, _)| *previous >= level)
            {
                headings.pop();
            }
            headings.push((
                level,
                line[level..].trim().trim_end_matches('#').trim().into(),
            ));
        }
        if let Some(marker) = marker {
            if fence == Some(marker) {
                fence = None;
            } else if fence.is_none() {
                fence = Some(marker);
            }
        }
        if body.is_empty() {
            start = number;
        }
        // Keep byte bounds without cutting a UTF-8 code point. Multiple
        // fragments of one long source line retain that exact line number.
        let mut rest = line;
        while rest.len() > 2399 {
            let mut boundary = 2399;
            while !rest.is_char_boundary(boundary) {
                boundary -= 1;
            }
            body.push_str(&rest[..boundary]);
            emit(&mut result, &mut body, number, number, &headings);
            rest = &rest[boundary..];
            start = number;
        }
        body.push_str(rest);
        body.push('\n');
        end = number;
    }
    emit(&mut result, &mut body, start, end, &headings);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keeps_small_sections_and_fenced_headings_with_lines() {
        let chunks = chunk(
            "# API\nx\n```python\n# comment\n```\n## Next\ny",
            "api.md",
            "docs",
        );
        assert_eq!(chunks.len(), 2);
        assert_eq!((chunks[0].start_line, chunks[0].end_line), (1, 5));
        assert_eq!(chunks[1].heading, "API > Next");
        assert_eq!(chunks[1].start_line, 6);
    }
    #[test]
    fn skipped_heading_levels_preserve_siblings_and_ancestors() {
        let chunks = chunk(
            "# API\n### First\none\n### Second\ntwo\n## Parent\n#### Child\nthree\n# Other\nfour",
            "guide.md",
            "docs",
        );
        let headings: Vec<_> = chunks.iter().map(|c| c.heading.as_str()).collect();
        assert_eq!(
            headings,
            [
                "API",
                "API > First",
                "API > Second",
                "API > Parent",
                "API > Parent > Child",
                "Other"
            ]
        );
        assert_eq!(chunks[2].start_line, 4);
        assert_eq!(chunks[2].text, "### Second\ntwo\n");
    }

    #[test]
    fn handles_long_unicode_lines() {
        let text = "🦀".repeat(2000);
        let chunks = chunk(&text, "code.rs", "code");
        assert!(chunks
            .iter()
            .all(|c| c.text.len() <= 2400 && c.start_line == 1 && c.end_line == 1));
        assert_eq!(
            chunks.iter().map(|c| c.text.as_str()).collect::<String>(),
            format!("{text}\n")
        );
    }
    #[test]
    fn incremental_read_reuses_only_unchanged_selected_files() -> Result<()> {
        use crate::config::SourceKind;
        let fixture = tempfile::tempdir()?;
        std::fs::write(fixture.path().join("same.md"), "# API\nunchanged docs")?;
        std::fs::write(fixture.path().join("changed.md"), "old docs")?;
        let source = Source {
            checksum: None,
            kind: SourceKind::Local,
            location: "unused".into(),
            refs: vec!["HEAD".into()],
            docs: vec!["**/*.md".into()],
            code: vec!["**/*.rs".into()],
            exclude: vec![],
            enabled: true,
        };
        std::fs::write(fixture.path().join("api.rs"), "codeonlysymbol();")?;
        let first = read(fixture.path(), &source, |_, _| Ok(None))?;
        assert_eq!(first.files.len(), 2);
        assert!(first.chunks.iter().all(|chunk| chunk.kind == "docs"));
        assert_eq!(first.reused_files, 0);
        let old_digest = first
            .files
            .into_iter()
            .find(|file| file.path == "same.md")
            .unwrap()
            .digest;
        let mut cached = Some(
            first
                .chunks
                .into_iter()
                .filter(|chunk| chunk.path == "same.md")
                .collect::<Vec<_>>(),
        );
        std::fs::write(fixture.path().join("changed.md"), "new docs")?;
        let second = read(fixture.path(), &source, |path, digest| {
            if path == "same.md" && digest == old_digest {
                Ok(cached.take())
            } else {
                Ok(None)
            }
        })?;
        assert_eq!(second.reused_files, 1);
        assert_eq!(second.files.len(), 2);
        assert!(second
            .chunks
            .iter()
            .any(|chunk| chunk.text.contains("new docs")));
        assert!(!second
            .chunks
            .iter()
            .any(|chunk| chunk.text.contains("old docs")));
        Ok(())
    }
}
