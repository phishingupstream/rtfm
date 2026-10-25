//! Offline direct dependency inventory. Discovery excludes generated, hidden and experimental trees.
use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Debug, Clone, Serialize)]
pub struct Dependency {
    pub ecosystem: String,
    pub name: String,
    pub version: Option<String>,
    pub manifest: PathBuf,
    pub evidence: String,
}

/// Scan ordinary project/workspace directories, without fetching or resolving constraints.
/// Each resolved version has its own row; missing lock evidence leaves version null.
#[derive(Debug, Serialize)]
pub struct Inventory {
    pub root: PathBuf,
    pub manifests: Vec<PathBuf>,
    pub exclusions: Vec<&'static str>,
    pub dependencies: Vec<Dependency>,
}
#[cfg(test)]
fn scan(root: &Path) -> Result<Vec<Dependency>> {
    Ok(inventory(root)?.dependencies)
}
pub fn inventory(root: &Path) -> Result<Inventory> {
    let root = root.canonicalize().with_context(|| {
        format!(
            "locate dependency inventory root {}; provide an existing project directory",
            root.display()
        )
    })?;
    anyhow::ensure!(
        root.is_dir(),
        "dependency inventory root {} must be a directory",
        root.display()
    );
    let mut manifests = Vec::new();
    let mut result = Vec::new();
    for entry in WalkDir::new(&root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            e.depth() == 0 || !e.file_type().is_dir() || {
                let name = e.file_name().to_string_lossy();
                !name.starts_with('.')
                    && !matches!(
                        name.as_ref(),
                        "node_modules"
                            | "target"
                            | "__pycache__"
                            | "venv"
                            | "experiments"
                            | "experiment"
                    )
            }
        })
    {
        let entry = entry.context("discover dependency manifests")?;
        if !entry.file_type().is_file() {
            continue;
        }
        if matches!(
            entry.file_name().to_str(),
            Some("pyproject.toml" | "package.json" | "Cargo.toml")
        ) {
            manifests.push(entry.path().to_owned());
        }
        match entry.file_name().to_str() {
            Some("pyproject.toml") => python(entry.path(), &root, &mut result)?,
            Some("package.json") => npm(entry.path(), &root, &mut result)?,
            Some("Cargo.toml") => cargo(entry.path(), &root, &mut result)?,
            _ => {}
        }
    }
    result.sort_by(|a, b| {
        (&a.manifest, &a.ecosystem, &a.name, &a.version).cmp(&(
            &b.manifest,
            &b.ecosystem,
            &b.name,
            &b.version,
        ))
    });
    result.dedup_by(|a, b| {
        a.manifest == b.manifest
            && a.ecosystem == b.ecosystem
            && a.name == b.name
            && a.version == b.version
    });
    manifests.sort();
    Ok(Inventory {
        root,
        manifests,
        exclusions: vec![
            "hidden directories",
            "node_modules",
            "target",
            "__pycache__",
            "venv",
            "experiments",
            "experiment",
        ],
        dependencies: result,
    })
}

fn read_toml(path: &Path) -> Result<toml::Value> {
    std::fs::read_to_string(path)
        .with_context(|| format!("read {}", path.display()))?
        .parse()
        .with_context(|| format!("parse {}", path.display()))
}
fn read_json(path: &Path) -> Result<serde_json::Value> {
    serde_json::from_str(
        &std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?,
    )
    .with_context(|| format!("parse {}", path.display()))
}
fn lockfile(manifest: &Path, root: &Path, name: &str) -> Option<PathBuf> {
    manifest
        .parent()?
        .ancestors()
        .take_while(|p| p.starts_with(root))
        .map(|p| p.join(name))
        .find(|p| p.is_file())
}
fn record(
    out: &mut Vec<Dependency>,
    ecosystem: &str,
    name: &str,
    manifest: &Path,
    versions: BTreeSet<String>,
    evidence: String,
    pin: Option<String>,
) {
    if versions.is_empty() {
        out.push(Dependency {
            ecosystem: ecosystem.into(),
            name: name.into(),
            version: pin.clone(),
            manifest: manifest.into(),
            evidence: if pin.is_some() {
                "exact manifest pin (no matching lock entry)".into()
            } else {
                "unresolved: no matching lock entry; manifest constraints are not resolved versions"
                    .into()
            },
        });
    } else {
        for version in versions {
            out.push(Dependency {
                ecosystem: ecosystem.into(),
                name: name.into(),
                version: Some(version),
                manifest: manifest.into(),
                evidence: evidence.clone(),
            });
        }
    }
}
fn origin_gap(
    out: &mut Vec<Dependency>,
    ecosystem: &str,
    name: &str,
    manifest: &Path,
    detail: &str,
) {
    out.push(Dependency { ecosystem: ecosystem.into(), name: name.into(), version: None, manifest: manifest.into(), evidence: format!("unresolved: non-public-registry or unverified origin ({detail}); public registry discovery is not applicable") });
}
fn public_url(value: &str, host: &str) -> bool {
    url::Url::parse(value).is_ok_and(|u| {
        u.scheme() == "https"
            && u.host_str() == Some(host)
            && u.port().is_none()
            && u.username().is_empty()
            && u.password().is_none()
    })
}
fn public_cargo_source(value: &str) -> bool {
    matches!(
        value,
        "registry+https://github.com/rust-lang/crates.io-index" | "sparse+https://index.crates.io/"
    )
}
fn normalized_python(name: &str) -> String {
    let mut result = String::new();
    for c in name.chars() {
        if matches!(c, '-' | '_' | '.') {
            if !result.ends_with('-') {
                result.push('-');
            }
        } else {
            result.push(c.to_ascii_lowercase());
        }
    }
    result
}
fn python_req(req: &str) -> Option<(String, Option<String>)> {
    let req = req.trim();
    let end = req
        .find(|c: char| !c.is_ascii_alphanumeric() && !matches!(c, '-' | '_' | '.'))
        .unwrap_or(req.len());
    if end == 0 {
        return None;
    }
    let name = normalized_python(&req[..end]);
    let mut rest = req[end..].trim();
    if rest.starts_with('[') {
        rest = rest.split_once(']')?.1.trim();
    }
    let version = rest
        .strip_prefix("==")
        .filter(|v| !v.starts_with('='))
        .map(|v| v.split(';').next().unwrap_or("").trim())
        .filter(|v| !v.is_empty() && !v.contains(['*', ',', ' ', '<', '>', '!', '=']))
        .map(str::to_owned);
    Some((name, version))
}
fn requirement_arrays(value: &toml::Value, out: &mut Vec<String>) {
    if let Some(array) = value.as_array() {
        for value in array {
            if let Some(req) = value.as_str() {
                out.push(req.into());
            }
        }
    }
}
fn ancestor_tomls(manifest: &Path, root: &Path, filename: &str) -> Result<Vec<toml::Value>> {
    manifest
        .parent()
        .unwrap()
        .ancestors()
        .take_while(|p| p.starts_with(root))
        .map(|p| p.join(filename))
        .filter(|p| p.is_file())
        .map(|p| read_toml(&p))
        .collect()
}
fn uv_custom_index(uv: &toml::Value) -> bool {
    uv.get("index-url")
        .and_then(toml::Value::as_str)
        .is_some_and(|r| !public_url(r, "pypi.org"))
        || uv
            .get("index")
            .and_then(toml::Value::as_array)
            .is_some_and(|indexes| {
                indexes.iter().any(|index| {
                    index.get("explicit").and_then(toml::Value::as_bool) != Some(true)
                        && !index
                            .get("url")
                            .and_then(toml::Value::as_str)
                            .is_some_and(|r| public_url(r, "pypi.org"))
                })
            })
}
fn npm_custom_registry(manifest: &Path, root: &Path, name: &str) -> Result<bool> {
    let scoped_key = name
        .starts_with('@')
        .then(|| format!("{}:registry", name.split('/').next().unwrap()));
    for directory in manifest
        .parent()
        .unwrap()
        .ancestors()
        .take_while(|p| p.starts_with(root))
    {
        let path = directory.join(".npmrc");
        if !path.is_file() {
            continue;
        }
        let text =
            std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        for line in text
            .lines()
            .map(str::trim)
            .filter(|line| !line.starts_with(['#', ';']))
        {
            if let Some((key, value)) = line.split_once('=') {
                if (key.trim() == "registry" || scoped_key.as_deref() == Some(key.trim()))
                    && !public_url(
                        value.trim().trim_matches(['\"', '\'']),
                        "registry.npmjs.org",
                    )
                {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}
fn cargo_custom_registry(manifest: &Path, root: &Path) -> Result<bool> {
    for filename in [".cargo/config.toml", ".cargo/config"] {
        for config in ancestor_tomls(manifest, root, filename)? {
            if config
                .get("registry")
                .and_then(|v| v.get("default"))
                .and_then(toml::Value::as_str)
                .is_some_and(|r| r != "crates-io")
                || config
                    .get("source")
                    .and_then(|v| v.get("crates-io"))
                    .and_then(|v| v.get("replace-with"))
                    .is_some()
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}
fn python(manifest: &Path, root: &Path, out: &mut Vec<Dependency>) -> Result<()> {
    let data = read_toml(manifest)?;
    let configs = ancestor_tomls(manifest, root, "pyproject.toml")?;
    let custom_index = configs
        .iter()
        .filter_map(|v| v.get("tool").and_then(|v| v.get("uv")))
        .any(uv_custom_index)
        || ancestor_tomls(manifest, root, "uv.toml")?
            .iter()
            .any(uv_custom_index);
    let mut reqs = Vec::new();
    if let Some(project) = data.get("project") {
        if let Some(deps) = project.get("dependencies") {
            requirement_arrays(deps, &mut reqs);
        }
        if let Some(groups) = project
            .get("optional-dependencies")
            .and_then(toml::Value::as_table)
        {
            for deps in groups.values() {
                requirement_arrays(deps, &mut reqs);
            }
        }
    }
    if let Some(groups) = data
        .get("dependency-groups")
        .and_then(toml::Value::as_table)
    {
        for deps in groups.values() {
            requirement_arrays(deps, &mut reqs);
        }
    }
    if let Some(deps) = data
        .get("tool")
        .and_then(|v| v.get("uv"))
        .and_then(|v| v.get("dev-dependencies"))
    {
        requirement_arrays(deps, &mut reqs);
    }
    let lock = lockfile(manifest, root, "uv.lock");
    let locked = lock.as_deref().map(read_toml).transpose()?;
    for req in reqs {
        let Some((name, pin)) = python_req(&req) else {
            continue;
        };
        let override_source = configs
            .iter()
            .filter_map(|v| {
                v.get("tool")
                    .and_then(|v| v.get("uv"))
                    .and_then(|v| v.get("sources"))
            })
            .filter_map(toml::Value::as_table)
            .any(|sources| sources.keys().any(|key| normalized_python(key) == name));
        let matches: Vec<_> = locked
            .as_ref()
            .and_then(|v| v.get("package"))
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
            .filter(|p| {
                p.get("name")
                    .and_then(toml::Value::as_str)
                    .is_some_and(|n| normalized_python(n) == name)
            })
            .collect();
        if req.split(';').next().unwrap_or("").contains('@')
            || override_source
            || custom_index
            || matches.iter().any(|p| {
                !p.get("source")
                    .and_then(|s| s.get("registry"))
                    .and_then(toml::Value::as_str)
                    .is_some_and(|r| public_url(r, "pypi.org"))
            })
        {
            origin_gap(
                out,
                "pypi",
                &name,
                manifest,
                "direct URL, uv source/index override, or lock source is not verified public PyPI",
            );
            continue;
        }
        let versions = matches
            .into_iter()
            .filter_map(|p| {
                p.get("version")
                    .and_then(toml::Value::as_str)
                    .map(str::to_owned)
            })
            .collect();
        record(
            out,
            "pypi",
            &name,
            manifest,
            versions,
            format!(
                "{} package entries (may include marker-specific versions)",
                lock.as_deref().unwrap_or(manifest).display()
            ),
            pin,
        );
    }
    Ok(())
}
fn npm_alias<'a>(key: &str, spec: &'a str) -> (String, &'a str) {
    if let Some(alias) = spec.strip_prefix("npm:") {
        if let Some(split) = alias.rfind('@').filter(|i| *i > 0) {
            return (alias[..split].into(), &alias[split + 1..]);
        }
        return (alias.into(), "");
    }
    (key.into(), spec)
}
fn exact_numeric(spec: &str) -> Option<String> {
    // Conservative npm/Cargo pins: three numeric components and optional prerelease/build suffix.
    let version = spec.strip_prefix('=').unwrap_or(spec);
    let core = version.split(['-', '+']).next()?;
    if core.split('.').count() == 3
        && core
            .split('.')
            .all(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
        && version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'))
    {
        Some(version.into())
    } else {
        None
    }
}
fn npm(manifest: &Path, root: &Path, out: &mut Vec<Dependency>) -> Result<()> {
    let data = read_json(manifest)?;
    let lock = lockfile(manifest, root, "package-lock.json");
    let locked = lock.as_deref().map(read_json).transpose()?;
    for group in [
        "dependencies",
        "devDependencies",
        "optionalDependencies",
        "peerDependencies",
    ] {
        let Some(deps) = data.get(group).and_then(serde_json::Value::as_object) else {
            continue;
        };
        for (key, spec) in deps {
            let spec = spec.as_str().unwrap_or("");
            let (name, version_spec) = npm_alias(key, spec);
            if version_spec.contains([':', '/'])
                || version_spec.starts_with('.')
                || npm_custom_registry(manifest, root, &name)?
            {
                origin_gap(
                    out,
                    "npm",
                    &name,
                    manifest,
                    "file/link/workspace/Git/URL specification or .npmrc registry override",
                );
                continue;
            }
            let mut unverified_origin = false;
            let mut versions = BTreeSet::new();
            if let (Some(locked), Some(lock)) = (&locked, &lock) {
                if let Some(packages) = locked
                    .get("packages")
                    .and_then(serde_json::Value::as_object)
                {
                    let base = lock.parent().unwrap();
                    let mut dir = manifest.parent().unwrap();
                    loop {
                        let prefix = dir
                            .strip_prefix(base)
                            .unwrap_or(Path::new(""))
                            .to_string_lossy()
                            .replace('\\', "/");
                        let path = if prefix.is_empty() {
                            format!("node_modules/{key}")
                        } else {
                            format!("{prefix}/node_modules/{key}")
                        };
                        if let Some(package) = packages.get(&path) {
                            unverified_origin =
                                package.get("link").and_then(serde_json::Value::as_bool)
                                    == Some(true)
                                    || !package
                                        .get("resolved")
                                        .and_then(serde_json::Value::as_str)
                                        .is_some_and(|r| public_url(r, "registry.npmjs.org"));
                            if let Some(v) =
                                package.get("version").and_then(serde_json::Value::as_str)
                            {
                                versions.insert(v.into());
                            }
                            break;
                        }
                        if dir == base {
                            break;
                        }
                        let Some(parent) = dir.parent().filter(|p| p.starts_with(base)) else {
                            break;
                        };
                        dir = parent;
                    }
                } else if manifest.parent() == lock.parent() {
                    // npm lockfile v1 has only the root dependency tree.
                    if let Some(package) = locked.get("dependencies").and_then(|d| d.get(key)) {
                        unverified_origin = !package
                            .get("resolved")
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|r| public_url(r, "registry.npmjs.org"));
                        if let Some(v) = package.get("version").and_then(serde_json::Value::as_str)
                        {
                            let (_, v) = npm_alias(key, v);
                            if let Some(v) = exact_numeric(v) {
                                versions.insert(v);
                            }
                        }
                    }
                }
            }
            if unverified_origin {
                origin_gap(
                    out,
                    "npm",
                    &name,
                    manifest,
                    "lock entry is linked or has no verified public npm resolved URL",
                );
                continue;
            }
            record(
                out,
                "npm",
                &name,
                manifest,
                versions,
                format!(
                    "{} installed direct dependency entry",
                    lock.as_deref().unwrap_or(manifest).display()
                ),
                exact_numeric(version_spec),
            );
        }
    }
    Ok(())
}
fn cargo(manifest: &Path, root: &Path, out: &mut Vec<Dependency>) -> Result<()> {
    let data = read_toml(manifest)?;
    let custom_registry = cargo_custom_registry(manifest, root)?;
    let manifests = ancestor_tomls(manifest, root, "Cargo.toml")?;
    let lock = lockfile(manifest, root, "Cargo.lock");
    let locked = lock.as_deref().map(read_toml).transpose()?;
    let mut groups = Vec::new();
    for key in ["dependencies", "dev-dependencies", "build-dependencies"] {
        if let Some(group) = data.get(key).and_then(toml::Value::as_table) {
            groups.push(group);
        }
    }
    if let Some(targets) = data.get("target").and_then(toml::Value::as_table) {
        for target in targets.values() {
            for key in ["dependencies", "dev-dependencies", "build-dependencies"] {
                if let Some(group) = target.get(key).and_then(toml::Value::as_table) {
                    groups.push(group);
                }
            }
        }
    }
    let mut workspace = None;
    for parent in manifest
        .parent()
        .unwrap()
        .ancestors()
        .take_while(|p| p.starts_with(root))
    {
        let path = parent.join("Cargo.toml");
        if path.is_file() {
            let candidate = read_toml(&path)?;
            if candidate.get("workspace").is_some() {
                workspace = Some(candidate);
                break;
            }
        }
    }
    let packages = locked
        .as_ref()
        .and_then(|l| l.get("package"))
        .and_then(toml::Value::as_array);
    let owner = data
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(toml::Value::as_str);
    let owner_edges = packages
        .into_iter()
        .flatten()
        .find(|p| p.get("name").and_then(toml::Value::as_str) == owner)
        .and_then(|p| p.get("dependencies"))
        .and_then(toml::Value::as_array);
    for group in groups {
        for (key, original) in group {
            let spec = if original.get("workspace").and_then(toml::Value::as_bool) == Some(true) {
                workspace
                    .as_ref()
                    .and_then(|w| w.get("workspace"))
                    .and_then(|w| w.get("dependencies"))
                    .and_then(|w| w.get(key))
                    .unwrap_or(original)
            } else {
                original
            };
            let name = spec
                .get("package")
                .and_then(toml::Value::as_str)
                .unwrap_or(key);
            let patched = manifests
                .iter()
                .filter_map(|v| v.get("patch").and_then(|v| v.get("crates-io")))
                .filter_map(toml::Value::as_table)
                .any(|patches| {
                    patches.iter().any(|(key, value)| {
                        value
                            .get("package")
                            .and_then(toml::Value::as_str)
                            .unwrap_or(key)
                            == name
                    })
                });
            if custom_registry
                || patched
                || ["git", "path", "registry", "registry-index"]
                    .iter()
                    .any(|key| spec.get(*key).is_some())
                || original.get("workspace").and_then(toml::Value::as_bool) == Some(true)
                    && spec.get("workspace").is_some()
            {
                origin_gap(
                    out,
                    "crates.io",
                    name,
                    manifest,
                    "Git/path/custom-registry, Cargo patch/config override, or unresolved workspace dependency",
                );
                continue;
            }
            let constraint = spec
                .as_str()
                .or_else(|| spec.get("version").and_then(toml::Value::as_str))
                .unwrap_or("");
            let pin = constraint.strip_prefix('=').and_then(exact_numeric);
            let edges: Vec<_> = owner_edges
                .into_iter()
                .flatten()
                .filter_map(toml::Value::as_str)
                .filter(|s| s.split_whitespace().next() == Some(name))
                .collect();
            let matches: Vec<_> = packages
                .into_iter()
                .flatten()
                .filter(|p| p.get("name").and_then(toml::Value::as_str) == Some(name))
                .filter(|p| {
                    edges.iter().any(|edge| {
                        let version_matches = edge.split_whitespace().nth(1).is_none_or(|v| {
                            p.get("version").and_then(toml::Value::as_str) == Some(v)
                        });
                        let source_matches = edge.split_once('(').is_none_or(|(_, source)| {
                            p.get("source").and_then(toml::Value::as_str)
                                == Some(source.trim_end_matches(')'))
                        });
                        version_matches && source_matches
                    })
                })
                .collect();
            if matches.iter().any(|p| {
                !p.get("source")
                    .and_then(toml::Value::as_str)
                    .is_some_and(public_cargo_source)
            }) {
                origin_gap(
                    out,
                    "crates.io",
                    name,
                    manifest,
                    "lock entry is Git/path/custom-registry or has no public crates.io source",
                );
                continue;
            }
            let versions = matches
                .into_iter()
                .filter_map(|p| {
                    p.get("version")
                        .and_then(toml::Value::as_str)
                        .map(str::to_owned)
                })
                .collect();
            record(
                out,
                "crates.io",
                name,
                manifest,
                versions,
                format!(
                    "{} package entries matched to direct dependency",
                    lock.as_deref().unwrap_or(manifest).display()
                ),
                pin,
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (path, text) in files {
            let path = dir.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        dir
    }
    #[test]
    fn python_workspace_and_missing_lock() {
        let dir = fixture(&[("backend/pyproject.toml", "[project]\ndependencies=['Foo_Bar>=1', 'pinned==2.3.4', 'unknown~=3']\n[dependency-groups]\ndev=['pytest>=8']"), ("uv.lock", "[[package]]\nname='foo-bar'\nversion='1.2.3'\nsource={registry='https://pypi.org/simple'}\n[[package]]\nname='pytest'\nversion='8.3.0'\nsource={registry='https://pypi.org/simple'}"), ("experiments/pyproject.toml", "broken")]);
        let deps = scan(dir.path()).unwrap();
        assert_eq!(deps.len(), 4);
        assert_eq!(
            deps.iter()
                .find(|d| d.name == "foo-bar")
                .unwrap()
                .version
                .as_deref(),
            Some("1.2.3")
        );
        assert_eq!(
            deps.iter()
                .find(|d| d.name == "pinned")
                .unwrap()
                .version
                .as_deref(),
            Some("2.3.4")
        );
        assert!(deps
            .iter()
            .find(|d| d.name == "unknown")
            .unwrap()
            .version
            .is_none());
    }
    #[test]
    fn npm_scoped_alias_and_workspace_lock() {
        let dir = fixture(&[
            (
                "frontend/package.json",
                r#"{"dependencies":{"alias":"npm:@scope/real@^2","@scope/plain":"^3","unresolved":"^5"}}"#,
            ),
            (
                "package-lock.json",
                r#"{"lockfileVersion":3,"packages":{"node_modules/alias":{"name":"@scope/real","version":"2.4.0","resolved":"https://registry.npmjs.org/@scope/real/-/real-2.4.0.tgz"},"frontend/node_modules/@scope/plain":{"version":"3.1.0","resolved":"https://registry.npmjs.org/@scope/plain/-/plain-3.1.0.tgz"},"node_modules/@scope/plain":{"version":"3.0.0","resolved":"https://registry.npmjs.org/@scope/plain/-/plain-3.0.0.tgz"}}}"#,
            ),
            ("node_modules/bad/package.json", "invalid"),
        ]);
        let deps = scan(dir.path()).unwrap();
        assert_eq!(deps.len(), 3);
        assert_eq!(
            deps.iter()
                .find(|d| d.name == "@scope/real")
                .unwrap()
                .version
                .as_deref(),
            Some("2.4.0")
        );
        assert_eq!(
            deps.iter()
                .find(|d| d.name == "@scope/plain")
                .unwrap()
                .version
                .as_deref(),
            Some("3.1.0")
        );
        assert!(deps
            .iter()
            .find(|d| d.name == "unresolved")
            .unwrap()
            .version
            .is_none());
    }
    #[test]
    fn cargo_renames_workspace_and_locked_direct_version() {
        let dir = fixture(&[("Cargo.toml", "[workspace]\nmembers=['backend']\n[workspace.dependencies]\nrenamed={package='actual',version='1'}"), ("backend/Cargo.toml", "[package]\nname='app'\nversion='0.1.0'\n[dependencies]\nrenamed.workspace=true\nmissing='1.2.3'\npinned='=2.3.4'"), ("Cargo.lock", "[[package]]\nname='app'\nversion='0.1.0'\ndependencies=['actual 1.2.0']\n[[package]]\nname='actual'\nversion='1.2.0'\nsource='registry+https://github.com/rust-lang/crates.io-index'\n[[package]]\nname='actual'\nversion='2.0.0'\nsource='registry+https://github.com/rust-lang/crates.io-index'")]);
        let deps = scan(dir.path()).unwrap();
        assert_eq!(deps.len(), 3);
        assert_eq!(
            deps.iter()
                .find(|d| d.name == "actual")
                .unwrap()
                .version
                .as_deref(),
            Some("1.2.0")
        );
        assert!(deps
            .iter()
            .find(|d| d.name == "missing")
            .unwrap()
            .version
            .is_none());
        assert_eq!(
            deps.iter()
                .find(|d| d.name == "pinned")
                .unwrap()
                .version
                .as_deref(),
            Some("2.3.4")
        );
    }
    #[test]
    fn no_lock_never_reports_ranges_as_versions() {
        let dir = fixture(&[(
            "package.json",
            r#"{"dependencies":{"range":"^1.2.3","exact":"1.2.3","alias":"npm:real@4.5.6"}}"#,
        )]);
        let deps = scan(dir.path()).unwrap();
        assert!(deps
            .iter()
            .find(|d| d.name == "range")
            .unwrap()
            .version
            .is_none());
        assert_eq!(
            deps.iter()
                .find(|d| d.name == "exact")
                .unwrap()
                .version
                .as_deref(),
            Some("1.2.3")
        );
        assert_eq!(
            deps.iter()
                .find(|d| d.name == "real")
                .unwrap()
                .version
                .as_deref(),
            Some("4.5.6")
        );
    }
    #[test]
    fn python_nonpublic_origins_cannot_collide_with_public_name_and_version() {
        let dir = fixture(&[
            ("pyproject.toml", "[project]\ndependencies=['direct @ git+https://example.test/direct', 'override==1.2.3', 'private==1.2.3', 'unverified==1.2.3', 'public==1.2.3']\n[tool.uv.sources]\noverride={path='../override'}"),
            ("uv.lock", "[[package]]\nname='direct'\nversion='1.2.3'\nsource={registry='https://pypi.org/simple'}\n[[package]]\nname='override'\nversion='1.2.3'\nsource={registry='https://pypi.org/simple'}\n[[package]]\nname='private'\nversion='1.2.3'\nsource={registry='https://private.test/simple'}\n[[package]]\nname='unverified'\nversion='1.2.3'\n[[package]]\nname='public'\nversion='1.2.3'\nsource={registry='https://pypi.org/simple'}"),
        ]);
        let deps = scan(dir.path()).unwrap();
        assert_eq!(deps.len(), 5);
        for dep in &deps {
            if dep.name == "public" {
                assert_eq!(dep.version.as_deref(), Some("1.2.3"));
            } else {
                assert!(dep.version.is_none(), "{dep:?}");
                assert!(dep.evidence.contains("origin"));
            }
        }
    }
    #[test]
    fn npm_nonpublic_origins_cannot_collide_with_public_name_and_version() {
        let dir = fixture(&[
            (
                "package.json",
                r#"{"dependencies":{"local":"file:../local","workspace":"workspace:*","git":"git+https://example.test/git","private":"1.2.3","unverified":"1.2.3","linked":"1.2.3","public":"1.2.3"}}"#,
            ),
            (
                "package-lock.json",
                r#"{"packages":{"node_modules/local":{"version":"1.2.3","resolved":"https://registry.npmjs.org/local/-/local-1.2.3.tgz"},"node_modules/private":{"version":"1.2.3","resolved":"https://private.test/private.tgz"},"node_modules/unverified":{"version":"1.2.3"},"node_modules/linked":{"version":"1.2.3","link":true,"resolved":"https://registry.npmjs.org/linked/-/linked-1.2.3.tgz"},"node_modules/public":{"version":"1.2.3","resolved":"https://registry.npmjs.org/public/-/public-1.2.3.tgz"}}}"#,
            ),
        ]);
        let deps = scan(dir.path()).unwrap();
        assert_eq!(deps.len(), 7);
        for dep in &deps {
            if dep.name == "public" {
                assert_eq!(dep.version.as_deref(), Some("1.2.3"));
            } else {
                assert!(dep.version.is_none(), "{dep:?}");
                assert!(dep.evidence.contains("origin"));
            }
        }
    }
    #[test]
    fn cargo_nonpublic_origins_cannot_collide_with_public_name_and_version() {
        let dir = fixture(&[
            ("Cargo.toml", "[package]\nname='app'\nversion='0.1.0'\n[dependencies]\nlocal={version='=1.2.3',path='../local'}\ngit={version='=1.2.3',git='https://example.test/git'}\nprivate={version='=1.2.3',registry='private'}\nlockprivate='=1.2.3'\nunverified='=1.2.3'\npublic='=1.2.3'"),
            ("Cargo.lock", "[[package]]\nname='app'\nversion='0.1.0'\ndependencies=['local', 'git', 'private', 'lockprivate', 'unverified', 'public']\n[[package]]\nname='local'\nversion='1.2.3'\nsource='registry+https://github.com/rust-lang/crates.io-index'\n[[package]]\nname='lockprivate'\nversion='1.2.3'\nsource='registry+https://private.test/index'\n[[package]]\nname='unverified'\nversion='1.2.3'\n[[package]]\nname='public'\nversion='1.2.3'\nsource='registry+https://github.com/rust-lang/crates.io-index'"),
        ]);
        let deps = scan(dir.path()).unwrap();
        assert_eq!(deps.len(), 6);
        for dep in &deps {
            if dep.name == "public" {
                assert_eq!(dep.version.as_deref(), Some("1.2.3"));
            } else {
                assert!(dep.version.is_none(), "{dep:?}");
                assert!(dep.evidence.contains("origin"));
            }
        }
    }
    #[test]
    fn cargo_root_patch_matches_renamed_package_and_blocks_exact_pin() {
        let dir = fixture(&[
            ("Cargo.toml", "[workspace]\nmembers=['backend']\n[patch.crates-io]\npatch_alias={package='actual',path='../replacement'}"),
            ("backend/Cargo.toml", "[package]\nname='app'\nversion='0.1.0'\n[dependencies]\ndep_alias={package='actual',version='=1.2.3'}\npublic='=2.3.4'"),
        ]);
        let deps = scan(dir.path()).unwrap();
        let patched = deps.iter().find(|d| d.name == "actual").unwrap();
        assert!(patched.version.is_none());
        assert!(patched.evidence.contains("patch/config override"));
        assert_eq!(
            deps.iter()
                .find(|d| d.name == "public")
                .unwrap()
                .version
                .as_deref(),
            Some("2.3.4")
        );
    }
    #[test]
    fn cargo_root_registry_and_source_replacement_configs_block_pin_fallback() {
        for (path, config) in [
            (".cargo/config.toml", "[registry]\ndefault='private'"),
            (".cargo/config", "[source.crates-io]\nreplace-with='vendored'\n[source.vendored]\ndirectory='vendor'"),
        ] {
            let dir = fixture(&[
                ("backend/Cargo.toml", "[package]\nname='app'\nversion='0.1.0'\n[dependencies]\ndemo='=1.2.3'"), (path, config),
            ]);
            let deps = scan(dir.path()).unwrap();
            assert_eq!(deps.len(), 1);
            assert!(deps[0].version.is_none());
            assert!(deps[0].evidence.contains("origin"));
        }
    }
    #[test]
    fn npm_root_scoped_registry_applies_to_alias_target_without_hiding_public_pins() {
        let dir = fixture(&[
            (
                "frontend/package.json",
                r#"{"dependencies":{"alias":"npm:@private/real@1.2.3","@public/real":"1.2.3"}}"#,
            ),
            (
                ".npmrc",
                "@private:registry=https://private.test/npm/\nregistry=https://registry.npmjs.org/",
            ),
        ]);
        let deps = scan(dir.path()).unwrap();
        let private = deps.iter().find(|d| d.name == "@private/real").unwrap();
        assert!(private.version.is_none());
        assert!(private.evidence.contains(".npmrc"));
        assert_eq!(
            deps.iter()
                .find(|d| d.name == "@public/real")
                .unwrap()
                .version
                .as_deref(),
            Some("1.2.3")
        );
        let private_default = fixture(&[
            ("package.json", r#"{"dependencies":{"demo":"1.2.3"}}"#),
            (".npmrc", "registry=https://private.test/"),
        ]);
        assert!(scan(private_default.path()).unwrap()[0].version.is_none());
    }
    #[test]
    fn uv_root_default_index_and_legacy_index_url_block_pin_fallback() {
        for (path, config) in [
            ("pyproject.toml", "[[tool.uv.index]]\nname='private'\nurl='https://private.test/simple'\ndefault=true"),
            ("pyproject.toml", "[tool.uv]\nindex-url='https://private.test/simple'"),
            ("uv.toml", "[[index]]\nurl='https://private.test/simple'\ndefault=true"),
        ] {
            let dir = fixture(&[("backend/pyproject.toml", "[project]\ndependencies=['demo==1.2.3']"), (path, config)]);
            let deps = scan(dir.path()).unwrap();
            assert_eq!(deps.len(), 1);
            assert!(deps[0].version.is_none());
            assert!(deps[0].evidence.contains("source/index override"));
        }
        let public = fixture(&[("pyproject.toml", "[project]\ndependencies=['demo==1.2.3']\n[[tool.uv.index]]\nurl='https://pypi.org/simple'\ndefault=true")]);
        assert_eq!(
            scan(public.path()).unwrap()[0].version.as_deref(),
            Some("1.2.3")
        );
    }
}
