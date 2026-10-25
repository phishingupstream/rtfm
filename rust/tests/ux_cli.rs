//! Offline CLI contracts for package filtering and actionable inspection output.
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};
use tempfile::TempDir;

struct Fixture {
    temp: TempDir,
    source: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("upstream");
        fs::create_dir_all(source.join("src")).unwrap();
        fs::write(
            source.join("guide.md"),
            "# Guide\nuxdocumentation explains the API.\n",
        )
        .unwrap();
        fs::write(
            source.join("src/api.rs"),
            "before();\nux_identifier();\nafter();\n",
        )
        .unwrap();
        let fixture = Self { temp, source };
        fixture.ok(&["init"]);
        fixture.ok(&[
            "add",
            "demo",
            fixture.source.to_str().unwrap(),
            "--kind",
            "local",
            "--ref",
            "stable",
            "--code",
            "src/**/*.rs",
        ]);
        fixture
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_rtfm"))
            .env_remove("RTFM_CONFIG")
            .env("XDG_CONFIG_HOME", self.temp.path().join("user-config"))
            .current_dir(self.temp.path())
            .arg("--config")
            .arg(self.temp.path().join("config.toml"))
            .arg("--cache-dir")
            .arg(self.temp.path().join("cache"))
            .args(args)
            .output()
            .unwrap()
    }
    fn ok(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }
    fn json(&self, args: &[&str]) -> Value {
        serde_json::from_str(&self.ok(args)).unwrap()
    }
}

#[test]
fn deps_filters_normalized_packages_scopes_and_exact_versions_offline() {
    let f = Fixture::new();
    let project = f.temp.path().join("project");
    fs::create_dir(&project).unwrap();
    fs::write(
        project.join("package.json"),
        r#"{"dependencies":{"@scope/pkg":"1.2.3","another":"2.0.0"}}"#,
    )
    .unwrap();
    fs::write(
        project.join("pyproject.toml"),
        "[project]\nname='fixture'\nversion='0.1.0'\ndependencies=['My_Package.Name==4.5.6']\n",
    )
    .unwrap();
    let path = project.to_str().unwrap();
    for (filter, package, version) in [
        ("npm:@scope/pkg", "npm:@scope/pkg", "1.2.3"),
        ("npm:@scope/pkg@1.2.3", "npm:@scope/pkg", "1.2.3"),
        ("pypi:MY_package.NAME", "pypi:my-package-name", "4.5.6"),
    ] {
        let rows = f.json(&["deps", path, "--package", filter, "--json"]);
        assert_eq!(rows.as_array().unwrap().len(), 1);
        assert_eq!(rows[0]["package"], package);
        assert_eq!(rows[0]["version"], version);
        assert_eq!(rows[0]["docs"]["status"], "unmapped");
    }
    assert_eq!(
        f.json(&["deps", path, "--package", "npm:@scope/pkg@9.0.0", "--json"]),
        json!([])
    );
    for malformed in ["pkg", "npm:@scope", "npm:pkg@", "pypi:pkg@^1", "pip:pkg"] {
        let output = f.run(&["deps", path, "--package", malformed, "--json"]);
        assert!(!output.status.success(), "{malformed}");
        assert!(!output.stderr.is_empty());
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn diagnostics_distinguish_empty_coverage_from_nonmatching_content() {
    let f = Fixture::new();
    let empty = f.json(&["search", "absentword", "--json", "--diagnostics"]);
    assert_eq!(empty["results"], json!([]));
    assert_eq!(empty["diagnostics"]["status"], "no_cached_sources");
    f.ok(&["update", "demo"]);
    assert!(f.json(&["search", "uxdocumentation", "--json"]).is_array());
    let found = f.json(&["search", "uxdocumentation", "--json", "--diagnostics"]);
    assert_eq!(found["results"].as_array().unwrap().len(), 1);
    assert_eq!(found["diagnostics"]["status"], "matches");
    let absent = f.json(&["search", "absentword", "--json", "--diagnostics"]);
    assert_eq!(absent["results"], json!([]));
    assert_eq!(absent["diagnostics"]["status"], "no_matches");
    assert!(
        absent["diagnostics"]["documentation_chunks"]
            .as_u64()
            .unwrap()
            > 0
    );
    let no_files = f.json(&[
        "search-code",
        "ux_identifier",
        "--path",
        "missing/**",
        "--json",
        "--diagnostics",
    ]);
    assert_eq!(no_files["results"], json!([]));
    assert_eq!(no_files["diagnostics"]["status"], "no_source_files");
    let no_code_match = f.json(&["search-code", "absentword", "--json", "--diagnostics"]);
    assert_eq!(no_code_match["diagnostics"]["status"], "no_matches");
    for command in ["search", "search-code"] {
        assert_eq!(
            f.ok(&[command, "absentword"]).trim(),
            "No matches in cached content; refine the query or path filter."
        );
    }
    f.ok(&[
        "add",
        "codeonly",
        f.source.to_str().unwrap(),
        "--kind",
        "local",
        "--docs",
        "missing/**/*.md",
        "--code",
        "src/**/*.rs",
    ]);
    f.ok(&["update", "codeonly"]);
    let no_docs = f.json(&[
        "search",
        "uxdocumentation",
        "--library",
        "codeonly",
        "--json",
        "--diagnostics",
    ]);
    assert_eq!(no_docs["results"], json!([]));
    assert_eq!(no_docs["diagnostics"]["status"], "no_documentation");
    assert_eq!(no_docs["diagnostics"]["code_available"], true);
    assert!(no_docs["diagnostics"]["suggestion"]
        .as_str()
        .unwrap()
        .contains("search-code"));
    assert_eq!(
        f.json(&[
            "search",
            "uxdocumentation",
            "--library",
            "codeonly",
            "--json"
        ]),
        json!([])
    );
}

#[test]
fn inspection_omits_rank_and_human_code_output_numbers_real_lines() {
    let f = Fixture::new();
    f.ok(&["update", "demo"]);
    let hits = f.json(&["search", "uxdocumentation", "--json"]);
    let id = hits[0]["id"].as_i64().unwrap().to_string();
    let shown = f.json(&["show", &id, "--json"]);
    assert!(shown.get("score").is_none());
    for key in [
        "text",
        "revision",
        "path",
        "provenance",
        "local_path",
        "start_line",
        "end_line",
    ] {
        assert_eq!(shown[key], hits[0][key], "{key}");
    }
    let snippet = f.ok(&["search-code", "ux_identifier", "--context", "1"]);
    assert!(
        snippet.contains("1: before();\n2: ux_identifier();\n3: after();"),
        "{snippet}"
    );
    let missing = f.run(&["path", "demo", "--version", "missing-release"]);
    assert!(!missing.status.success());
    let error = String::from_utf8_lossy(&missing.stderr);
    for detail in [
        "missing-release",
        "rtfm versions demo",
        "rtfm use demo --ref",
        "rtfm update demo",
    ] {
        assert!(error.contains(detail), "missing {detail:?}: {error}");
    }
}

#[test]
fn help_explains_package_discovery_and_custom_source_bindings() {
    let f = Fixture::new();
    let discover = f.ok(&["discover", "--help"]);
    for detail in [
        "pypi:",
        "npm:@",
        "crates.io:",
        "--json",
        "release correspondence",
    ] {
        assert!(discover.contains(detail), "missing {detail:?}: {discover}");
    }
    let add = f.ok(&["add", "--help"]);
    for detail in [
        "--package",
        "--role",
        "--code",
        "--project",
        "--kind",
        "LOCATION",
        "configuration only",
    ] {
        assert!(add.contains(detail), "missing {detail:?}: {add}");
    }
    // Invalid discovery specs fail during validation, before registry access.
    let invalid = f.run(&["discover", "npm:@scope/pkg", "--json"]);
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("exact"));
}
