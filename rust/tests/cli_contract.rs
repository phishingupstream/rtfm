//! R-16: consumers use versioned outcomes and argv without parsing prose.
use serde_json::Value;
use std::{
    fs,
    process::{Command, Stdio},
};
fn command(root: &std::path::Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_rtfm"));
    c.args(["--output", "json", "--config"])
        .arg(root.join("config with spaces.toml"))
        .arg("--cache-dir")
        .arg(root.join("cache"));
    c
}
fn json(c: &mut Command) -> Value {
    let o = c.output().unwrap();
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["schema_version"], 1);
    v
}
#[test]
fn mutation_action_executes_with_original_scope_and_argument_boundaries() {
    let root = tempfile::tempdir().unwrap();
    let upstream = root.path().join("source ' $text");
    fs::create_dir(&upstream).unwrap();
    fs::write(
        upstream.join("guide.md"),
        "# Guide\nfixture contract evidence\n",
    )
    .unwrap();
    assert_eq!(
        json(command(root.path()).arg("init"))["data"]["changed"],
        true
    );
    let added = json(
        command(root.path())
            .args(["add", "fixture"])
            .arg(&upstream)
            .args(["--kind", "local"]),
    );
    assert_eq!(added["ok"], true);
    let action = &added["next_actions"][0];
    let mut next = Command::new(env!("CARGO_BIN_EXE_rtfm"));
    next.args(
        action["argv"]
            .as_array()
            .unwrap()
            .iter()
            .skip(1)
            .map(|v| v.as_str().unwrap()),
    );
    next.current_dir(action["cwd"].as_str().unwrap());
    assert!(next.output().unwrap().status.success());
    let found = json(command(root.path()).args(["search", "evidence"]));
    assert_eq!(found["data"]["results"].as_array().unwrap().len(), 1);
    assert_eq!(found["data"]["has_more"], false);
    let unchanged = json(command(root.path()).args(["update", "fixture"]));
    assert_eq!(unchanged["data"]["changed"], false);
    let missing = json(command(root.path()).args(["search", "x", "--library", "unknown"]));
    assert_eq!(missing["error"]["code"], "source_unknown");
}
#[test]
fn parser_errors_and_help_are_process_contracts() {
    let root = tempfile::tempdir().unwrap();
    let o = command(root.path())
        .args(["search", "x", "--top-k", "0"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2));
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["error"]["code"], "argument_invalid");
    assert!(o.stderr.is_empty());
    let bare = Command::new(env!("CARGO_BIN_EXE_rtfm")).output().unwrap();
    assert!(bare.status.success());
    assert!(String::from_utf8_lossy(&bare.stdout).contains("Usage:"));
}
#[cfg(unix)]
#[test]
fn closed_stdout_is_success_without_panic() {
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;
    let root = tempfile::tempdir().unwrap();
    assert_eq!(json(command(root.path()).arg("init"))["ok"], true);
    let upstream = root.path().join("upstream");
    fs::create_dir(&upstream).unwrap();
    assert_eq!(
        json(
            command(root.path())
                .args(["add", "fixture"])
                .arg(&upstream)
                .args(["--kind", "local"])
        )["ok"],
        true
    );
    for args in [vec!["sources"], vec!["sources", "--json"]] {
        let (reader, writer) = UnixStream::pair().unwrap();
        drop(reader);
        let fd: OwnedFd = writer.into();
        let o = Command::new(env!("CARGO_BIN_EXE_rtfm"))
            .arg("--config")
            .arg(root.path().join("config with spaces.toml"))
            .arg("--cache-dir")
            .arg(root.path().join("cache"))
            .args(args)
            .stdout(Stdio::from(fd))
            .output()
            .unwrap();
        assert!(o.status.success());
        assert!(o.stderr.is_empty());
    }
}

#[test]
fn inventories_explain_selectors_and_distinguish_current_from_selected() {
    let root = tempfile::tempdir().unwrap();
    let upstream = root.path().join("upstream");
    fs::create_dir(&upstream).unwrap();
    fs::write(
        upstream.join("guide.md"),
        "# Guide\nversion inventory evidence\n",
    )
    .unwrap();
    json(command(root.path()).arg("init"));
    json(
        command(root.path())
            .args(["add", "fixture"])
            .arg(&upstream)
            .args([
                "--kind",
                "local",
                "--ref",
                "one",
                "--ref",
                "two",
                "--docs",
                "*.md",
                "--code",
                "*.rs",
                "--exclude",
                "private/**",
            ]),
    );
    json(command(root.path()).args(["update", "fixture"]));
    json(command(root.path()).args(["use", "fixture", "--ref", "two"]));
    let sources = json(command(root.path()).arg("sources"));
    assert_eq!(sources["data"][0]["docs"], serde_json::json!(["*.md"]));
    assert_eq!(sources["data"][0]["code"], serde_json::json!(["*.rs"]));
    assert_eq!(
        sources["data"][0]["exclude"],
        serde_json::json!(["private/**"])
    );
    let versions = json(command(root.path()).args(["versions", "fixture"]));
    let rows = versions["data"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|v| v["current"] == true));
    assert_eq!(rows.iter().filter(|v| v["selected"] == true).count(), 1);
    assert_eq!(
        rows.iter().find(|v| v["selected"] == true).unwrap()["version"],
        "two"
    );
    let human = Command::new(env!("CARGO_BIN_EXE_rtfm"))
        .arg("--config")
        .arg(root.path().join("config with spaces.toml"))
        .arg("--cache-dir")
        .arg(root.path().join("cache"))
        .arg("sources")
        .output()
        .unwrap();
    let text = String::from_utf8(human.stdout).unwrap();
    assert!(text.contains("docs: [\"*.md\"]"));
    assert!(text.contains("code: [\"*.rs\"]"));
    assert!(text.contains("exclude: [\"private/**\"]"));
}

#[test]
fn dependency_envelope_identifies_requested_roots_effective_configuration() {
    let root = tempfile::tempdir().unwrap();
    let caller = root.path().join("caller");
    let target = root.path().join("target");
    fs::create_dir(&caller).unwrap();
    fs::create_dir(&target).unwrap();
    fs::write(caller.join(".rtfm.toml"), "[sources]\n").unwrap();
    fs::write(target.join(".rtfm.toml"), "[sources]\n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_rtfm"))
        .env_remove("RTFM_CONFIG")
        .env("XDG_CONFIG_HOME", root.path().join("user-config"))
        .current_dir(&caller)
        .args(["--output", "json", "--cache-dir"])
        .arg(root.path().join("cache"))
        .arg("deps")
        .arg(&target)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let result: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        result["context"]["config"],
        target.join(".rtfm.toml").to_str().unwrap()
    );
    assert_eq!(result["context"]["cwd"], caller.to_str().unwrap());
    assert_eq!(result["data"]["root"], target.to_str().unwrap());
}

#[test]
fn nested_project_selection_keeps_only_the_named_parent_definition() {
    let root = tempfile::tempdir().unwrap();
    let parent = root.path().join("parent");
    let nested = parent.join("nested");
    let source = root.path().join("source");
    fs::create_dir_all(&nested).unwrap();
    fs::create_dir(&source).unwrap();
    fs::write(parent.join(".rtfm.toml"),format!("[sources.projectonly]\nkind='local'\nlocation={:?}\nrefs=['HEAD']\n[sources.unrelated]\nkind='local'\nlocation={:?}\n",source.to_str().unwrap(),source.to_str().unwrap())).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_rtfm"))
        .env_remove("RTFM_CONFIG")
        .env("XDG_CONFIG_HOME", root.path().join("user"))
        .current_dir(&nested)
        .arg("--cache-dir")
        .arg(root.path().join("cache"))
        .args([
            "--output",
            "json",
            "use",
            "projectonly",
            "--ref",
            "HEAD",
            "--project",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let config: toml::Value =
        toml::from_str(&fs::read_to_string(nested.join(".rtfm.toml")).unwrap()).unwrap();
    assert!(config["sources"].get("projectonly").is_some());
    assert!(config["sources"].get("unrelated").is_none());
    assert_eq!(config["selections"]["projectonly"].as_str(), Some("HEAD"));
}

#[test]
fn partial_failure_retains_successful_mutation_outcomes() {
    let root = tempfile::tempdir().unwrap();
    let upstream = root.path().join("upstream");
    fs::create_dir(&upstream).unwrap();
    fs::write(upstream.join("guide.md"), "# Guide\npartial evidence\n").unwrap();
    json(command(root.path()).arg("init"));
    json(
        command(root.path())
            .args(["add", "a-good"])
            .arg(&upstream)
            .args(["--kind", "local"]),
    );
    fs::create_dir(root.path().join("absent")).unwrap();
    json(
        command(root.path())
            .args(["add", "z-bad"])
            .arg(root.path().join("absent"))
            .args(["--kind", "local"]),
    );
    fs::remove_dir(root.path().join("absent")).unwrap();
    let out = command(root.path()).arg("update").output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let result: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(result["error"]["code"], "acquisition_failed");
    assert_eq!(result["data"]["updated"][0]["source"], "a-good");
    assert_eq!(result["data"]["failures"][0]["source"], "z-bad");
    assert_eq!(result["data"]["changed"], true);
}

#[cfg(unix)]
#[test]
fn interrupted_second_source_keeps_first_publication_in_envelope() {
    use std::{
        os::unix::fs::PermissionsExt,
        time::{Duration, Instant},
    };
    let root = tempfile::tempdir().unwrap();
    let upstream = root.path().join("upstream");
    fs::create_dir(&upstream).unwrap();
    fs::write(
        upstream.join("guide.md"),
        "# Guide\npartial signal evidence\n",
    )
    .unwrap();
    json(command(root.path()).arg("init"));
    json(
        command(root.path())
            .args(["add", "a-good"])
            .arg(&upstream)
            .args(["--kind", "local"]),
    );
    json(command(root.path()).args(["add", "z-blocked", "https://fixture.invalid/repo.git"]));
    let helpers = root.path().join("helpers");
    fs::create_dir(&helpers).unwrap();
    let marker = root.path().join("started");
    let git = helpers.join("git");
    fs::write(
        &git,
        "#!/bin/sh\nprintf ready > \"$RTFM_TEST_STARTED\"\nsleep 9 &\nwait\n",
    )
    .unwrap();
    fs::set_permissions(&git, fs::Permissions::from_mode(0o755)).unwrap();
    let mut child = command(root.path())
        .arg("update")
        .env(
            "PATH",
            format!("{}:{}", helpers.display(), std::env::var("PATH").unwrap()),
        )
        .env("RTFM_TEST_STARTED", &marker)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(8);
    while !marker.exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    unsafe {
        assert_eq!(libc::kill(child.id() as i32, libc::SIGTERM), 0);
    }
    while child.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(128 + libc::SIGTERM));
    let result: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(result["error"]["code"], "interrupted");
    assert_eq!(result["data"]["updated"][0]["source"], "a-good");
    assert_eq!(result["data"]["failures"][0]["code"], "interrupted");
    let found = json(command(root.path()).args(["search", "evidence", "--library", "a-good"]));
    assert_eq!(found["data"]["results"].as_array().unwrap().len(), 1);
}

#[test]
fn positional_output_text_does_not_enable_machine_parser_errors() {
    let human = Command::new(env!("CARGO_BIN_EXE_rtfm"))
        .args(["search", "--", "--output=json", "extra"])
        .output()
        .unwrap();
    assert_eq!(human.status.code(), Some(2));
    assert!(human.stdout.is_empty());
    assert!(String::from_utf8_lossy(&human.stderr).contains("extra"));
    let machine = Command::new(env!("CARGO_BIN_EXE_rtfm"))
        .args(["--output", "json", "search", "--", "--output=json", "extra"])
        .output()
        .unwrap();
    assert_eq!(machine.status.code(), Some(2));
    assert!(machine.stderr.is_empty());
    let result: Value = serde_json::from_slice(&machine.stdout).unwrap();
    assert_eq!(result["error"]["code"], "argument_invalid");
    assert_eq!(result["context"], serde_json::json!({}));
}
