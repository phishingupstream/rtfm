//! R-6, R-7, R-13, R-14: rejected mutations preserve existing configuration.
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};
fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rtfm"))
        .args([
            "--config",
            root.join("config.toml").to_str().unwrap(),
            "--cache-dir",
            root.join("cache").to_str().unwrap(),
        ])
        .args(args)
        .output()
        .unwrap()
}
#[test]
fn invalid_mutations_preserve_configuration_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    assert!(run(root, &["init"]).status.success());
    assert!(run(
        root,
        &["add", "demo", root.to_str().unwrap(), "--kind", "local"]
    )
    .status
    .success());
    let before = fs::read(root.join("config.toml")).unwrap();
    for args in [
        vec!["use", "demo", "--ref", ""],
        vec!["use", "demo", "--ref", "   "],
        vec!["add", "bad", "/tmp", "--kind", "local", "--docs", "["],
        vec!["add", "bad", "/tmp", "--kind", "local", "--code", "["],
        vec![
            "add",
            "bad",
            "/tmp",
            "--kind",
            "archive",
            "--checksum",
            "sha256:abc",
        ],
        vec!["add", "demo", "/tmp", "--kind", "local"],
        vec!["init"],
    ] {
        let output = run(root, &args);
        assert!(!output.status.success(), "accepted {args:?}");
        assert_eq!(
            fs::read(root.join("config.toml")).unwrap(),
            before,
            "changed bytes for {args:?}"
        );
    }
}

#[test]
fn malformed_configuration_has_typed_errors_in_reads_and_mutations() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let config = root.join("config.toml");
    for text in [
        "[sources.",
        "unknown_setting=true\n",
        "[sources.demo]\nkind='local'\n",
        "[sources.demo]\nkind='local'\nlocation='/tmp'\nrefs=[]\n",
        "[sources.demo]\nkind='local'\nlocation=''\n",
        "[sources.demo]\nkind='local'\nlocation='   '\n",
        "[sources.'bad/name']\nkind='local'\nlocation='/tmp'\n",
        "[sources.demo]\nkind='local'\nlocation='/tmp'\ndocs=['[']\n",
    ] {
        fs::write(&config, text).unwrap();
        for args in [
            vec!["--output", "json", "sources"],
            vec![
                "--output", "json", "add", "added", "/tmp", "--kind", "local",
            ],
            vec!["--output", "json", "use", "demo", "--ref", "HEAD"],
        ] {
            let output = run(root, &args);
            assert_eq!(output.status.code(), Some(1));
            let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(value["error"]["code"], "config_invalid", "{text}: {value}");
            assert_eq!(
                value["error"]["details"]["config"],
                config.to_str().unwrap()
            );
            assert_eq!(fs::read_to_string(&config).unwrap(), text);
        }
    }
    fs::remove_file(&config).unwrap();
    let missing = run(root, &["--output", "json", "sources"]);
    let value: serde_json::Value = serde_json::from_slice(&missing.stdout).unwrap();
    assert_eq!(value["error"]["code"], "io_failed");
}

#[test]
fn inherited_parse_error_identifies_its_own_configuration() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::create_dir_all(root.join("user/rtfm")).unwrap();
    let user = root.join("user/rtfm/config.toml");
    fs::write(&user, "unknown_setting=true\n").unwrap();
    fs::write(root.join(".rtfm.toml"), "").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_rtfm"))
        .current_dir(root)
        .env_remove("RTFM_CONFIG")
        .env("XDG_CONFIG_HOME", root.join("user"))
        .arg("--cache-dir")
        .arg(root.join("cache"))
        .args(["--output", "json", "sources"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["error"]["code"], "config_invalid");
    assert_eq!(value["error"]["details"]["config"], user.to_str().unwrap());
}

#[test]
fn source_origins_preserve_project_overrides_and_inherited_definitions() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::create_dir_all(root.join("user/rtfm")).unwrap();
    let user = root.join("user/rtfm/config.toml");
    let project = root.join(".rtfm.toml");
    fs::write(&user, "[sources.shared]\nkind='local'\nlocation='shared'\n[sources.overridden]\nkind='local'\nlocation='base'\n").unwrap();
    fs::write(&project, "[sources.overridden]\nkind='local'\nlocation='override'\nenabled=false\n[sources.project]\nkind='local'\nlocation='project'\n").unwrap();
    let output = |json: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rtfm"));
        command
            .current_dir(root)
            .env_remove("RTFM_CONFIG")
            .env("XDG_CONFIG_HOME", root.join("user"))
            .arg("--cache-dir")
            .arg(root.join("cache"))
            .arg("sources");
        if json {
            command.arg("--json");
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    };
    let rows: serde_json::Value = serde_json::from_slice(&output(true)).unwrap();
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 3);
    for row in rows {
        let expected = if row["name"] == "shared" {
            &user
        } else {
            &project
        };
        assert_eq!(row["config_origin"], expected.to_str().unwrap());
        assert_eq!(row["cached_versions"], 0);
    }
    assert_eq!(
        rows.iter().find(|r| r["name"] == "overridden").unwrap()["enabled"],
        false
    );
    let human = String::from_utf8(output(false)).unwrap();
    assert_eq!(
        human
            .matches(&format!("config: {}", project.display()))
            .count(),
        2
    );
    assert_eq!(
        human
            .matches(&format!("config: {}", user.display()))
            .count(),
        1
    );
}

#[test]
fn concurrent_additions_are_not_lost() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    assert!(run(root, &["init"]).status.success());
    let children: Vec<_> = ["first", "second", "third", "fourth"]
        .into_iter()
        .map(|name| {
            Command::new(env!("CARGO_BIN_EXE_rtfm"))
                .args([
                    "--config",
                    root.join("config.toml").to_str().unwrap(),
                    "add",
                    name,
                    root.to_str().unwrap(),
                    "--kind",
                    "local",
                ])
                .spawn()
                .unwrap()
        })
        .collect();
    for mut child in children {
        assert!(child.wait().unwrap().success());
    }
    let value: toml::Value =
        toml::from_str(&fs::read_to_string(root.join("config.toml")).unwrap()).unwrap();
    assert_eq!(value["sources"].as_table().unwrap().len(), 4);
}
#[test]
fn disabled_and_unknown_targets_fail_consistently() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::write(
        root.join("config.toml"),
        "[sources.demo]\nkind='local'\nlocation='/tmp'\nenabled=false\n",
    )
    .unwrap();
    for name in ["demo", "unknown"] {
        for args in [
            vec!["search", "needle", "--library", name],
            vec!["search", "needle", "--library", name, "--version", "HEAD"],
            vec!["search-code", "needle", "--library", name],
            vec!["path", name],
            vec!["versions", name],
            vec!["update", name],
        ] {
            let output = run(root, &args);
            assert!(!output.status.success(), "accepted {args:?}");
        }
    }
}
#[test]
fn package_bindings_and_source_selectors_validate_before_init() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let catalog = root.join("catalog.toml");
    for text in [
        "[sources.demo]\nkind='local'\nlocation='/tmp'\nexclude=['[']\n",
        "[sources.demo]\nkind='local'\nlocation='/tmp'\nrefs=[' ']\n",
        "[packages.'npm:demo@1.0.0'.docs]\nsource='missing'\nreference='HEAD'\nevidence='declared'\n",
        "[packages.'npm:demo@^1'.docs]\nsource='demo'\nreference='HEAD'\nevidence='declared'\n[sources.demo]\nkind='local'\nlocation='/tmp'\n",
    ] {
        fs::write(&catalog, text).unwrap();
        assert!(!run(root, &["init", "--source", catalog.to_str().unwrap()]).status.success());
        assert!(!root.join("config.toml").exists());
        assert!(!root.join("cache/index.sqlite3").exists());
    }
}
#[test]
fn repeated_selection_is_idempotent_and_uncached_is_distinct_from_unknown() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    assert!(run(root, &["init"]).status.success());
    assert!(run(
        root,
        &["add", "demo", root.to_str().unwrap(), "--kind", "local"]
    )
    .status
    .success());
    assert!(run(root, &["use", "demo", "--ref", "stable"])
        .status
        .success());
    let before = fs::read(root.join("config.toml")).unwrap();
    assert!(run(root, &["use", "demo", "--ref", "stable"])
        .status
        .success());
    assert_eq!(fs::read(root.join("config.toml")).unwrap(), before);
    for (name, code) in [("demo", "version_missing"), ("unknown", "source_unknown")] {
        let output = run(
            root,
            &[
                "--output",
                "json",
                "search",
                "needle",
                "--library",
                name,
                "--diagnostics",
            ],
        );
        assert!(!output.status.success());
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["error"]["code"], code);
    }
}
#[test]
fn dependency_scope_and_invalid_paths_have_context() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    assert!(run(root, &["init"]).status.success());
    let empty = root.join("empty");
    fs::create_dir(&empty).unwrap();
    let output = run(root, &["--output", "json", "deps", empty.to_str().unwrap()]);
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(value.to_string().contains(empty.to_str().unwrap()));
    fs::write(empty.join("package.json"), "{").unwrap();
    let output = run(root, &["deps", empty.to_str().unwrap()]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("package.json"));
    let missing = root.join("missing");
    let output = run(root, &["deps", missing.to_str().unwrap()]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains(missing.to_str().unwrap()));
}

#[test]
fn project_mutations_preserve_interval_inheritance_and_explicit_default() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let user = root.join("user/rtfm");
    let project = root.join("project");
    fs::create_dir_all(&user).unwrap();
    fs::create_dir(&project).unwrap();
    let user_config = user.join("config.toml");
    fs::write(&user_config, "update_interval_seconds = 12345\n").unwrap();
    let project_config = project.join(".rtfm.toml");
    fs::write(&project_config, "# minimal project overlay\n").unwrap();
    let invoke = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_rtfm"))
            .current_dir(&project)
            .env_remove("RTFM_CONFIG")
            .env_remove("RTFM_CACHE_DIR")
            .env("XDG_CONFIG_HOME", root.join("user"))
            .arg("--cache-dir")
            .arg(root.join("cache"))
            .args(args)
            .output()
            .unwrap()
    };
    assert!(
        invoke(&["add", "demo", project.to_str().unwrap(), "--kind", "local"])
            .status
            .success()
    );
    assert!(invoke(&["use", "demo", "--ref", "stable"]).status.success());
    let absent: toml::Value =
        toml::from_str(&fs::read_to_string(&project_config).unwrap()).unwrap();
    assert!(
        absent.get("update_interval_seconds").is_none(),
        "mutation froze inherited interval: {absent}"
    );
    assert_eq!(
        fs::read_to_string(&user_config).unwrap(),
        "update_interval_seconds = 12345\n"
    );
    fs::write(
        project.join("guide.md"),
        "# Inheritance\ninterval coverage evidence\n",
    )
    .unwrap();
    assert!(invoke(&["update", "demo"]).status.success());
    let age_checks = || {
        let db = rusqlite::Connection::open(root.join("cache/index.sqlite3")).unwrap();
        db.execute("UPDATE update_checks SET checked_at=unixepoch()-1000", [])
            .unwrap();
    };
    age_checks();
    let output = invoke(&["--output", "json", "update", "demo", "--if-due"]);
    assert!(output.status.success());
    let inherited: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        inherited["data"]["updated"].as_array().unwrap().is_empty(),
        "inherited long interval was ignored: {inherited}"
    );
    // Explicit 900 remains an override even though it equals the built-in fallback.
    let text = fs::read_to_string(&project_config).unwrap();
    fs::write(
        &project_config,
        format!("update_interval_seconds = 900\n{text}"),
    )
    .unwrap();
    assert!(invoke(&["use", "demo", "--ref", "preview"])
        .status
        .success());
    let explicit: toml::Value =
        toml::from_str(&fs::read_to_string(&project_config).unwrap()).unwrap();
    assert_eq!(explicit["update_interval_seconds"].as_integer(), Some(900));
    age_checks();
    let output = invoke(&["--output", "json", "update", "demo", "--if-due"]);
    assert!(output.status.success());
    let overridden: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        !overridden["data"]["updated"].as_array().unwrap().is_empty(),
        "explicit default interval did not override inherited value: {overridden}"
    );
}

// R-7/R-9/R-17: importing a relative catalog must not rebase its sources.
#[test]
fn relative_catalog_import_preserves_local_source_location() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::create_dir_all(root.join("catalog/docs")).unwrap();
    fs::create_dir(root.join("destination")).unwrap();
    fs::write(root.join("catalog/docs/guide.md"), "catalogtoken").unwrap();
    fs::write(
        root.join("catalog/sources.toml"),
        "[sources.demo]\nkind='local'\nlocation='docs'\n",
    )
    .unwrap();
    let invoke = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_rtfm"))
            .current_dir(root)
            .args([
                "--config",
                "destination/config.toml",
                "--cache-dir",
                "cache",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    };
    invoke(&["init", "--source", "catalog/sources.toml"]);
    invoke(&["update", "demo"]);
    let result = invoke(&["search", "catalogtoken", "--library", "demo", "--json"]);
    let hits: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(hits[0]["text"], "catalogtoken\n");
    assert_eq!(
        fs::read_to_string(root.join("catalog/docs/guide.md")).unwrap(),
        "catalogtoken"
    );
}
