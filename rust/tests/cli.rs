//! R-1, R-3 through R-11: the local-files tracer exercises real cache and FTS5.
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};
use tempfile::TempDir;

#[test]
fn limited_code_search_keeps_complete_ranges_at_eof_and_context_boundaries() {
    let fixture = Fixture::new();
    for length in [119, 120, 121, 257] {
        fs::write(fixture.source.join("src/api.rs"), "🦀x\r\n".repeat(length)).unwrap();
        fixture.ok(&["update", "demo"]);
        for context in [0, 1, 20] {
            for regex in [false, true] {
                let context_arg = context.to_string();
                let query = if regex { "^🦀x$" } else { "🦀x" };
                let mut args = vec![
                    "--output",
                    "json",
                    "search-code",
                    query,
                    "--library",
                    "demo",
                    "--limit",
                    "1",
                    "--context",
                    &context_arg,
                ];
                if regex {
                    args.push("--regex");
                }
                let result = fixture.json(&args);
                assert_eq!(result["data"]["has_more"], length > 120);
                let hits = result["data"]["results"].as_array().unwrap();
                assert_eq!(hits.len(), 1);
                let end = length.min(120);
                let matched = if length <= 120 { length } else { 120 - context };
                assert_eq!(hits[0]["start_line"], 1);
                assert_eq!(hits[0]["end_line"], end);
                assert_eq!(hits[0]["excerpt_end_line"], end);
                assert_eq!(hits[0]["text_truncated"], false);
                assert_eq!(hits[0]["text"], vec!["🦀x"; end].join("\n"));
                assert_eq!(
                    hits[0]["match_lines"],
                    serde_json::json!((1..=matched).collect::<Vec<_>>())
                );
                let spans = hits[0]["excerpt_spans"].as_array().unwrap();
                assert_eq!(spans.len(), end);
                for (i, span) in spans.iter().enumerate() {
                    assert_eq!(
                        span,
                        &serde_json::json!({"line":i + 1,"start_byte":0,"end_byte":5})
                    );
                }
            }
        }
    }
}

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
            "# Dependency injection\nUse solve_dependencies to resolve middleware dependencies.\n",
        )
        .unwrap();
        fs::write(
            source.join("src/api.rs"),
            "pub fn hidden_identifier() { /* middleware */ }\n",
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
            .args([
                "--config",
                self.temp.path().join("config.toml").to_str().unwrap(),
                "--cache-dir",
                self.temp.path().join("cache").to_str().unwrap(),
            ])
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
fn local_tracer_searches_docs_and_full_source_offline() {
    let fixture = Fixture::new();
    // Session hooks do not populate a catalog the user has not selected to cache.
    fixture.ok(&["update-if-due"]);
    assert_eq!(fixture.json(&["status", "--json"])["chunks"], 0);
    fixture.ok(&["update", "demo"]);
    let hits = fixture.json(&["search", "solve_dependencies", "--json"]);
    assert_eq!(hits.as_array().unwrap().len(), 1);
    assert_eq!(hits[0]["library"], "demo");
    assert_eq!(hits[0]["version"], "stable");
    assert_eq!(hits[0]["start_line"], 1);
    // Natural topic queries retain useful matches when some context words are absent.
    assert_eq!(
        fixture.json(&[
            "search",
            "solve_dependencies unfamiliar_context_word",
            "--json"
        ])[0]["library"],
        "demo"
    );
    assert_eq!(
        fixture.json(&["search", "hidden_identifier", "--json"]),
        serde_json::json!([])
    );
    let code = fixture.json(&["search-code", "hidden_identifier", "--json"]);
    assert_eq!(code[0]["path"], "src/api.rs");
    let id = hits[0]["id"].as_i64().unwrap().to_string();
    assert_eq!(
        fixture.json(&["show", &id, "--json"])["text"],
        hits[0]["text"]
    );
    // Source disappears; search and cached source paths remain usable.
    fs::remove_dir_all(&fixture.source).unwrap();
    assert_eq!(
        fixture.json(&["search", "solve_dependencies", "--json"])[0]["revision"],
        hits[0]["revision"]
    );
    let path = fixture.ok(&["path", "demo"]);
    assert!(PathBuf::from(path.trim()).join("guide.md").is_file());
    assert!(fixture.run(&["update", "demo"]).status.code().unwrap() != 0);
    assert_eq!(
        fixture.json(&["search", "solve_dependencies", "--json"])[0]["revision"],
        hits[0]["revision"]
    );
}
#[test]
fn updates_retain_versions_and_failed_updates_keep_previous_search() {
    let fixture = Fixture::new();
    fixture.ok(&["update", "demo"]);
    let initial = fixture.json(&["versions", "demo", "--json"]);
    let revision = initial[0]["revision"].as_str().unwrap();
    fs::write(
        fixture.source.join("guide.md"),
        "# Replacement\nA new unrelated document about strawberry resolvers.\n",
    )
    .unwrap();
    fixture.ok(&["update", "demo"]);
    let versions = fixture.json(&["versions", "demo", "--json"]);
    assert_eq!(versions.as_array().unwrap().len(), 2);
    assert_eq!(
        fixture.json(&["search", "solve_dependencies", "--json"]),
        serde_json::json!([])
    );
    let old = fixture.json(&[
        "search",
        "solve_dependencies",
        "--version",
        revision,
        "--json",
    ]);
    assert_eq!(old[0]["revision"], revision);
    assert!(fixture.json(&["search", "resolvers", "--json"])[0].is_object());
    fixture.ok(&["use", "demo", "--ref", revision]);
    assert_eq!(
        fixture.json(&["search", "solve_dependencies", "--json"])[0]["revision"],
        revision
    );
    fixture.ok(&["update", "demo"]);
    assert_eq!(
        fixture.json(&["search", "solve_dependencies", "--json"])[0]["revision"],
        revision
    );
    assert!(fixture.run(&["search", "!!!"]).status.code().unwrap() != 0);
    assert!(
        fixture
            .run(&["search", "anything", "--top-k", "0"])
            .status
            .code()
            .unwrap()
            != 0
    );
}
#[test]
fn due_checks_are_quiet_and_selected_versions_are_visible() {
    let fixture = Fixture::new();
    fixture.ok(&["update", "demo"]);
    fixture.ok(&["use", "demo", "--ref", "preview"]);
    fixture.ok(&["update", "demo"]);
    let versions = fixture.json(&["versions", "demo", "--json"]);
    assert!(versions
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v["version"] == "preview"));
    let hits = fixture.json(&["search", "solve_dependencies", "--json"]);
    assert_eq!(hits.as_array().unwrap().len(), 1);
    assert_eq!(hits[0]["version"], "preview");
    let due = fixture.run(&["update-if-due"]);
    assert!(due.status.success());
    assert!(due.stdout.is_empty());
    assert!(due.stderr.is_empty());
    assert_eq!(
        fixture.json(&["sources", "--json"])[0]["selected"],
        "preview"
    );
}

#[test]
fn unchanged_updates_preserve_chunk_ids_and_project_config_overlays_user_catalog() {
    let fixture = Fixture::new();
    fixture.ok(&["update", "demo"]);
    let before = fixture.json(&["search", "solve_dependencies", "--json"]);
    fixture.ok(&["update", "demo"]);
    assert_eq!(
        fixture.json(&["search", "solve_dependencies", "--json"])[0]["id"],
        before[0]["id"]
    );
    fixture.ok(&["use", "demo", "--ref", "preview"]);
    fixture.ok(&["update", "demo"]);
    let config_home = fixture.temp.path().join("user-config");
    fs::create_dir_all(config_home.join("rtfm")).unwrap();
    fs::copy(
        fixture.temp.path().join("config.toml"),
        config_home.join("rtfm/config.toml"),
    )
    .unwrap();
    let projects: Vec<_> = ["stable", "preview"]
        .into_iter()
        .map(|reference| {
            let project = fixture.temp.path().join(reference);
            fs::create_dir(&project).unwrap();
            fs::write(
                project.join(".rtfm.toml"),
                format!("[selections]\ndemo = {reference:?}\n"),
            )
            .unwrap();
            (project, reference)
        })
        .collect();
    for (project, reference) in projects {
        let output = Command::new(env!("CARGO_BIN_EXE_rtfm"))
            .env_remove("RTFM_CONFIG")
            .env("XDG_CONFIG_HOME", &config_home)
            .env("RTFM_CACHE_DIR", fixture.temp.path().join("cache"))
            .current_dir(project)
            .args(["search", "solve_dependencies", "--json"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let hits: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(hits[0]["version"], reference);
        let output = Command::new(env!("CARGO_BIN_EXE_rtfm"))
            .env_remove("RTFM_CONFIG")
            .env("XDG_CONFIG_HOME", &config_home)
            .env("RTFM_CACHE_DIR", fixture.temp.path().join("cache"))
            .current_dir(fixture.temp.path().join(reference))
            .args(["search-code", "hidden_identifier", "--json"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let hits: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(hits[0]["version"], reference);
    }
}

#[test]
fn changed_snapshots_reuse_unchanged_files_and_scope_changes_reindex() {
    let fixture = Fixture::new();
    fixture.ok(&["update", "demo"]);
    fs::write(fixture.source.join("same.md"), "unchanged documentation").unwrap();
    fixture.ok(&["update", "demo"]);
    fs::write(
        fixture.source.join("guide.md"),
        "# Changed\nNew configuration prose.\n",
    )
    .unwrap();
    let update = fixture.run(&["update", "demo"]);
    assert!(update.status.success());
    assert!(String::from_utf8_lossy(&update.stderr).contains("Reused 1 unchanged file(s)"));
    assert_eq!(
        fixture.json(&["search-code", "hidden_identifier", "--json"])[0]["path"],
        "src/api.rs"
    );
    let path = fixture.temp.path().join("config.toml");
    let mut config: toml::Value = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    config["sources"]["demo"]["exclude"] =
        toml::Value::Array(vec![toml::Value::String("src/**".into())]);
    fs::write(path, toml::to_string_pretty(&config).unwrap()).unwrap();
    fixture.ok(&["update", "demo"]);
    assert_eq!(
        fixture.json(&["search-code", "hidden_identifier", "--json"]),
        serde_json::json!([])
    );
}

#[test]
fn failed_due_checks_are_throttled_without_hiding_cache() {
    let fixture = Fixture::new();
    fixture.ok(&["update", "demo"]);
    let db = rusqlite::Connection::open(fixture.temp.path().join("cache/index.sqlite3")).unwrap();
    db.execute("UPDATE versions SET updated_at=0", []).unwrap();
    db.execute("DELETE FROM update_checks", []).unwrap();
    fs::remove_dir_all(&fixture.source).unwrap();
    assert!(!fixture.run(&["update-if-due"]).status.success());
    let again = fixture.run(&["update-if-due"]);
    assert!(again.status.success());
    assert!(again.stderr.is_empty());
    assert!(again.stdout.is_empty());
    assert_eq!(
        fixture.json(&["search", "solve_dependencies", "--json"])[0]["library"],
        "demo"
    );
}

#[test]
fn uncached_version_selections_fail_explicitly() {
    let fixture = Fixture::new();
    fixture.ok(&["update", "demo"]);
    let missing = fixture.run(&[
        "search",
        "solve_dependencies",
        "--version",
        "missing",
        "--json",
    ]);
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("not cached"));
    fixture.ok(&["use", "demo", "--ref", "missing"]);
    assert!(!fixture
        .run(&["search", "solve_dependencies", "--json"])
        .status
        .success());
    fixture.ok(&["update", "demo"]);
    assert_eq!(
        fixture.json(&["search", "solve_dependencies", "--json"])[0]["version"],
        "missing"
    );
}

#[test]
fn git_source_searches_pinned_versions_and_recursive_submodules() {
    let fixture = Fixture::new();
    let child = fixture.temp.path().join("child-repository");
    let parent = fixture.temp.path().join("parent-repository");
    for directory in [&child, &parent] {
        fs::create_dir(directory).unwrap();
        let output = Command::new("git")
            .args(["init", "-b", "main"])
            .arg(directory)
            .output()
            .unwrap();
        assert!(output.status.success());
        for setting in [
            ["user.name", "Fixture"],
            ["user.email", "fixture@example.invalid"],
        ] {
            assert!(Command::new("git")
                .arg("-C")
                .arg(directory)
                .args(["config", setting[0], setting[1]])
                .status()
                .unwrap()
                .success());
        }
    }
    fs::write(
        child.join("guide.md"),
        "# Submodule\nsubmoduletracer explains nested checkout behavior.\n",
    )
    .unwrap();
    git_ok(&child, &["add", "."]);
    git_ok(&child, &["commit", "-qm", "nested docs"]);
    fs::write(
        parent.join("guide.md"),
        "# Stable\nstabletracer appears in the pinned release.\n",
    )
    .unwrap();
    git_ok(&parent, &["add", "."]);
    git_ok(&parent, &["commit", "-qm", "first release"]);
    git_ok(&parent, &["tag", "v1"]);
    git_ok(
        &parent,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            child.to_str().unwrap(),
            "nested",
        ],
    );
    fs::write(
        parent.join("guide.md"),
        "# Current\ncurrenttracer appears on the tracked branch.\n",
    )
    .unwrap();
    git_ok(&parent, &["add", "."]);
    git_ok(&parent, &["commit", "-qm", "add nested docs"]);
    fixture.ok(&[
        "add",
        "repo",
        parent.to_str().unwrap(),
        "--ref",
        "HEAD",
        "--ref",
        "tag:v1",
    ]);
    let output = Command::new(env!("CARGO_BIN_EXE_rtfm"))
        .env("GIT_ALLOW_PROTOCOL", "file")
        .args([
            "--config",
            fixture.temp.path().join("config.toml").to_str().unwrap(),
            "--cache-dir",
            fixture.temp.path().join("cache").to_str().unwrap(),
            "update",
            "repo",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fixture.json(&["search", "submoduletracer", "--library", "repo", "--json"])[0]["path"],
        "nested/guide.md"
    );
    assert_eq!(
        fixture.json(&[
            "search",
            "stabletracer",
            "--library",
            "repo",
            "--version",
            "tag:v1",
            "--json"
        ])[0]["version"],
        "tag:v1"
    );
    fixture.ok(&["use", "repo", "--ref", "tag:v1"]);
    assert_eq!(
        fixture.json(&["search", "stabletracer", "--library", "repo", "--json"])[0]["version"],
        "tag:v1"
    );
    assert_eq!(
        fixture.json(&["search", "currenttracer", "--library", "repo", "--json"]),
        serde_json::json!([])
    );
}
fn git_ok(directory: &std::path::Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn code_search_matches_raw_lines_merges_context_and_filters_versions_offline() {
    let fixture = Fixture::new();
    // A token spanning the former 2400-byte chunk boundary remains searchable.
    let long = format!("{}boundary::member(value);", "x".repeat(2395));
    let source = format!("before\nalpha::partial_name(value);\nbetween\nalpha::partial_name(other);\nafter\n{long}\n");
    fs::write(fixture.source.join("src/api.rs"), source).unwrap();
    fixture.ok(&["update", "demo"]);
    let old = fixture.json(&["versions", "demo", "--json"])[0]["revision"]
        .as_str()
        .unwrap()
        .to_owned();
    let merged = fixture.json(&["search-code", "::partial_", "--context", "1", "--json"]);
    assert_eq!(merged.as_array().unwrap().len(), 1);
    assert_eq!(merged[0]["start_line"], 1);
    assert_eq!(merged[0]["end_line"], 5);
    assert_eq!(merged[0]["match_lines"], serde_json::json!([2, 4]));
    assert_eq!(merged[0]["version"], "stable");
    assert!(merged[0]["provenance"].as_str().unwrap().contains("local"));
    assert!(PathBuf::from(merged[0]["local_path"].as_str().unwrap()).is_file());
    assert_eq!(
        fixture.json(&[
            "search-code",
            "boundary::member(",
            "--context",
            "0",
            "--json"
        ])[0]["start_line"],
        6
    );
    assert_eq!(
        fixture.json(&[
            "search-code",
            r"alpha::partial_name\(other\)",
            "--regex",
            "--context",
            "0",
            "--json"
        ])[0]["start_line"],
        4
    );
    assert_eq!(
        fixture.json(&[
            "search-code",
            "::partial_",
            "--path",
            "src/missing*",
            "--json"
        ]),
        serde_json::json!([])
    );
    assert_eq!(
        fixture.json(&[
            "search-code",
            "::partial_",
            "--library",
            "demo",
            "--path",
            "src/*.rs",
            "--json"
        ])[0]["path"],
        "src/api.rs"
    );
    let invalid = fixture.run(&["search-code", "[", "--regex"]);
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("regex"));
    fs::write(fixture.source.join("src/api.rs"), "replacement_symbol();\n").unwrap();
    fixture.ok(&["update", "demo"]);
    fs::remove_dir_all(&fixture.source).unwrap();
    assert_eq!(
        fixture.json(&["search-code", "::partial_", "--json"]),
        serde_json::json!([])
    );
    assert_eq!(
        fixture.json(&["search-code", "::partial_", "--version", &old, "--json"])[0]["revision"],
        old
    );
    fixture.ok(&["use", "demo", "--ref", &old]);
    assert_eq!(
        fixture.json(&["search-code", "::partial_", "--json"])[0]["revision"],
        old
    );
}

#[test]
fn legacy_code_chunks_never_enter_docs_search_and_update_rebuilds_docs_only() {
    let fixture = Fixture::new();
    fixture.ok(&["update", "demo"]);
    let db = rusqlite::Connection::open(fixture.temp.path().join("cache/index.sqlite3")).unwrap();
    db.execute("INSERT INTO chunks(library,revision,kind,path,heading,start_line,end_line,text) SELECT library,revision,'code','src/api.rs','',1,1,'legacyonlysymbol' FROM indexed", []).unwrap();
    db.execute(
        "UPDATE indexed SET signature='legacy-chunking-signature'",
        [],
    )
    .unwrap();
    assert_eq!(
        fixture.json(&["search", "legacyonlysymbol", "--json"]),
        serde_json::json!([])
    );
    fixture.ok(&["update", "demo"]);
    let code_count: i64 = db
        .query_row("SELECT count(*) FROM chunks WHERE kind='code'", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(code_count, 0);
    assert_eq!(fixture.json(&["status", "--json"])["chunks"], 1);
}

#[test]
fn code_search_enforces_scope_text_boundaries_and_input_contracts() {
    let fixture = Fixture::new();
    fs::write(fixture.source.join("src/another.rs"), "scopeprobe();\n").unwrap();
    fs::write(fixture.source.join("src/api.rs"), "scopeprobe();\n").unwrap();
    fs::write(fixture.source.join("src/excluded.rs"), "scopeprobe();\n").unwrap();
    fs::write(fixture.source.join("unselected.py"), "scopeprobe();\n").unwrap();
    fs::write(fixture.source.join("src/binary.rs"), b"scopeprobe\0").unwrap();
    fs::write(fixture.source.join("src/invalid.rs"), b"scopeprobe\xff").unwrap();
    let path = fixture.temp.path().join("config.toml");
    let mut config: toml::Value = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    config["sources"]["demo"]["exclude"] =
        toml::Value::Array(vec![toml::Value::String("src/excluded.rs".into())]);
    fs::write(path, toml::to_string_pretty(&config).unwrap()).unwrap();
    fixture.ok(&["update", "demo"]);
    let hits = fixture.json(&["search-code", "scopeprobe", "--json"]);
    assert_eq!(hits.as_array().unwrap().len(), 2);
    assert_eq!(hits[0]["path"], "src/another.rs");
    assert_eq!(hits[1]["path"], "src/api.rs");
    assert_eq!(
        fixture
            .json(&["search-code", "scopeprobe", "--top-k", "1", "--json"])
            .as_array()
            .unwrap()
            .len(),
        1
    );
    for args in [
        vec!["search-code", ""],
        vec!["search-code", "scope\nprobe"],
        vec!["search-code", "scopeprobe", "--top-k", "0"],
        vec!["search-code", "scopeprobe", "--path", "["],
    ] {
        assert!(!fixture.run(&args).status.success(), "{args:?}");
    }
    let missing = fixture.run(&["search-code", "scopeprobe", "--version", "missing"]);
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("not cached"));
    let migration = fixture.run(&["search", "scopeprobe", "--code"]);
    assert!(!migration.status.success());
    assert!(String::from_utf8_lossy(&migration.stderr).contains("search-code"));
}

#[test]
fn discover_human_commands_match_json_next_actions() {
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let registry = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(&mut stream);
            loop {
                let mut line = String::new();
                assert_ne!(
                    reader.read_line(&mut line).unwrap(),
                    0,
                    "incomplete HTTP headers"
                );
                if line == "\r\n" {
                    break;
                }
            }
            let body = r#"{"info":{"version":"1.2.3"},"urls":[{"packagetype":"sdist","url":"https://example.test/demo.tar.gz"}]}"#;
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
        }
    });
    let temp = tempfile::tempdir().unwrap();
    let run = |json: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rtfm"));
        command
            .current_dir(temp.path())
            .env("RTFM_PYPI_REGISTRY", &registry)
            .args(["discover", "pypi:demo@1.2.3"]);
        if json {
            command.arg("--json");
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    let human = run(false);
    let json: Value = serde_json::from_str(&run(true)).unwrap();
    server.join().unwrap();
    assert!(human.contains(&format!(
        "source: {}",
        json["source_name"].as_str().unwrap()
    )));
    for action in json["next_actions"].as_array().unwrap() {
        let command = action["argv"]
            .as_array()
            .unwrap()
            .iter()
            .map(|arg| arg.as_str().unwrap())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(human.lines().any(|line| line == command), "{human}");
    }
    assert_eq!(json["next_actions"].as_array().unwrap().len(), 2);
}

#[test]
fn discover_missing_ecosystem_reports_supported_prefixes() {
    let output = Command::new(env!("CARGO_BIN_EXE_rtfm"))
        .args(["discover", "tokio@1.53.1"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("use pypi:, npm:, or crates.io:"));
}
