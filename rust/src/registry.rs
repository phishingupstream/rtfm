//! Explicit registry metadata lookup. Repository links are evidence, never inferred Git refs.
use anyhow::{bail, Context, Result};
use serde::Serialize;
use serde_json::Value;
use std::{io::Read, time::Duration};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PackageSpec {
    pub ecosystem: String,
    pub name: String,
    pub version: String,
}
impl PackageSpec {
    pub fn parse(input: &str) -> Result<Self> {
        let (ecosystem, package) = input
            .split_once(':')
            .context("expected ecosystem:name@exact-version; use pypi:, npm:, or crates.io:")?;
        let (name, version) = package
            .rsplit_once('@')
            .context("an exact published version is required")?;
        if !matches!(ecosystem, "pypi" | "npm" | "crates.io") {
            bail!("unsupported ecosystem {ecosystem}; expected pypi, npm, or crates.io");
        }
        if name.is_empty()
            || version.is_empty()
            || version
                .chars()
                .any(|c| c.is_whitespace() || "^~*<>=|/@".contains(c))
            || matches!(version, "latest" | "next" | "HEAD")
        {
            bail!("expected a package name and exact published version");
        }
        let valid_name = if ecosystem == "npm" && name.starts_with('@') {
            let parts: Vec<_> = name[1..].split('/').collect();
            parts.len() == 2
                && parts.iter().all(|s| {
                    !s.is_empty()
                        && s.chars()
                            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
                })
        } else {
            name.chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        };
        if !valid_name {
            bail!("invalid {ecosystem} package name {name}");
        }
        Ok(Self {
            ecosystem: ecosystem.into(),
            name: if ecosystem == "pypi" {
                regex::Regex::new("[-_.]+")?
                    .replace_all(&name.to_ascii_lowercase(), "-")
                    .into_owned()
            } else {
                name.into()
            },
            version: version.into(),
        })
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct Candidate {
    pub kind: String,
    pub url: String,
    pub version: Option<String>,
    /// Algorithm-prefixed digest; npm integrity retains its published SRI encoding.
    pub checksum: Option<String>,
    pub paths_status: String,
    pub release_mapping_status: String,
    pub repository_root: Option<String>,
    pub subdirectory: Option<String>,
    pub repository_path_hint: Option<String>,
    pub docs_paths: Vec<String>,
    pub code_paths: Vec<String>,
    pub evidence: Vec<String>,
}
#[derive(Debug, Serialize)]
pub struct Discovery {
    pub package: PackageSpec,
    pub candidates: Vec<Candidate>,
    pub source_name: String,
    pub next_actions: Vec<NextAction>,
    pub evidence: Vec<String>,
}
pub fn source_name(package: &PackageSpec) -> String {
    use sha2::{Digest, Sha256};
    let key = format!("{}:{}@{}", package.ecosystem, package.name, package.version);
    format!(
        "pkg-{}-{}-{}",
        package.ecosystem.replace('.', "-"),
        package
            .name
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect::<String>(),
        &format!("{:x}", Sha256::digest(key.as_bytes()))[..12]
    )
}
#[derive(Debug, Serialize)]
pub struct NextAction {
    pub action: String,
    pub argv: Vec<String>,
}
fn normalize_directory(directory: &str) -> Option<String> {
    let mut parts = Vec::new();
    for part in directory.split('/') {
        match part {
            "" | "." => (),
            ".." => return None,
            _ if part.contains('\\') || part.chars().any(char::is_control) => return None,
            _ => parts.push(part),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}
fn endpoint(base: &str, segments: &[&str]) -> Result<String> {
    let mut url = url::Url::parse(base)?;
    url.path_segments_mut()
        .map_err(|_| anyhow::anyhow!("registry endpoint must be a base URL"))?
        .pop_if_empty()
        .extend(segments);
    Ok(url.into())
}
fn string<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}
fn candidate(
    package: &PackageSpec,
    kind: &str,
    url: &str,
    checksum: Option<String>,
    evidence: &str,
) -> Candidate {
    let extensions: &[&str] = match package.ecosystem.as_str() {
        "pypi" => &["py", "pyi", "pyx", "pxd", "c", "h"],
        "npm" => &["js", "jsx", "ts", "tsx", "mjs", "cjs", "vue", "svelte"],
        _ => &["rs"],
    };
    Candidate {
        kind: kind.into(),
        url: url.into(),
        version: (kind == "artifact").then(|| package.version.clone()),
        checksum,
        paths_status: "suggested_unverified".into(),
        release_mapping_status: if kind == "artifact" {
            "published_exact_version"
        } else {
            "unverified"
        }
        .into(),
        repository_root: (kind == "repository").then(|| url.into()),
        subdirectory: None,
        repository_path_hint: None,
        docs_paths: ["**/*.md", "**/*.mdx", "**/*.rst", "**/*.txt", "**/*.html"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        code_paths: extensions.iter().map(|ext| format!("**/*.{ext}")).collect(),
        evidence: vec![evidence.into()],
    }
}
fn add_link(out: &mut Discovery, kind: &str, url: Option<&str>, evidence: &str) {
    if let Some(raw) = url {
        let normalized = if kind == "repository" {
            raw.strip_prefix("git+")
                .unwrap_or(raw)
                .trim_end_matches('/')
        } else {
            raw
        };
        let mut items = vec![evidence.to_string()];
        if raw != normalized {
            items.push(format!("Published {kind} URL: {raw}"));
        }
        if let Some(existing) = out
            .candidates
            .iter_mut()
            .find(|c| c.kind == kind && c.url == normalized)
        {
            for item in items {
                if !existing.evidence.contains(&item) {
                    existing.evidence.push(item);
                }
            }
        } else {
            let mut entry = candidate(&out.package, kind, normalized, None, evidence);
            entry.evidence = items;
            if kind == "repository" {
                if let Ok(parsed) = url::Url::parse(normalized) {
                    let parts: Vec<_> = parsed.path_segments().into_iter().flatten().collect();
                    if parsed.host_str() == Some("github.com")
                        && parts.len() >= 4
                        && matches!(parts[2], "tree" | "blob")
                    {
                        entry.repository_root = Some(format!(
                            "https://github.com/{}/{}.git",
                            parts[0],
                            parts[1].trim_end_matches(".git")
                        ));
                        entry.repository_path_hint = Some(parts[3..].join("/"));
                        entry.evidence.push("GitHub browser URL: repository root extracted; ref/subdirectory boundary is unverified, so path selectors are not inferred.".into());
                    }
                }
            }
            out.candidates.push(entry);
        }
    }
}
pub fn discover(spec: &str) -> Result<Discovery> {
    let package = PackageSpec::parse(spec)?;
    let (env, default, segments) = match package.ecosystem.as_str() {
        "pypi" => (
            "RTFM_PYPI_REGISTRY",
            "https://pypi.org",
            vec![
                "pypi",
                package.name.as_str(),
                package.version.as_str(),
                "json",
            ],
        ),
        "npm" => (
            "RTFM_NPM_REGISTRY",
            "https://registry.npmjs.org",
            vec![package.name.as_str(), package.version.as_str()],
        ),
        _ => (
            "RTFM_CRATES_REGISTRY",
            "https://crates.io",
            vec![
                "api",
                "v1",
                "crates",
                package.name.as_str(),
                package.version.as_str(),
            ],
        ),
    };
    let base = std::env::var(env).unwrap_or_else(|_| default.into());
    let url = endpoint(&base, &segments)?;
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(30))
        .user_agent(concat!("rtfm/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let response = client
        .get(&url)
        .send()
        .with_context(|| format!("query {url}"))?
        .error_for_status()
        .with_context(|| format!("registry lookup {url}"))?;
    let mut bytes = Vec::new();
    response.take(8 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 8 * 1024 * 1024 {
        bail!("registry metadata exceeds 8 MiB; inspect the configured registry endpoint");
    }
    crate::cache::check_interrupted()?;
    let metadata: Value = serde_json::from_slice(&bytes).context("parse registry metadata")?;
    parse_metadata(package, &base, &url, &metadata)
}
fn parse_metadata(
    package: PackageSpec,
    base: &str,
    evidence: &str,
    metadata: &Value,
) -> Result<Discovery> {
    let mut out = Discovery {
        source_name: source_name(&package),
        next_actions: Vec::new(),
        package,
        candidates: Vec::new(),
        evidence: vec![
            evidence.into(),
            "Repository and documentation links do not establish a matching Git ref.".into(),
        ],
    };
    let info = match out.package.ecosystem.as_str() {
        "pypi" => &metadata["info"],
        "crates.io" => &metadata["version"],
        _ => metadata,
    };
    let version_key = if out.package.ecosystem == "crates.io" {
        "num"
    } else {
        "version"
    };
    if string(info, version_key) != Some(out.package.version.as_str()) {
        bail!(
            "registry response did not match requested version {}",
            out.package.version
        );
    }
    match out.package.ecosystem.as_str() {
        "pypi" => {
            if let Some(files) = metadata["urls"].as_array() {
                for file in files
                    .iter()
                    .filter(|f| string(f, "packagetype") == Some("sdist"))
                {
                    if let Some(url) = string(file, "url") {
                        let checksum =
                            string(&file["digests"], "sha256").map(|s| format!("sha256:{s}"));
                        out.candidates.push(candidate(
                            &out.package,
                            "artifact",
                            url,
                            checksum,
                            evidence,
                        ));
                    }
                }
            }
            if let Some(links) = info["project_urls"].as_object() {
                for (label, value) in links {
                    let label = label.to_ascii_lowercase();
                    let kind =
                        if label.contains("source") || label.contains("repo") || label == "code" {
                            "repository"
                        } else if label.contains("doc") {
                            "documentation"
                        } else {
                            "homepage"
                        };
                    let link_evidence = format!("{evidence}; published project_urls.{label}");
                    add_link(&mut out, kind, value.as_str(), &link_evidence);
                }
            }
            add_link(&mut out, "homepage", string(info, "home_page"), evidence);
        }
        "npm" => {
            if let Some(url) = string(&info["dist"], "tarball") {
                let checksum = string(&info["dist"], "integrity")
                    .and_then(|s| {
                        s.split_whitespace()
                            .find(|hash| hash.starts_with("sha512-"))
                    })
                    .map(String::from);
                if checksum.is_none() {
                    out.evidence.push(format!(
                        "No supported SHA-512 integrity supplied; published SHA-1 digest: {}",
                        string(&info["dist"], "shasum").unwrap_or("absent")
                    ));
                }
                out.candidates
                    .push(candidate(&out.package, "artifact", url, checksum, evidence));
            }
            let repository = info["repository"]
                .as_str()
                .or_else(|| string(&info["repository"], "url"));
            add_link(&mut out, "repository", repository, evidence);
            if let Some(directory) = string(&info["repository"], "directory") {
                if let Some(repository) = out.candidates.iter_mut().find(|c| c.kind == "repository")
                {
                    repository
                        .evidence
                        .push(format!("Published repository.directory: {directory}"));
                    // Selectors remain relative to the repository; archive payloads keep their own layout.
                    if let Some(directory) = normalize_directory(directory) {
                        repository.subdirectory = Some(directory.clone());
                        for path in repository
                            .docs_paths
                            .iter_mut()
                            .chain(repository.code_paths.iter_mut())
                        {
                            *path = format!("{directory}/{path}");
                        }
                    }
                }
            }
            add_link(&mut out, "homepage", string(info, "homepage"), evidence);
        }
        _ => {
            if let Some(path) = string(info, "dl_path") {
                let download = url::Url::parse(base)?.join(path)?.to_string();
                let checksum = string(info, "checksum").map(|s| format!("sha256:{s}"));
                out.candidates.push(candidate(
                    &out.package,
                    "artifact",
                    &download,
                    checksum,
                    evidence,
                ));
            }
            add_link(&mut out, "repository", string(info, "repository"), evidence);
            if let Some(documentation) = string(info, "documentation") {
                add_link(&mut out, "documentation", Some(documentation), evidence);
            } else {
                let documentation = format!(
                    "https://docs.rs/{}/{}/",
                    out.package.name, out.package.version
                );
                add_link(&mut out, "documentation", Some(&documentation),
                    "Conventional docs.rs version URL; availability and correspondence have not been checked.");
            }
            add_link(&mut out, "homepage", string(info, "homepage"), evidence);
        }
    }
    if !out.candidates.iter().any(|c| c.kind == "artifact") {
        out.evidence
            .push("No published source artifact was reported for this exact version.".into());
    }
    if out.candidates.iter().any(|c| c.kind == "artifact") {
        let spec = format!(
            "{}:{}@{}",
            out.package.ecosystem, out.package.name, out.package.version
        );
        out.next_actions.push(NextAction {
            action: "add_project".into(),
            argv: vec!["rtfm".into(), "add".into(), spec, "--project".into()],
        });
        out.next_actions.push(NextAction {
            action: "update".into(),
            argv: vec!["rtfm".into(), "update".into(), out.source_name.clone()],
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn github_tree_urls_expose_clone_root_without_guessing_slash_refs() {
        let data = serde_json::json!({"info":{"version":"1.0.0","project_urls":{"Source":"https://github.com/owner/mono/tree/release/v1/packages/lib"}},"urls":[]});
        let out = parse_metadata(
            PackageSpec::parse("pypi:lib@1.0.0").unwrap(),
            "https://pypi.org",
            "fixture",
            &data,
        )
        .unwrap();
        let repo = out
            .candidates
            .iter()
            .find(|c| c.kind == "repository")
            .unwrap();
        assert_eq!(
            repo.repository_root.as_deref(),
            Some("https://github.com/owner/mono.git")
        );
        assert_eq!(
            repo.repository_path_hint.as_deref(),
            Some("release/v1/packages/lib")
        );
        assert!(repo.subdirectory.is_none());
        assert!(repo.url.contains("/tree/"));
        assert_eq!(repo.release_mapping_status, "unverified");
    }
    #[test]
    fn actions_use_exact_identity_as_arguments_and_stable_name() {
        let package = PackageSpec::parse("npm:@scope/pkg@1.0.0").unwrap();
        let data = json!({"version":"1.0.0","dist":{"tarball":"https://example.org/a.tgz","integrity":"sha512-YWJj"}});
        let out = parse_metadata(package.clone(), "https://example.org", "fixture", &data).unwrap();
        assert_eq!(out.source_name, source_name(&package));
        assert_eq!(
            out.next_actions[0].argv,
            ["rtfm", "add", "npm:@scope/pkg@1.0.0", "--project"]
        );
        assert_eq!(
            out.next_actions[1].argv,
            ["rtfm", "update", out.source_name.as_str()]
        );
        let artifact = &out.candidates[0];
        assert!(artifact.code_paths.contains(&"**/*.vue".into()));
        assert!(artifact.code_paths.contains(&"**/*.svelte".into()));
        assert_eq!(artifact.paths_status, "suggested_unverified");
        assert_eq!(artifact.checksum.as_deref(), Some("sha512-YWJj"));
        let no_artifact = parse_metadata(
            package,
            "https://example.org",
            "fixture",
            &json!({"version":"1.0.0","repository":"https://example.org/repo"}),
        )
        .unwrap();
        assert!(no_artifact.next_actions.is_empty());
        assert_eq!(
            no_artifact.candidates[0].release_mapping_status,
            "unverified"
        );
    }
    #[test]
    fn repository_deduplication_keeps_published_evidence() {
        let data = json!({"info":{"version":"1","project_urls":{"Repository":"git+https://example.org/repo/","Source":"https://example.org/repo"}}});
        let out = parse_metadata(
            PackageSpec::parse("pypi:pkg@1").unwrap(),
            "https://example.org",
            "fixture",
            &data,
        )
        .unwrap();
        assert_eq!(out.candidates.len(), 1);
        assert_eq!(
            out.candidates[0].repository_root.as_deref(),
            Some("https://example.org/repo")
        );
        assert!(out.candidates[0]
            .evidence
            .iter()
            .any(|item| item.contains("git+https://example.org/repo/")));
    }
    #[test]
    fn monorepo_directory_normalization_rejects_parent_traversal() {
        assert_eq!(
            normalize_directory("./packages//./pkg/"),
            Some("packages/pkg".into())
        );
        assert_eq!(normalize_directory("packages/../pkg"), None);
        assert_eq!(normalize_directory("packages\\pkg"), None);
        let data = json!({"version":"1","repository":{"url":"git+https://example.org/repo/","directory":"./packages//pkg/"}});
        let out = parse_metadata(
            PackageSpec::parse("npm:pkg@1").unwrap(),
            "https://example.org",
            "fixture",
            &data,
        )
        .unwrap();
        let repo = &out.candidates[0];
        assert_eq!(
            repo.repository_root.as_deref(),
            Some("https://example.org/repo")
        );
        assert_eq!(repo.subdirectory.as_deref(), Some("packages/pkg"));
        assert_eq!(repo.code_paths[0], "packages/pkg/**/*.js");
        assert!(repo.version.is_none());
    }
    #[test]
    fn scoped_npm_and_exact_versions() {
        let spec = PackageSpec::parse("npm:@scope/name@1.2.3-beta.1").unwrap();
        assert_eq!(spec.name, "@scope/name");
        for invalid in [
            "npm:@scope/name",
            "npm:foo@latest",
            "pypi:foo@>=1",
            "crates.io:foo@*",
            "npm:foo@",
            "npm:../foo@1",
        ] {
            assert!(PackageSpec::parse(invalid).is_err(), "{invalid}");
        }
        assert!(
            endpoint("https://registry.npmjs.org", &["@scope/name", "1.2.3"])
                .unwrap()
                .contains("@scope%2Fname")
        );
    }
    #[test]
    fn pypi_identity_is_normalized() {
        assert_eq!(
            PackageSpec::parse("pypi:My__Package.Name@1.0")
                .unwrap()
                .name,
            "my-package-name"
        );
    }
    #[test]
    fn npm_repository_directory_scopes_only_repository_selectors() {
        let data = json!({"version":"1.0.0","dist":{"tarball":"https://example.org/pkg.tgz"},"repository":{"url":"https://example.org/repo", "directory":"packages/pkg"}});
        let out = parse_metadata(
            PackageSpec::parse("npm:pkg@1.0.0").unwrap(),
            "https://registry.npmjs.org",
            "fixture",
            &data,
        )
        .unwrap();
        let artifact = out
            .candidates
            .iter()
            .find(|c| c.kind == "artifact")
            .unwrap();
        let repository = out
            .candidates
            .iter()
            .find(|c| c.kind == "repository")
            .unwrap();
        assert_eq!(artifact.code_paths[0], "**/*.js");
        assert_eq!(repository.code_paths[0], "packages/pkg/**/*.js");
        assert_eq!(repository.docs_paths[0], "packages/pkg/**/*.md");
        assert!(repository
            .evidence
            .iter()
            .any(|e| e.contains("packages/pkg")));
    }
    #[test]
    fn pypi_selects_sdist_and_keeps_repository_unpinned() {
        let data = json!({"info":{"version":"1.0","project_urls":{"Source":"https://example.org/repo","Documentation":"https://example.org/docs"}},"urls":[{"packagetype":"bdist_wheel","url":"wheel"},{"packagetype":"sdist","url":"https://example.org/pkg.tar.gz","digests":{"sha256":"abc"}}]});
        let out = parse_metadata(
            PackageSpec::parse("pypi:pkg@1.0").unwrap(),
            "https://pypi.org",
            "fixture",
            &data,
        )
        .unwrap();
        assert_eq!(out.candidates[0].checksum.as_deref(), Some("sha256:abc"));
        assert_eq!(out.candidates[0].code_paths[0], "**/*.py");
        assert!(out
            .candidates
            .iter()
            .find(|c| c.kind == "repository")
            .unwrap()
            .version
            .is_none());
        assert_eq!(
            out.candidates
                .iter()
                .filter(|c| c.kind == "artifact")
                .count(),
            1
        );
    }
    #[test]
    fn npm_preserves_integrity_and_rejects_version_mismatch() {
        let data = json!({"version":"1.2.3","dist":{"tarball":"https://example.org/pkg.tgz","integrity":"sha512-YWJj"},"repository":{"url":"git+https://example.org/repo.git"}});
        let out = parse_metadata(
            PackageSpec::parse("npm:pkg@1.2.3").unwrap(),
            "https://registry.npmjs.org",
            "fixture",
            &data,
        )
        .unwrap();
        assert_eq!(out.candidates[0].checksum.as_deref(), Some("sha512-YWJj"));
        let repository = out
            .candidates
            .iter()
            .find(|c| c.kind == "repository")
            .unwrap();
        assert_eq!(repository.url, "https://example.org/repo.git");
        assert!(repository
            .evidence
            .iter()
            .any(|e| e.contains("git+https://")));
        assert!(parse_metadata(
            PackageSpec::parse("npm:pkg@2.0.0").unwrap(),
            "https://registry.npmjs.org",
            "fixture",
            &data
        )
        .is_err());
    }
    #[test]
    fn crate_uses_published_download_and_checksum() {
        let data = json!({"version":{"num":"1.0.0","dl_path":"/api/v1/crates/pkg/1.0.0/download","checksum":"abc","repository":"https://example.org/repo"}});
        let out = parse_metadata(
            PackageSpec::parse("crates.io:pkg@1.0.0").unwrap(),
            "https://crates.io",
            "fixture",
            &data,
        )
        .unwrap();
        assert_eq!(
            out.candidates[0].url,
            "https://crates.io/api/v1/crates/pkg/1.0.0/download"
        );
        assert_eq!(out.candidates[0].checksum.as_deref(), Some("sha256:abc"));
        let docs = out
            .candidates
            .iter()
            .find(|c| c.kind == "documentation")
            .unwrap();
        assert_eq!(docs.url, "https://docs.rs/pkg/1.0.0/");
        assert!(docs.version.is_none());
        assert!(docs.evidence[0].contains("have not been checked"));
    }
}
