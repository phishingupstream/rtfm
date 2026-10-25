//! #15/#18/#21, R-7/R-10/R-16: process I/O, permissions and lock outcomes.
#![cfg(unix)]
use fs2::FileExt;
use serde_json::Value;
use std::{
    fs,
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::Path,
    process::{Child, Command, Output, Stdio},
    time::{Duration, Instant},
};

fn command(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rtfm"));
    command
        .current_dir(root)
        .env_remove("RTFM_CONFIG")
        .env_remove("RTFM_CACHE_DIR")
        .arg("--config")
        .arg(root.join("config.toml"))
        .arg("--cache-dir")
        .arg(root.join("cache"));
    command
}
fn json(output: &Output, success: bool) -> Value {
    assert_eq!(
        output.status.success(),
        success,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["ok"], success);
    value
}
fn fixture(root: &Path) {
    fs::create_dir(root.join("source")).unwrap();
    fs::write(
        root.join("source/guide.md"),
        "# Guide\npermission preservation evidence\n",
    )
    .unwrap();
    assert!(command(root).arg("init").output().unwrap().status.success());
    assert!(command(root)
        .args(["add", "demo"])
        .arg(root.join("source"))
        .args(["--kind", "local"])
        .output()
        .unwrap()
        .status
        .success());
    assert!(command(root)
        .args(["update", "demo"])
        .output()
        .unwrap()
        .status
        .success());
}

fn finish_with_deadline(mut child: Child) -> Output {
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let blocked = child.try_wait().unwrap().is_none();
    if blocked {
        child.kill().unwrap();
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        !blocked,
        "CLI did not finish while the fixture held its lock"
    );
    output
}

#[test]
fn cache_reads_remain_available_during_an_uncommitted_index_write() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fixture(root);
    let mut db = rusqlite::Connection::open(root.join("cache/index.sqlite3")).unwrap();
    let id: i64 = db
        .query_row("SELECT rowid FROM chunks LIMIT 1", [], |r| r.get(0))
        .unwrap();
    let id = id.to_string();
    let commands = [
        vec!["search", "preservation"],
        vec!["search-code", "preservation"],
        vec!["show", &id],
        vec!["path", "demo"],
        vec!["versions", "demo"],
        vec!["sources"],
        vec!["status"],
        vec!["deps", "."],
    ];
    // Also exercise a cache predating the ID allocator, without migrating on reads.
    for legacy in [false, true] {
        if legacy {
            db.execute_batch("DROP TABLE chunk_ids").unwrap();
        }
        let expected: Vec<_> = commands
            .iter()
            .map(|args| {
                json(
                    &command(root)
                        .args(["--output", "json"])
                        .args(args)
                        .output()
                        .unwrap(),
                    true,
                )
            })
            .collect();
        let tx = db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        tx.execute("DELETE FROM chunks", []).unwrap();
        for (args, before) in commands.iter().zip(&expected) {
            let child = command(root)
                .args(["--output", "json"])
                .args(args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            assert_eq!(
                &json(&finish_with_deadline(child), true),
                before,
                "{args:?}"
            );
        }
        tx.rollback().unwrap();
    }
}

#[test]
fn signals_during_config_lock_wait_preserve_the_signal_exit_and_configuration() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fixture(root);
    let before = fs::read(root.join("config.toml")).unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.join("config.toml.lock"))
        .unwrap();
    lock.lock_exclusive().unwrap();
    for signal in [libc::SIGINT, libc::SIGTERM] {
        let mut child = command(root)
            .args(["--output", "json", "use", "demo", "--ref", "cancelled"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        // Linux exposes the blocked flock syscall, avoiding a race with handler setup.
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut waiting = false;
        while Instant::now() < deadline && child.try_wait().unwrap().is_none() {
            let wait =
                fs::read_to_string(format!("/proc/{}/wchan", child.id())).unwrap_or_default();
            if wait.contains("locks_lock_inode_wait") {
                waiting = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        unsafe {
            assert_eq!(libc::kill(child.id() as i32, signal), 0);
        }
        let output = finish_with_deadline(child);
        assert!(waiting, "did not observe the configuration lock wait");
        assert_eq!(output.status.code(), Some(128 + signal));
        assert_eq!(json(&output, false)["error"]["code"], "interrupted");
        assert_eq!(fs::read(root.join("config.toml")).unwrap(), before);
    }
}
#[test]
fn stdout_disk_error_fails_without_panic() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fixture(root);
    for args in [
        vec!["status"],
        vec!["sources", "--json"],
        vec!["--output", "json", "sources"],
    ] {
        let output = command(root)
            .args(&args)
            .stdout(Stdio::from(
                fs::OpenOptions::new()
                    .write(true)
                    .open("/dev/full")
                    .unwrap(),
            ))
            .output()
            .unwrap();
        assert!(
            !output.status.success(),
            "accepted failed stdout for {args:?}"
        );
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains("stdout"), "missing I/O context: {error}");
        assert!(
            !error.contains("panicked"),
            "panic on output failure: {error}"
        );
    }
}
#[test]
fn denied_configuration_write_preserves_config_and_searchable_cache() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fixture(root);
    let config = root.join("config.toml");
    let before = fs::read(&config).unwrap();
    let cached = command(root)
        .args(["search", "preservation", "--json"])
        .output()
        .unwrap();
    assert!(cached.status.success());
    // UID 0 ignores ordinary mode bits, so drop only the child to an unprivileged UID.
    let uid = String::from_utf8(Command::new("id").arg("-u").output().unwrap().stdout).unwrap();
    let original_mode = fs::metadata(root).unwrap().permissions().mode();
    fs::set_permissions(root, fs::Permissions::from_mode(0o555)).unwrap();
    fs::set_permissions(&config, fs::Permissions::from_mode(0o444)).unwrap();
    let mut denied = command(root);
    if uid.trim() == "0" {
        denied.uid(65534).gid(65534);
    }
    let output = denied
        .args(["--output", "json", "use", "demo", "--ref", "denied"])
        .output()
        .unwrap();
    fs::set_permissions(root, fs::Permissions::from_mode(original_mode)).unwrap();
    fs::set_permissions(&config, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(!output.status.success());
    assert_eq!(json(&output, false)["error"]["code"], "io_failed");
    assert_eq!(fs::read(&config).unwrap(), before);
    let after = command(root)
        .args(["search", "preservation", "--json"])
        .output()
        .unwrap();
    assert!(after.status.success());
    assert_eq!(after.stdout, cached.stdout);
}
#[test]
fn held_update_lock_distinguishes_explicit_work_from_due_noop() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fixture(root);
    let config_before = fs::read(root.join("config.toml")).unwrap();
    let versions_before = command(root)
        .args(["versions", "demo", "--json"])
        .output()
        .unwrap();
    let db = rusqlite::Connection::open(root.join("cache/index.sqlite3")).unwrap();
    let checked_before: i64 = db
        .query_row(
            "SELECT checked_at FROM update_checks WHERE library='demo'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join("cache/update.lock"))
        .unwrap();
    lock.lock_exclusive().unwrap();
    let explicit = command(root)
        .args(["--output", "json", "update", "demo"])
        .output()
        .unwrap();
    assert_eq!(json(&explicit, false)["error"]["code"], "update_locked");
    for args in [vec!["update", "--if-due"], vec!["update-if-due"]] {
        let output = command(root)
            .args(["--output", "json"])
            .args(args)
            .output()
            .unwrap();
        let value = json(&output, true);
        assert_eq!(value["data"]["changed"], false);
        assert_eq!(value["data"]["skipped"], "update_locked");
        assert!(value["data"]["updated"].as_array().unwrap().is_empty());
    }
    FileExt::unlock(&lock).unwrap();
    assert_eq!(fs::read(root.join("config.toml")).unwrap(), config_before);
    assert_eq!(
        command(root)
            .args(["versions", "demo", "--json"])
            .output()
            .unwrap()
            .stdout,
        versions_before.stdout
    );
    let checked_after: i64 = db
        .query_row(
            "SELECT checked_at FROM update_checks WHERE library='demo'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(checked_after, checked_before);
}
#[test]
fn narrow_redirected_help_has_readable_semantics_without_ansi() {
    let temp = tempfile::tempdir().unwrap();
    for (name, required) in [
        ("search", vec!["QUERY", "--library", "--top-k", "Literal"]),
        (
            "search-code",
            vec!["QUERY", "--regex", "--context", "--path"],
        ),
        ("add", vec!["NAME", "LOCATION", "--project", "--ref"]),
        ("use", vec!["NAME", "--ref", "--project"]),
    ] {
        let output = command(temp.path())
            .env("COLUMNS", "40")
            .env("NO_COLOR", "1")
            .args([name, "--help"])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        assert!(!output.stdout.contains(&0x1b), "ANSI in {name} help");
        let help = String::from_utf8(output.stdout).unwrap();
        assert!(help.contains("Usage:"));
        for expected in required {
            assert!(
                help.contains(expected),
                "{name} help omitted {expected}: {help}"
            );
        }
    }
}
