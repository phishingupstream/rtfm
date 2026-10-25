//! Explicit package/source bindings and offline coverage, independent of registry lookup.
use crate::{
    config::{Binding, Config},
    deps,
    store::{Store, Version},
};
use anyhow::Result;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Serialize)]
pub struct Coverage {
    pub package: String,
    pub version: Option<String>,
    pub manifest: PathBuf,
    pub evidence: String,
    pub docs: Part,
    pub code: Part,
    pub discover: Option<String>,
    pub possible_sources: Vec<PossibleSource>,
}
#[derive(Serialize)]
pub struct PossibleSource {
    pub source: String,
    pub location: String,
    pub reference: String,
    pub revision: Option<String>,
    pub evidence: String,
}

fn parse_filter(input: &str) -> Result<(String, Option<String>)> {
    let (_, name) = input.split_once(':').ok_or_else(|| {
        anyhow::anyhow!(
        "expected ecosystem:name or ecosystem:name@exact-version (for example npm:@scope/package)"
    )
    })?;
    let has_version = name.rfind('@').is_some_and(|i| i > 0);
    let spec = crate::registry::PackageSpec::parse(&if has_version {
        input.to_owned()
    } else {
        format!("{input}@0")
    })?;
    Ok((
        format!("{}:{}", spec.ecosystem, spec.name),
        has_version.then_some(spec.version),
    ))
}

pub fn validate_filter(input: &str) -> Result<()> {
    parse_filter(input).map(|_| ())
}

fn normalized_name(ecosystem: &str, name: &str) -> String {
    if ecosystem == "pypi" {
        // PyPI treats runs of dots, underscores and hyphens as equivalent.
        name.to_ascii_lowercase()
            .split(['-', '_', '.'])
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("-")
    } else {
        name.to_owned()
    }
}

fn metadata_matches(root: &Path, ecosystem: &str, name: &str) -> bool {
    let (file, section) = match ecosystem {
        "npm" => ("package.json", ""),
        "pypi" => ("pyproject.toml", "project"),
        "crates.io" => ("Cargo.toml", "package"),
        _ => return false,
    };
    let path = root.join(file);
    // Suggestions must not scan trees or load arbitrarily large metadata files.
    if !std::fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() <= 128 * 1024) {
        return false;
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let declared = if ecosystem == "npm" {
        serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v.get("name")?.as_str().map(str::to_owned))
    } else {
        toml::from_str::<toml::Value>(&text)
            .ok()
            .and_then(|v| v.get(section)?.get("name")?.as_str().map(str::to_owned))
    };
    declared.is_some_and(|declared| normalized_name(ecosystem, &declared) == name)
}

fn possible_sources(
    d: &deps::Dependency,
    config: &Config,
    versions: &[Version],
) -> Vec<PossibleSource> {
    let mut candidates = Vec::new();
    for (name, source) in &config.sources {
        let location_name = source
            .location
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("")
            .trim_end_matches(".git");
        let name_match = normalized_name(&d.ecosystem, name) == d.name
            || (!d.name.starts_with('@') && normalized_name(&d.ecosystem, location_name) == d.name);
        for reference in &source.refs {
            let version = versions
                .iter()
                .find(|v| v.library == *name && v.version == *reference && v.current);
            let metadata_match =
                version.is_some_and(|v| metadata_matches(&v.root, &d.ecosystem, &d.name));
            if !name_match && !metadata_match {
                continue;
            }
            candidates.push(PossibleSource {
                source: name.clone(),
                location: source.location.clone(),
                reference: reference.clone(),
                revision: version.map(|v| v.revision.clone()),
                evidence: format!(
                    "{}; package/version correspondence unproven; explicit binding required",
                    if metadata_match {
                        "cached package metadata name matches"
                    } else {
                        "source name or location basename matches"
                    }
                ),
            });
            if candidates.len() == 5 {
                return candidates;
            }
        }
    }
    candidates
}

#[derive(Serialize)]
pub struct Part {
    pub status: String,
    pub source: Option<String>,
    pub reference: Option<String>,
    pub evidence: Option<String>,
    pub revision: Option<String>,
}
fn empty(status: &str) -> Part {
    Part {
        status: status.into(),
        source: None,
        reference: None,
        evidence: None,
        revision: None,
    }
}
fn part(
    binding: Option<&Binding>,
    kind: &str,
    config: &Config,
    versions: &[Version],
    store: &Store,
) -> Result<Part> {
    let Some(binding) = binding else {
        return Ok(empty("unmapped"));
    };
    let mut p = Part {
        status: "configured".into(),
        source: Some(binding.source.clone()),
        reference: Some(binding.reference.clone()),
        evidence: Some(binding.evidence.clone()),
        revision: None,
    };
    let Some(source) = config.sources.get(&binding.source) else {
        p.status = "missing".into();
        return Ok(p);
    };
    if !source.enabled {
        p.status = "disabled".into();
        return Ok(p);
    }
    let Some(version) = versions.iter().find(|v| {
        v.library == binding.source
            && (v.revision == binding.reference || (v.version == binding.reference && v.current))
    }) else {
        return Ok(p);
    };
    p.revision = Some(version.revision.clone());
    if !version.root.is_dir() {
        p.status = "missing".into();
        return Ok(p);
    }
    let available = if kind == "docs" {
        store.db.query_row(
            "SELECT count(*) FROM chunks WHERE library=?1 AND revision=?2 AND kind='docs'",
            rusqlite::params![binding.source, version.revision],
            |r| r.get::<_, usize>(0),
        )? > 0
    } else {
        let mut included = globset::GlobSetBuilder::new();
        for pattern in &source.code {
            included.add(globset::Glob::new(pattern)?);
        }
        let included = included.build()?;
        let mut excluded = globset::GlobSetBuilder::new();
        for pattern in &source.exclude {
            excluded.add(globset::Glob::new(pattern)?);
        }
        let excluded = excluded.build()?;
        let mut found = false;
        for entry in walkdir::WalkDir::new(&version.root)
            .into_iter()
            .filter_entry(|e| e.file_name() != ".git")
        {
            let entry = entry?;
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path().strip_prefix(&version.root)?;
            if included.is_match(path)
                && !excluded.is_match(path)
                && entry.metadata()?.len() <= 2 * 1024 * 1024
            {
                let bytes = std::fs::read(entry.path())?;
                if !bytes.contains(&0) && std::str::from_utf8(&bytes).is_ok() {
                    found = true;
                    break;
                }
            }
        }
        found
    };
    p.status = if available { "cached" } else { "missing" }.into();
    Ok(p)
}
pub fn coverage(
    dependencies: &[deps::Dependency],
    config: &Config,
    store: &Store,
    filter: Option<&str>,
) -> Result<Vec<Coverage>> {
    let filter = filter.map(parse_filter).transpose()?;
    let versions = store.versions(None)?;
    dependencies
        .iter()
        .filter(|d| {
            filter.as_ref().is_none_or(|(package, version)| {
                *package == format!("{}:{}", d.ecosystem, d.name)
                    && version
                        .as_ref()
                        .is_none_or(|v| d.version.as_ref() == Some(v))
            })
        })
        .map(|d| {
            let package = format!("{}:{}", d.ecosystem, d.name);
            let key = d.version.as_ref().map(|v| format!("{package}@{v}"));
            let mapping = key.as_ref().and_then(|k| config.packages.get(k));
            let fallback = if d.version.is_none() {
                "unresolved"
            } else if config
                .packages
                .keys()
                .any(|k| k.starts_with(&format!("{package}@")))
            {
                "mismatched"
            } else {
                "unmapped"
            };
            let (docs, code) = if let Some(m) = mapping {
                (
                    part(m.docs.as_ref(), "docs", config, &versions, store)?,
                    part(m.code.as_ref(), "code", config, &versions, store)?,
                )
            } else {
                (empty(fallback), empty(fallback))
            };
            let possible_sources = if docs.status != "cached" || code.status != "cached" {
                possible_sources(d, config, &versions)
            } else {
                Vec::new()
            };
            Ok(Coverage {
                possible_sources,
                package,
                version: d.version.clone(),
                manifest: d.manifest.clone(),
                evidence: d.evidence.clone(),
                docs,
                code,
                discover: key.map(|k| format!("rtfm discover {k}")),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{PackageMapping, Source, SourceKind};

    #[test]
    fn filters_validate_names_versions_and_scopes() {
        assert_eq!(
            parse_filter("pypi:My_Package.Name").unwrap(),
            ("pypi:my-package-name".into(), None)
        );
        assert_eq!(
            parse_filter("npm:@scope/pkg").unwrap(),
            ("npm:@scope/pkg".into(), None)
        );
        assert_eq!(
            parse_filter("npm:@scope/pkg@1.2.3").unwrap(),
            ("npm:@scope/pkg".into(), Some("1.2.3".into()))
        );
        for input in [
            "",
            "pkg",
            "pip:pkg",
            "npm:@scope",
            "npm:pkg@",
            "npm:pkg@^1",
            "npm:pkg@1@2",
        ] {
            assert!(validate_filter(input).is_err(), "{input}");
        }
    }

    #[test]
    fn fixture_filter_precedes_role_checks_and_candidates_never_bind() {
        let fixture = tempfile::tempdir().unwrap();
        let project = fixture.path().join("project");
        std::fs::create_dir(&project).unwrap();
        std::fs::write(
            project.join("package.json"),
            r#"{"dependencies":{"wanted":"1.2.3","other":"2.0.0","@scope/pkg":"3.0.0"}}"#,
        )
        .unwrap();
        let cache = fixture.path().join("cache");
        let store = Store::open(&cache).unwrap();
        let source_root = fixture.path().join("source");
        std::fs::create_dir(&source_root).unwrap();
        std::fs::write(
            source_root.join("package.json"),
            r#"{"name":"@scope/pkg","version":"0.1.0"}"#,
        )
        .unwrap();
        let mut config = Config::default();
        config.sources.insert(
            "wanted".into(),
            Source {
                kind: SourceKind::Local,
                location: source_root.to_string_lossy().into(),
                checksum: None,
                refs: vec!["HEAD".into()],
                docs: vec![],
                code: vec!["[".into()],
                exclude: vec![],
                enabled: true,
            },
        );
        store
            .db
            .execute(
                "INSERT INTO versions VALUES (?1,'HEAD','revision',1,?2,'fixture',0)",
                rusqlite::params!["wanted", source_root.to_string_lossy()],
            )
            .unwrap();
        // The malformed code glob would fail if the excluded dependency were checked.
        config.packages.insert(
            "npm:other@2.0.0".into(),
            PackageMapping {
                docs: None,
                code: Some(Binding {
                    source: "wanted".into(),
                    reference: "HEAD".into(),
                    evidence: "fixture".into(),
                }),
            },
        );
        config
            .packages
            .insert("npm:wanted@0.1.0".into(), PackageMapping::default());
        let inventory = deps::inventory(&project).unwrap();
        let rows = coverage(
            &inventory.dependencies,
            &config,
            &store,
            Some("npm:wanted@1.2.3"),
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].docs.status, "mismatched");
        assert_eq!(rows[0].code.status, "mismatched");
        assert_eq!(
            rows[0].possible_sources[0].revision.as_deref(),
            Some("revision")
        );
        assert!(rows[0].possible_sources[0]
            .evidence
            .contains("correspondence unproven"));
        let rows = coverage(
            &inventory.dependencies,
            &config,
            &store,
            Some("npm:@scope/pkg"),
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].code.status, "unmapped");
        assert_eq!(rows[0].possible_sources.len(), 1);
        assert!(rows[0].possible_sources[0].evidence.contains("metadata"));
        assert!(coverage(
            &inventory.dependencies,
            &config,
            &store,
            Some("npm:wanted@9.0.0")
        )
        .unwrap()
        .is_empty());
        assert!(coverage(&inventory.dependencies, &config, &store, Some("npm:other")).is_err());
    }
}
