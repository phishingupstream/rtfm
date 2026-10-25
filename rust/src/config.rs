use anyhow::{bail, Context, Result};
use base64::Engine;
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, Deserialize, Serialize, ValueEnum, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    Git,
    Local,
    Url,
    Archive,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub kind: SourceKind,
    pub location: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checksum: Option<String>,
    #[serde(default = "default_refs")]
    pub refs: Vec<String>,
    #[serde(default = "default_docs")]
    pub docs: Vec<String>,
    #[serde(default)]
    pub code: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default = "enabled")]
    pub enabled: bool,
}
fn enabled() -> bool {
    true
}
pub fn default_refs() -> Vec<String> {
    vec!["HEAD".into()]
}
pub fn default_docs() -> Vec<String> {
    [
        "**/*.md",
        "**/*.mdx",
        "**/*.rst",
        "**/*.txt",
        "**/*.html",
        "**/*.htm",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}
fn default_interval() -> u64 {
    900
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub source: String,
    pub reference: String,
    pub evidence: String,
}
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackageMapping {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs: Option<Binding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<Binding>,
}
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_interval_seconds: Option<u64>,
    #[serde(default)]
    pub sources: BTreeMap<String, Source>,
    #[serde(default)]
    pub selections: BTreeMap<String, String>,
    #[serde(default)]
    pub packages: BTreeMap<String, PackageMapping>,
}
impl Config {
    pub fn update_interval_seconds(&self) -> u64 {
        self.update_interval_seconds
            .unwrap_or_else(default_interval)
    }
    pub fn load(path: &Path) -> Result<Self> {
        let mut config = Self::load_raw(path)?;
        config = config.effective(path)?;
        Ok(config)
    }
    pub fn effective(mut self, path: &Path) -> Result<Self> {
        let user = user_config_path();
        if path.file_name().is_some_and(|n| n == ".rtfm.toml") && user != path && user.is_file() {
            let mut base = Self::load_raw(&user)?;
            base.sources.extend(self.sources);
            base.selections.extend(self.selections);
            for (key, mapping) in self.packages {
                let inherited = base.packages.entry(key).or_default();
                if mapping.docs.is_some() {
                    inherited.docs = mapping.docs;
                }
                if mapping.code.is_some() {
                    inherited.code = mapping.code;
                }
            }
            if self.update_interval_seconds.is_some() {
                base.update_interval_seconds = self.update_interval_seconds;
            }
            self = base;
        }
        self.validate()
            .map_err(|error| invalid_config(path, error))?;
        Ok(self)
    }
    pub fn validate(&self) -> Result<()> {
        for (name, source) in &self.sources {
            validate_name(name).with_context(|| format!("source {name}"))?;
            if source.location.trim().is_empty()
                || source.refs.is_empty()
                || source.refs.iter().any(|r| r.trim().is_empty())
            {
                bail!("source {name}: location and refs must be nonempty");
            }
            for (role, patterns) in [
                ("docs", &source.docs),
                ("code", &source.code),
                ("exclude", &source.exclude),
            ] {
                for pattern in patterns {
                    if pattern.trim().is_empty() {
                        bail!("source {name}: {role} selector must be nonempty");
                    }
                    globset::Glob::new(pattern).with_context(|| {
                        format!("source {name}: invalid {role} selector {pattern:?}")
                    })?;
                }
            }
            if let Some(checksum) = &source.checksum {
                if source.kind != SourceKind::Archive {
                    bail!("source {name}: checksum is supported only for archive sources");
                }
                let valid = if let Some(hex) = checksum.strip_prefix("sha256:") {
                    hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit())
                } else if let Some(encoded) = checksum
                    .strip_prefix("sha512-base64:")
                    .or_else(|| checksum.strip_prefix("sha512-"))
                {
                    base64::engine::general_purpose::STANDARD
                        .decode(encoded)
                        .is_ok_and(|bytes| bytes.len() == 64)
                } else {
                    false
                };
                if !valid {
                    bail!("source {name}: invalid checksum; use sha256:<64 hex digits> or sha512-base64:<64 encoded bytes>");
                }
            }
        }
        for (name, reference) in &self.selections {
            if !self.sources.contains_key(name) {
                bail!("selection {name}: unknown source");
            }
            if reference.trim().is_empty() {
                bail!("selection {name}: ref must be nonempty");
            }
        }
        for (key, mapping) in &self.packages {
            crate::registry::PackageSpec::parse(key)
                .with_context(|| format!("package binding {key}"))?;
            for binding in [&mapping.docs, &mapping.code].into_iter().flatten() {
                if !self.sources.contains_key(&binding.source) {
                    bail!("package binding {key}: unknown source {}", binding.source);
                }
                if binding.reference.trim().is_empty() || binding.evidence.trim().is_empty() {
                    bail!("package binding {key}: reference and evidence must be nonempty");
                }
            }
        }
        Ok(())
    }
    pub fn source(&self, name: &str) -> Result<&Source> {
        let source = self.sources.get(name).ok_or_else(|| {
            crate::output::Failure::new(
                "source_unknown",
                format!("unknown source {name}; inspect `rtfm sources`"),
                serde_json::json!({"source":name}),
            )
        })?;
        if !source.enabled {
            return Err(crate::output::Failure::new(
                "source_disabled",
                format!(
                    "source {name} is disabled; enable it in configuration before selecting it"
                ),
                serde_json::json!({"source":name}),
            )
            .into());
        }
        Ok(source)
    }
    pub fn selected_reference<'a>(
        &'a self,
        name: &str,
        explicit: Option<&'a str>,
    ) -> Result<&'a str> {
        let source = self.source(name)?;
        Ok(explicit
            .or_else(|| self.selections.get(name).map(String::as_str))
            .unwrap_or(&source.refs[0]))
    }
    pub fn source_origins(path: &Path) -> Result<impl Fn(&str) -> PathBuf> {
        let local = Self::load_raw(path)?.sources;
        let path = path.to_owned();
        let user = user_config_path();
        Ok(move |name: &str| {
            if local.contains_key(name) {
                path.clone()
            } else {
                user.clone()
            }
        })
    }
    pub fn load_raw(path: &Path) -> Result<Self> {
        let path = if path.is_absolute() {
            path.to_owned()
        } else {
            env::current_dir()?.join(path)
        };
        let text = fs::read_to_string(&path).with_context(|| {
            format!(
                "read {} (run `rtfm init` to create a config)",
                path.display()
            )
        })?;
        let mut config: Self =
            toml::from_str(&text).map_err(|error| invalid_config(&path, error))?;
        for source in config.sources.values_mut() {
            // Leave invalid empty locations intact for effective validation.
            if source.location.trim().is_empty() {
                continue;
            }
            if source.kind == SourceKind::Local
                || (source.kind == SourceKind::Archive
                    && !source.location.starts_with("http://")
                    && !source.location.starts_with("https://"))
                || (source.kind == SourceKind::Git
                    && !source.location.contains(':')
                    && !source.location.starts_with("/"))
            {
                let location = Path::new(&source.location);
                if location.is_relative() {
                    source.location = path
                        .parent()
                        .unwrap_or(Path::new("."))
                        .join(location)
                        .to_string_lossy()
                        .into_owned();
                }
            }
        }
        Ok(config)
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        let text = toml::to_string_pretty(self)?;
        toml::from_str::<Self>(&text)?.effective(path)?;
        save_text(path, &text)
    }
}
fn invalid_config(path: &Path, error: impl std::fmt::Display) -> crate::output::Failure {
    crate::output::Failure::new(
        "config_invalid",
        format!("invalid configuration {}: {error:#}", path.display()),
        serde_json::json!({"config":path}),
    )
}
fn save_text(path: &Path, text: &str) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    use std::io::Write;
    file.write_all(text.as_bytes())?;
    crate::cache::check_interrupted()?;
    file.persist(path).map_err(|e| e.error)?;
    Ok(())
}
pub fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        bail!("source names must contain only letters, digits, '-' and '_'");
    }
    Ok(())
}
pub fn config_path() -> PathBuf {
    if let Some(path) = env::var_os("RTFM_CONFIG") {
        return path.into();
    }
    let mut cursor = env::current_dir().ok();
    while let Some(dir) = cursor {
        let path = dir.join(".rtfm.toml");
        if path.is_file() {
            return path;
        }
        cursor = dir.parent().map(Path::to_path_buf);
    }
    user_config_path()
}
pub fn user_config_path() -> PathBuf {
    env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"))
        .join("rtfm/config.toml")
}
pub fn cache_path() -> PathBuf {
    env::var_os("RTFM_CACHE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            env::var_os("XDG_CACHE_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home().join(".cache"))
                .join("rtfm")
        })
}
fn home() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Hold across the complete read/modify/publish transaction.
pub struct ConfigLock(std::fs::File);
impl ConfigLock {
    pub fn acquire(path: &Path) -> Result<Self> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::create_dir_all(parent)?;
        let mut lock_name = path.as_os_str().to_owned();
        lock_name.push(".lock");
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(PathBuf::from(lock_name))?;
        fs2::FileExt::lock_exclusive(&file)?;
        Ok(Self(file))
    }
}
impl Drop for ConfigLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.0);
    }
}
