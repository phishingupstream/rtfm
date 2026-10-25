//! One offline selection policy shared by lookup and coverage reporting.
use crate::{
    config::Config,
    store::{Store, Version},
};
use anyhow::Result;

pub fn resolve(
    config: &Config,
    store: &Store,
    library: Option<&str>,
    reference: Option<&str>,
) -> Result<Vec<Version>> {
    resolve_versions(config, store.versions(library)?, library, reference)
}
pub fn resolve_versions(
    config: &Config,
    mut available: Vec<Version>,
    library: Option<&str>,
    reference: Option<&str>,
) -> Result<Vec<Version>> {
    if let Some(name) = library {
        config.source(name)?;
    }
    let mut selected = Vec::new();
    for (name, source) in &config.sources {
        if !source.enabled || library.is_some_and(|target| target != name) {
            continue;
        }
        let requested = config.selected_reference(name, reference)?;
        if let Some(index) = available.iter().position(|v| {
            v.library == *name && (v.revision == requested || (v.version == requested && v.current))
        }) {
            selected.push(available.remove(index));
        } else if library.is_some() || (reference.is_none() && config.selections.contains_key(name))
        {
            return Err(crate::output::Failure::new("version_missing", format!("selected version {requested} is not cached for {name}; inspect `rtfm versions {name}`; select a fetchable ref with `rtfm use {name} --ref REF` then explicitly run `rtfm update {name}`"), serde_json::json!({"source":name,"reference":requested})).into());
        }
    }
    if let Some(reference) = reference.filter(|_| selected.is_empty()) {
        return Err(crate::output::Failure::new(
            "version_missing",
            format!(
                "version {} is not cached in the selected enabled sources; inspect `rtfm versions`",
                reference
            ),
            serde_json::json!({"reference":reference}),
        )
        .into());
    }
    Ok(selected)
}
