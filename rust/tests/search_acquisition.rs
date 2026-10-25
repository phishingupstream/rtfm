//! R-2/R-3/R-6/R-10: original-source evidence, stable IDs, and interrupted acquisition.
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};
struct Fixture {
    temp: tempfile::TempDir,
    upstream: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let upstream = temp.path().join("upstream space 🦀");
        fs::create_dir(&upstream).unwrap();
        let this = Self { temp, upstream };
        this.ok(&["init"]);
        this.ok(&[
            "add",
            "demo",
            this.upstream.to_str().unwrap(),
            "--kind",
            "local",
            "--code",
            "**/*.rs",
        ]);
        this
    }
    fn command(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_rtfm"));
        c.args(["--config"])
            .arg(self.temp.path().join("config.toml"))
            .arg("--cache-dir")
            .arg(self.temp.path().join("cache"));
        c
    }
    fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }
    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }
    fn json(&self, args: &[&str]) -> Value {
        serde_json::from_str(&self.ok(args)).unwrap()
    }
}

#[test]
fn directory_refresh_and_exact_revision_reject_corrupt_retained_content() {
    let f = Fixture::new();
    let text = "# Guide\nverifieddirectoryhash evidence\n";
    fs::write(f.upstream.join("guide.md"), text).unwrap();
    f.ok(&["update", "demo"]);
    let before = f.json(&["search", "verifieddirectoryhash", "--json"]);
    let revision = before[0]["revision"].as_str().unwrap();
    fs::write(
        before[0]["local_path"].as_str().unwrap(),
        "corrupted snapshot",
    )
    .unwrap();
    for exact in [false, true] {
        if exact {
            f.ok(&["use", "demo", "--ref", revision]);
        }
        let versions = f.json(&["versions", "demo", "--json"]);
        let result = f.run(&["update", "demo"]);
        assert_eq!(result.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&result.stderr).contains("cached snapshot was modified"));
        assert_eq!(f.json(&["versions", "demo", "--json"]), versions);
        assert_eq!(
            f.json(&["search", "verifieddirectoryhash", "--json"]),
            before
        );
        assert_eq!(
            fs::read_to_string(f.upstream.join("guide.md")).unwrap(),
            text
        );
    }
}

#[test]
fn first_legacy_reindex_never_reuses_an_existing_chunk_id() {
    let f = Fixture::new();
    fs::write(
        f.upstream.join("guide.md"),
        "# Guide\nlegacyidentifier evidence\n",
    )
    .unwrap();
    f.ok(&["update", "demo"]);
    let db = rusqlite::Connection::open(f.temp.path().join("cache/index.sqlite3")).unwrap();
    db.execute_batch("DROP TABLE chunk_ids; UPDATE chunks SET rowid=1000;")
        .unwrap();
    assert_eq!(
        f.json(&["search", "legacyidentifier", "--json"])[0]["id"],
        1000
    );
    let config = f.temp.path().join("config.toml");
    let text = fs::read_to_string(&config)
        .unwrap()
        .replace("docs = [", "docs = [\"extra/**\", ");
    fs::write(config, text).unwrap();
    f.ok(&["update", "demo"]);
    let hits = f.json(&["search", "legacyidentifier", "--json"]);
    assert!(hits[0]["id"].as_i64().unwrap() > 1000);
    assert!(!f.run(&["show", "1000"]).status.success());
}

#[test]
fn retained_ids_never_alias_a_new_chunk_after_reindex() {
    let f = Fixture::new();
    fs::write(
        f.upstream.join("a.md"),
        "# Alpha\nalpha original evidence\n",
    )
    .unwrap();
    f.ok(&["update", "demo"]);
    let old = f.json(&["search", "alpha", "--json"]);
    let id = old[0]["id"].as_i64().unwrap().to_string();
    let revision = old[0]["revision"].as_str().unwrap();
    fs::write(f.upstream.join("a.md"), "# Beta\nbeta new evidence\n").unwrap();
    f.ok(&["update", "demo"]);
    assert!(f.json(&["show", &id, "--json"])["text"]
        .as_str()
        .unwrap()
        .contains("original"));
    assert_eq!(
        f.json(&["search", "alpha", "--version", revision, "--json"])[0]["id"],
        old[0]["id"]
    );
    let beta = f.json(&["search", "beta", "--json"]);
    let beta_id = beta[0]["id"].as_i64().unwrap().to_string();
    // Reindex the same retained revision under changed selectors, invalidating its IDs.
    let config = f.temp.path().join("config.toml");
    let mut text = fs::read_to_string(&config).unwrap();
    text.push('\n');
    text = text.replace("docs = [", "docs = [\"missing/**\", ");
    fs::write(&config, text).unwrap();
    f.ok(&["update", "demo"]);
    let new = f.json(&["search", "beta", "--json"]);
    assert_ne!(new[0]["id"], beta[0]["id"]);
    assert!(!f.run(&["show", &beta_id]).status.success());
    assert_eq!(
        f.json(&["show", &id, "--json"])["revision"],
        old[0]["revision"]
    );
}
#[test]
fn bounded_unicode_code_excerpts_disclose_omissions_and_keep_coordinates() {
    let f = Fixture::new();
    let line = format!("{}needle{}", "🦀".repeat(6000), "🦀".repeat(6000));
    let source = format!("before\n{line}\nafter\n");
    fs::write(f.upstream.join("huge.rs"), &source).unwrap();
    fs::write(f.upstream.join("many.rs"), "needle\n".repeat(300)).unwrap();
    f.ok(&["update", "demo"]);
    let hits = f.json(&["search-code", "needle", "--path", "huge.rs", "--json"]);
    let hit = &hits[0];
    assert_eq!(hit["match_lines"], serde_json::json!([2]));
    assert_eq!(hit["start_line"], 1);
    assert_eq!(hit["end_line"], 3);
    assert_eq!(hit["text_truncated"], true);
    assert!(hit["text"].as_str().unwrap().len() <= 8192);
    assert!(hit["text"].as_str().unwrap().contains("needle"));
    let excerpt_lines: Vec<_> = hit["text"].as_str().unwrap().lines().collect();
    let source_lines: Vec<_> = source.lines().collect();
    for (excerpt, span) in excerpt_lines
        .iter()
        .zip(hit["excerpt_spans"].as_array().unwrap())
    {
        let line = source_lines[span["line"].as_u64().unwrap() as usize - 1];
        let from = span["start_byte"].as_u64().unwrap() as usize;
        let to = span["end_byte"].as_u64().unwrap() as usize;
        assert_eq!(*excerpt, &line[from..to]);
    }
    let cached = fs::read_to_string(hit["local_path"].as_str().unwrap()).unwrap();
    assert_eq!(cached, source);
    let many = f.json(&["search-code", "needle", "--path", "many.rs", "--json"]);
    for h in many.as_array().unwrap() {
        assert!(h["text"].as_str().unwrap().len() <= 8192);
        assert!(h["end_line"].as_u64().unwrap() - h["start_line"].as_u64().unwrap() < 120);
    }
    assert!(!f.run(&["search-code", "[", "--regex"]).status.success());
    assert!(!f
        .run(&["search-code", "needle", "--path", "["])
        .status
        .success());
    assert!(!f.run(&["search", "---"]).status.success());
    assert!(f
        .json(&["search-code", "absent", "--json"])
        .as_array()
        .unwrap()
        .is_empty());
}
#[test]
fn failed_local_copy_preserves_published_docs_and_original() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    fs::write(
        f.upstream.join("guide.md"),
        "# Stable\nstable original docs\n",
    )
    .unwrap();
    f.ok(&["update", "demo"]);
    let before = f.json(&["search", "stable", "--json"]);
    symlink("/unavailable/source", f.upstream.join("broken-link")).unwrap();
    assert!(!f.run(&["update", "demo"]).status.success());
    let after = f.json(&["search", "stable", "--json"]);
    assert_eq!(before, after);
    assert_eq!(
        fs::read_to_string(f.upstream.join("guide.md")).unwrap(),
        "# Stable\nstable original docs\n"
    );
    fs::remove_file(f.upstream.join("broken-link")).unwrap();
    f.ok(&["update", "demo"]);
}
#[test]
fn terminating_git_update_reaps_git_and_releases_cache_locks() {
    use std::os::unix::fs::PermissionsExt;
    for signal in [libc::SIGINT, libc::SIGTERM] {
        let f = Fixture::new();
        fs::write(
            f.upstream.join("guide.md"),
            "# Stable\nstable preserved evidence\n",
        )
        .unwrap();
        for args in [
            vec!["init", "--quiet"],
            vec!["add", "."],
            vec![
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.test",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--quiet",
                "-m",
                "fixture",
            ],
        ] {
            assert!(Command::new("git")
                .arg("-C")
                .arg(&f.upstream)
                .args(args)
                .status()
                .unwrap()
                .success());
        }
        f.ok(&["add", "remote", f.upstream.to_str().unwrap()]);
        f.ok(&["update", "remote"]);
        let before = f.json(&["search", "stable", "--library", "remote", "--json"]);
        let helpers = f.temp.path().join("helpers");
        fs::create_dir(&helpers).unwrap();
        let script = helpers.join("git");
        fs::write(&script, "#!/bin/sh\nprintf '%s' \"$$\" > \"$RTFM_TEST_GIT_PID\"\nsleep 9 &\nprintf '%s' \"$!\" > \"$RTFM_TEST_CHILD_PID\"\nwait\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let pid_path = f.temp.path().join("git.pid");
        let child_pid_path = f.temp.path().join("child.pid");
        let mut child = f
            .command()
            .args(["update", "remote"])
            .env(
                "PATH",
                format!("{}:{}", helpers.display(), std::env::var("PATH").unwrap()),
            )
            .env("RTFM_TEST_GIT_PID", &pid_path)
            .env("RTFM_TEST_CHILD_PID", &child_pid_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(8);
        while !child_pid_path.exists() || fs::read_to_string(&child_pid_path).unwrap().is_empty() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(20));
        }
        let git_pid: i32 = fs::read_to_string(&pid_path).unwrap().parse().unwrap();
        let helper_pid: i32 = fs::read_to_string(&child_pid_path)
            .unwrap()
            .parse()
            .unwrap();
        unsafe {
            assert_eq!(libc::kill(child.id() as i32, signal), 0);
        }
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            assert!(Instant::now() < deadline, "update ignored signal {signal}");
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(!status.success());
        unsafe {
            assert_eq!(
                libc::kill(git_pid, 0),
                -1,
                "Git process survived interrupted update"
            );
        }
        // A killed helper can briefly remain a zombie under the container init; it cannot do work.
        if let Ok(stat) = fs::read_to_string(format!("/proc/{helper_pid}/stat")) {
            assert_eq!(
                stat.rsplit_once(") ").unwrap().1.chars().next(),
                Some('Z'),
                "Git helper remained alive"
            );
        }
        assert_eq!(
            f.json(&["search", "stable", "--library", "remote", "--json"]),
            before
        );
        f.ok(&["update", "remote"]);
        assert_eq!(
            f.json(&["search", "stable", "--library", "remote", "--json"]),
            before
        );
    }
}
#[test]
fn explicit_versions_do_not_search_disabled_or_removed_sources() {
    let f = Fixture::new();
    fs::write(
        f.upstream.join("guide.md"),
        "# Shared\nshared query evidence\n",
    )
    .unwrap();
    f.ok(&[
        "add",
        "disabled",
        f.upstream.to_str().unwrap(),
        "--kind",
        "local",
    ]);
    f.ok(&[
        "add",
        "removed",
        f.upstream.to_str().unwrap(),
        "--kind",
        "local",
    ]);
    f.ok(&["update"]);
    let config = f.temp.path().join("config.toml");
    let mut data: toml::Value = toml::from_str(&fs::read_to_string(&config).unwrap()).unwrap();
    data["sources"]["disabled"]["enabled"] = toml::Value::Boolean(false);
    data["sources"].as_table_mut().unwrap().remove("removed");
    fs::write(&config, toml::to_string(&data).unwrap()).unwrap();
    let docs = f.json(&["search", "shared", "--version", "HEAD", "--json"]);
    assert_eq!(docs.as_array().unwrap().len(), 1);
    assert_eq!(docs[0]["library"], "demo");
    for name in ["disabled", "removed"] {
        assert!(!f
            .run(&["search", "shared", "--library", name, "--version", "HEAD"])
            .status
            .success());
    }
}
#[test]
fn controlled_registries_discover_add_update_and_reject_changed_release_metadata() {
    use sha2::{Digest, Sha256};
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
    };
    for (ecosystem, registry_env) in [
        ("pypi", "RTFM_PYPI_REGISTRY"),
        ("npm", "RTFM_NPM_REGISTRY"),
        ("crates.io", "RTFM_CRATES_REGISTRY"),
    ] {
        let f = Fixture::new();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let mut payload = tar::Builder::new(Vec::new());
        for path in [
            "package/guide.md",
            "package/api.py",
            "package/api.js",
            "package/api.rs",
        ] {
            let text = b"registryfixture needlefixture\n";
            let mut header = tar::Header::new_gnu();
            header.set_size(text.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            payload.append_data(&mut header, path, &text[..]).unwrap();
        }
        let archive = payload.into_inner().unwrap();
        let checksum = format!("{:x}", Sha256::digest(&archive));
        let artifact = format!("{base}/artifact.tar");
        let metadata = match ecosystem {
            "pypi" => {
                serde_json::json!({"info":{"name":"fixture","version":"1.0.0"},"urls":[{"packagetype":"sdist","url":artifact,"digests":{"sha256":checksum}}]})
            }
            "npm" => {
                serde_json::json!({"name":"fixture","version":"1.0.0","dist":{"tarball":artifact}})
            }
            _ => {
                serde_json::json!({"version":{"crate":"fixture","num":"1.0.0","dl_path":"/artifact.tar","checksum":checksum}})
            }
        };
        let server = std::thread::spawn(move || {
            for request_number in 0..4 {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(&mut stream);
                let mut request = String::new();
                reader.read_line(&mut request).unwrap();
                loop {
                    let mut header = String::new();
                    assert_ne!(reader.read_line(&mut header).unwrap(), 0);
                    if header == "\r\n" {
                        break;
                    }
                }
                let body = if request.starts_with("GET /artifact.tar ") {
                    archive.clone()
                } else if request_number == 3 {
                    b"{\"version\":\"9.9.9\"}".to_vec()
                } else {
                    serde_json::to_vec(&metadata).unwrap()
                };
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                stream.write_all(&body).unwrap();
            }
        });
        let spec = format!("{ecosystem}:fixture@1.0.0");
        let discover = f
            .command()
            .env(registry_env, &base)
            .args(["discover", &spec, "--json"])
            .output()
            .unwrap();
        assert!(
            discover.status.success(),
            "{}",
            String::from_utf8_lossy(&discover.stderr)
        );
        let discovery: Value = serde_json::from_slice(&discover.stdout).unwrap();
        let name = discovery["source_name"].as_str().unwrap();
        let added = f
            .command()
            .env(registry_env, &base)
            .args(["add", &spec])
            .output()
            .unwrap();
        assert!(
            added.status.success(),
            "{}",
            String::from_utf8_lossy(&added.stderr)
        );
        f.ok(&["update", name]);
        let docs = f.json(&["search", "registryfixture", "--library", name, "--json"]);
        assert_eq!(docs.as_array().unwrap().len(), 1);
        assert_eq!(
            f.json(&["search-code", "needlefixture", "--library", name, "--json"])
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let config = fs::read(f.temp.path().join("config.toml")).unwrap();
        let rejected = f
            .command()
            .env(registry_env, &base)
            .args(["add", &spec])
            .output()
            .unwrap();
        assert!(!rejected.status.success());
        assert_eq!(config, fs::read(f.temp.path().join("config.toml")).unwrap());
        assert_eq!(
            docs,
            f.json(&["search", "registryfixture", "--library", name, "--json"])
        );
        server.join().unwrap();
    }
}
#[test]
fn recursive_submodule_failure_keeps_previous_parent_searchable() {
    fn git(path: &std::path::Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(path)
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.test",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().into()
    }
    let f = Fixture::new();
    fs::write(
        f.upstream.join("guide.md"),
        "# Prior\npriorpublication evidence\n",
    )
    .unwrap();
    git(&f.upstream, &["init", "--quiet"]);
    git(&f.upstream, &["add", "."]);
    git(&f.upstream, &["commit", "--quiet", "-m", "published"]);
    let parent_revision = git(&f.upstream, &["rev-parse", "HEAD"]);
    f.ok(&["add", "parent", f.upstream.to_str().unwrap()]);
    f.ok(&["update", "parent"]);
    let before = f.json(&[
        "search",
        "priorpublication",
        "--library",
        "parent",
        "--json",
    ]);
    let nested = f.temp.path().join("nested");
    fs::create_dir(&nested).unwrap();
    git(&nested, &["init", "--quiet"]);
    fs::write(nested.join(".gitmodules"), "[submodule \"missing\"]\n path = missing\n url = /nonexistent/rtfm-fixture-grandchild.git\n").unwrap();
    git(&nested, &["add", ".gitmodules"]);
    git(
        &nested,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{parent_revision},missing"),
        ],
    );
    git(&nested, &["commit", "--quiet", "-m", "missing grandchild"]);
    let nested_revision = git(&nested, &["rev-parse", "HEAD"]);
    fs::write(
        f.upstream.join(".gitmodules"),
        format!(
            "[submodule \"nested\"]\n path = nested\n url = {}\n",
            nested.display()
        ),
    )
    .unwrap();
    git(&f.upstream, &["add", ".gitmodules"]);
    git(
        &f.upstream,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{nested_revision},nested"),
        ],
    );
    git(
        &f.upstream,
        &["commit", "--quiet", "-m", "broken recursive submodule"],
    );
    let failed = f
        .command()
        .env("GIT_ALLOW_PROTOCOL", "file")
        .args(["update", "parent"])
        .output()
        .unwrap();
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("grandchild"));
    assert_eq!(
        before,
        f.json(&[
            "search",
            "priorpublication",
            "--library",
            "parent",
            "--json"
        ])
    );
    let versions = f.json(&["versions", "parent", "--json"]);
    assert_eq!(versions.as_array().unwrap().len(), 1);
    assert_eq!(versions[0]["revision"], parent_revision);
}
#[test]
fn interrupted_http_body_never_publishes_partial_or_completed_replacement() {
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
        sync::mpsc,
    };
    let f = Fixture::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let location = format!("http://{}/guide.md", listener.local_addr().unwrap());
    let (ready_send, ready) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let server = std::thread::spawn(move || {
        for request in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(&mut stream);
            loop {
                let mut line = String::new();
                assert_ne!(reader.read_line(&mut line).unwrap(), 0);
                if line == "\r\n" {
                    break;
                }
            }
            let body = if request == 0 {
                "# Prior\nhttppreserved original evidence\n"
            } else {
                "# Replacement\nreplacement evidence\n"
            };
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            if request == 1 {
                stream.write_all(&body.as_bytes()[..1]).unwrap();
                ready_send.send(()).unwrap();
                released.recv_timeout(Duration::from_secs(5)).unwrap();
                let _ = stream.write_all(&body.as_bytes()[1..]);
            } else {
                stream.write_all(body.as_bytes()).unwrap();
            }
        }
    });
    f.ok(&["add", "web", &location, "--kind", "url"]);
    f.ok(&["update", "web"]);
    let before = f.json(&["search", "httppreserved", "--library", "web", "--json"]);
    let mut child = f
        .command()
        .args(["update", "web"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    unsafe {
        assert_eq!(libc::kill(child.id() as i32, libc::SIGTERM), 0);
    }
    // Complete the controlled request after signal delivery; the publication gate must still reject it.
    release.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "HTTP update did not exit after its request completed"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(!status.success());
    assert_eq!(
        before,
        f.json(&["search", "httppreserved", "--library", "web", "--json"])
    );
    assert!(f
        .json(&["search", "replacement", "--library", "web", "--json"])
        .as_array()
        .unwrap()
        .is_empty());
    server.join().unwrap();
}
#[test]
fn interrupted_http_body_error_reports_cancellation_and_prior_successes() {
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
        sync::mpsc,
    };
    for signal in [libc::SIGINT, libc::SIGTERM] {
        let f = Fixture::new();
        fs::write(
            f.upstream.join("guide.md"),
            "# Prior\nhttperrorprior evidence\n",
        )
        .unwrap();
        f.ok(&["update", "demo"]);
        let before = f.json(&["search", "httperrorprior", "--library", "demo", "--json"]);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let location = format!("http://{}/guide.md", listener.local_addr().unwrap());
        let (started_send, started) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(&mut stream);
            loop {
                let mut line = String::new();
                assert_ne!(reader.read_line(&mut line).unwrap(), 0);
                if line == "\r\n" {
                    break;
                }
            }
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\nConnection: close\r\n\r\nx")
                .unwrap();
            started_send.send(()).unwrap();
            released.recv_timeout(Duration::from_secs(5)).unwrap();
            // Close an incomplete body so the read fails before its cancellation check.
        });
        f.ok(&["add", "zweb", &location, "--kind", "url"]);
        let mut child = f
            .command()
            .args(["--output", "json", "update"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        started.recv_timeout(Duration::from_secs(5)).unwrap();
        unsafe {
            assert_eq!(libc::kill(child.id() as i32, signal), 0);
        }
        release.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let timed_out = child.try_wait().unwrap().is_none();
        if timed_out {
            child.kill().unwrap();
        }
        let output = child.wait_with_output().unwrap();
        server.join().unwrap();
        assert!(
            !timed_out,
            "interrupted update did not finish after body failure"
        );
        assert_eq!(output.status.code(), Some(128 + signal));
        let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(envelope["error"]["code"], "interrupted");
        assert_eq!(envelope["data"]["failures"][0]["code"], "interrupted");
        assert_eq!(envelope["data"]["failures"][0]["source"], "zweb");
        assert_eq!(envelope["data"]["updated"][0]["source"], "demo");
        assert_eq!(
            f.json(&["versions", "zweb", "--json"]),
            serde_json::json!([])
        );
        assert_eq!(
            before,
            f.json(&["search", "httperrorprior", "--library", "demo", "--json"])
        );
        assert!(f.temp.path().join("cache/update.lock").exists());
        f.ok(&["update", "demo"]);
    }
}

#[test]
fn signal_during_active_index_transaction_rolls_back_replacement() {
    use std::{
        io::{BufRead, BufReader},
        sync::mpsc,
    };
    let f = Fixture::new();
    let original = "# Prior\ntransactionprior searchable evidence\n";
    let replacement = "# Replacement\ntransactionreplacement searchable evidence\n";
    fs::write(f.upstream.join("guide.md"), original).unwrap();
    f.ok(&["update", "demo"]);
    let before = f.json(&["search", "transactionprior", "--library", "demo", "--json"]);
    let versions = f.json(&["versions", "demo", "--json"]);
    let database = rusqlite::Connection::open(f.temp.path().join("cache/index.sqlite3")).unwrap();
    database.busy_timeout(Duration::ZERO).unwrap();
    // This owned fixture holds the real indexing writer after chunk insertion but
    // before publication. The bounded CTE uses constant working storage; production
    // has no timing hook or altered code path.
    database
        .execute_batch(
            "CREATE TRIGGER fixture_hold_index BEFORE INSERT ON file_entries BEGIN
        SELECT sum(n) FROM (WITH RECURSIVE work(n) AS
            (VALUES(0) UNION ALL SELECT n+1 FROM work WHERE n<3000000) SELECT n FROM work);
        END;",
        )
        .unwrap();
    fs::write(f.upstream.join("guide.md"), replacement).unwrap();
    let mut child = f
        .command()
        .args(["--output", "json", "update", "demo"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let (send, receive) = mpsc::channel();
    let stderr = child.stderr.take().unwrap();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            if send.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut observed_index_start = false;
    while let Ok(line) = receive.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        if line.starts_with("Reused ") {
            observed_index_start = true;
            break;
        }
    }
    let mut active_writer = false;
    if observed_index_start {
        while Instant::now() < deadline {
            match database.execute_batch("BEGIN IMMEDIATE") {
                Ok(()) => database.execute_batch("ROLLBACK").unwrap(),
                Err(rusqlite::Error::SqliteFailure(error, _))
                    if error.code == rusqlite::ErrorCode::DatabaseBusy =>
                {
                    active_writer = true;
                    unsafe {
                        assert_eq!(libc::kill(child.id() as i32, libc::SIGTERM), 0);
                    }
                    break;
                }
                Err(error) => panic!("observe indexing writer: {error}"),
            }
            if child.try_wait().unwrap().is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let timed_out = child.try_wait().unwrap().is_none();
    if timed_out {
        child.kill().unwrap();
    }
    let output = child.wait_with_output().unwrap();
    reader.join().unwrap();
    let stderr: Vec<_> = receive.try_iter().collect();
    assert!(observed_index_start, "update never reached indexing");
    assert!(
        active_writer,
        "signal was not synchronized with an active index write transaction: status={:?}, stdout={}, stderr={stderr:?}",
        output.status,
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        !timed_out,
        "indexing ignored its signal beyond the fixture deadline"
    );
    assert_eq!(output.status.code(), Some(128 + libc::SIGTERM));
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope["error"]["code"], "interrupted");
    assert_eq!(
        before,
        f.json(&["search", "transactionprior", "--library", "demo", "--json"])
    );
    assert_eq!(versions, f.json(&["versions", "demo", "--json"]));
    assert!(f
        .json(&[
            "search",
            "transactionreplacement",
            "--library",
            "demo",
            "--json"
        ])
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(
        fs::read_to_string(f.upstream.join("guide.md")).unwrap(),
        replacement
    );
    let published = before[0]["local_path"].as_str().unwrap();
    assert_eq!(fs::read_to_string(published).unwrap(), original);
    assert_eq!(
        database
            .query_row("SELECT count(*) FROM chunks", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    database
        .execute_batch("DROP TRIGGER fixture_hold_index")
        .unwrap();
    f.ok(&["update", "demo"]);
    assert_eq!(
        f.json(&[
            "search",
            "transactionreplacement",
            "--library",
            "demo",
            "--json"
        ])[0]["library"],
        "demo"
    );
}
