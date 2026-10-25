//! #11/#14/#15/#18/#21, R-7/R-10/R-12/R-16: independent consumer journeys.
//!
//! Retained 46-probe baseline mapping (labels from /tmp/rtfm-ux-audit.py):
//! version, help <root/init/add/discover/deps/sources/update/update-if-due/versions/
//! use/path/search/search-code/show/status>, bare, typo -> help_and_parser_contracts.
//! missing deps path -> config_selection::dependency_scope_and_invalid_paths_have_context.
//! init/add/update/sources/versions/match -> cli::local_tracer_searches_docs_and_full_source_offline.
//! no match -> ux_cli::diagnostics_distinguish_empty_coverage_from_nonmatching_content.
//! OR semantics -> missing_version_recovers_using_only_structured_actions; unknown versions/docs, disabled
//! update/docs/pinned docs/code -> config_selection::disabled_and_unknown_targets_fail_consistently.
//! code context -> ux_cli::inspection_omits_rank_and_human_code_output_numbers_real_lines.
//! empty ref accepted/after empty ref/invalid selector add/update -> config_selection::invalid_mutations_preserve_configuration_bytes.
//! broken stdout preparation/closed stdout -> cli_contract::closed_stdout_is_success_without_panic.
//! custom init/add/printed update -> cli_contract::mutation_action_executes_with_original_scope_and_argument_boundaries.
//! project add/use default scope/init -> nested_project_actions_and_environment_scope.
//! Additional gates below execute error-recovery argv without parsing human messages.
use serde_json::Value;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn command(root: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_rtfm"));
    c.current_dir(root)
        .env_remove("RTFM_CONFIG")
        .env_remove("RTFM_CACHE_DIR")
        .env("XDG_CONFIG_HOME", root.join("user"))
        .env("XDG_CACHE_HOME", root.join("cache"));
    c
}
fn envelope(output: &Output, success: bool) -> Value {
    assert_eq!(
        output.status.success(),
        success,
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "invalid machine output: {error}; {}",
            String::from_utf8_lossy(&output.stdout)
        )
    });
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["ok"], success);
    value
}
fn machine(root: &Path, args: &[&str]) -> Value {
    envelope(
        &command(root)
            .args(["--output", "json"])
            .args(args)
            .output()
            .unwrap(),
        true,
    )
}
fn execute_action(root: &Path, action: &Value) {
    let argv = action["argv"].as_array().expect("action argv");
    assert_eq!(argv[0], "rtfm");
    let output = command(root)
        .current_dir(action["cwd"].as_str().expect("action cwd"))
        .args(
            argv.iter()
                .skip(1)
                .map(|value| value.as_str().expect("argv string")),
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "action {action}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
#[test]
fn missing_version_recovers_using_only_structured_actions() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let config = root.join("config ' $scope.toml");
    let cache = root.join("cache ' $scope");
    let source = root.join("source ' $scope");
    fs::create_dir(&source).unwrap();
    fs::write(
        source.join("guide.md"),
        "# Recovery\nstructured recovery evidence\n",
    )
    .unwrap();
    let scoped = [
        "--config",
        config.to_str().unwrap(),
        "--cache-dir",
        cache.to_str().unwrap(),
    ];
    let invoke = |args: &[&str], success| {
        envelope(
            &command(root)
                .args(["--output", "json"])
                .args(scoped)
                .args(args)
                .output()
                .unwrap(),
            success,
        )
    };
    invoke(&["init"], true);
    invoke(
        &["add", "demo", source.to_str().unwrap(), "--kind", "local"],
        true,
    );
    invoke(&["use", "demo", "--ref", "stable"], true);
    let missing = invoke(&["search", "recovery", "--library", "demo"], false);
    assert_eq!(missing["error"]["code"], "version_missing");
    assert_eq!(missing["error"]["details"]["source"], "demo");
    assert_eq!(missing["error"]["details"]["reference"], "stable");
    let actions = missing["next_actions"].as_array().unwrap();
    assert!(
        actions.iter().any(|action| action["argv"]
            .as_array()
            .unwrap()
            .iter()
            .any(|arg| arg == "update")),
        "missing acquisition action: {missing}"
    );
    for action in actions {
        let argv = action["argv"].as_array().unwrap();
        assert!(argv
            .windows(2)
            .any(|pair| pair[0] == "--config" && pair[1] == config.to_str().unwrap()));
        assert!(argv
            .windows(2)
            .any(|pair| pair[0] == "--cache-dir" && pair[1] == cache.to_str().unwrap()));
        execute_action(root, action);
    }
    let found = invoke(
        &["search", "recovery", "--library", "demo", "--diagnostics"],
        true,
    );
    assert_eq!(found["data"]["results"].as_array().unwrap().len(), 1);
    assert_eq!(found["data"]["diagnostics"]["cached_sources"], 1);
    let alternatives = invoke(
        &["search", "recovery nonexistentword", "--library", "demo"],
        true,
    );
    assert_eq!(
        alternatives["data"]["results"][0]["id"],
        found["data"]["results"][0]["id"]
    );
    let no_matches = invoke(&["search", "nonexistentword", "--library", "demo"], true);
    assert!(no_matches["data"]["results"].as_array().unwrap().is_empty());
}
#[test]
fn nested_project_actions_and_environment_scope() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let project = root.join("project ' $scope");
    let nested = project.join("nested");
    let source = root.join("upstream");
    fs::create_dir_all(&nested).unwrap();
    fs::create_dir(&source).unwrap();
    fs::write(source.join("guide.md"), "# Scope\nproject scope evidence\n").unwrap();
    machine(root, &["init"]);
    let user_before = fs::read(root.join("user/rtfm/config.toml")).unwrap();
    let added = envelope(
        &command(root)
            .current_dir(&project)
            .args([
                "--output",
                "json",
                "add",
                "projectonly",
                source.to_str().unwrap(),
                "--kind",
                "local",
                "--project",
            ])
            .output()
            .unwrap(),
        true,
    );
    assert_eq!(
        added["data"]["destination"],
        project.join(".rtfm.toml").to_str().unwrap()
    );
    let selected = envelope(
        &command(root)
            .current_dir(&nested)
            .args(["--output", "json", "use", "projectonly", "--ref", "stable"])
            .output()
            .unwrap(),
        true,
    );
    assert_eq!(
        selected["data"]["destination"],
        project.join(".rtfm.toml").to_str().unwrap()
    );
    for action in selected["next_actions"].as_array().unwrap() {
        execute_action(root, action);
    }
    assert_eq!(
        fs::read(root.join("user/rtfm/config.toml")).unwrap(),
        user_before
    );
    let found = envelope(
        &command(root)
            .current_dir(&nested)
            .args(["--output", "json", "search", "evidence"])
            .output()
            .unwrap(),
        true,
    );
    assert_eq!(found["data"]["results"].as_array().unwrap().len(), 1);
    let external = root.join("environment.toml");
    let created = envelope(
        &command(root)
            .current_dir(&nested)
            .env("RTFM_CONFIG", &external)
            .args(["--output", "json", "init"])
            .output()
            .unwrap(),
        true,
    );
    assert_eq!(created["data"]["destination"], external.to_str().unwrap());
    let conflict = command(root)
        .env("RTFM_CONFIG", &external)
        .args([
            "--output",
            "json",
            "add",
            "conflict",
            source.to_str().unwrap(),
            "--kind",
            "local",
            "--project",
        ])
        .output()
        .unwrap();
    assert_eq!(
        envelope(&conflict, false)["error"]["code"],
        "scope_conflict"
    );
    let explicit = root.join("explicit.toml");
    let created = envelope(
        &command(root)
            .env("RTFM_CONFIG", &external)
            .args([
                "--output",
                "json",
                "--config",
                explicit.to_str().unwrap(),
                "init",
            ])
            .output()
            .unwrap(),
        true,
    );
    assert_eq!(created["data"]["destination"], explicit.to_str().unwrap());
}
#[test]
fn help_and_parser_contracts() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    for args in [
        vec!["--output", "json", "--help"],
        vec!["--output=json", "--version"],
        vec!["--help"],
        vec!["-h"],
        vec![],
    ] {
        let output = command(root).args(&args).output().unwrap();
        assert!(output.status.success(), "{args:?}");
        assert!(output.stderr.is_empty());
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(
            !text.contains("schema_version"),
            "help/version must be ordinary text: {args:?}"
        );
        assert!(!text.trim().is_empty());
    }
    for name in [
        "init",
        "add",
        "discover",
        "deps",
        "sources",
        "update",
        "update-if-due",
        "versions",
        "use",
        "path",
        "search",
        "search-code",
        "show",
        "status",
    ] {
        for args in [vec![name, "--help"], vec!["help", name]] {
            let output = command(root)
                .env("COLUMNS", "45")
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "help {name}");
            assert!(output.stderr.is_empty());
            assert!(String::from_utf8_lossy(&output.stdout).contains("Usage:"));
        }
    }
    for args in [
        vec!["--output=json", "serch", "needle"],
        vec!["--output", "json", "search"],
        vec!["search", "needle", "--output", "json", "--top-k", "101"],
        vec![
            "--output",
            "json",
            "search-code",
            "needle",
            "--context",
            "21",
        ],
    ] {
        let output = command(root).args(&args).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stderr.is_empty());
        assert_eq!(
            envelope(&output, false)["error"]["code"],
            "argument_invalid"
        );
    }
    let output = command(root).args(["search", "--help"]).output().unwrap();
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.contains("1..100") || help.contains("1–100") || help.contains("1-100"));
}
