//! Registry discovery, explicit population, version coverage and failed-update preservation.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    process::{Command, Output},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::JoinHandle,
};
use tempfile::TempDir;

struct Registry {
    address: String,
    artifact: Arc<Mutex<Vec<u8>>>,
    paths: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}
impl Registry {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let mut archive = tar::Builder::new(Vec::new());
        for (path, contents) in [
            (
                "demo-1.2/README.md",
                "# Demo\nregistry_documentation_token explains widgets.\n",
            ),
            (
                "demo-1.2/demo.py",
                "def registry_source_token():\n    pass\n",
            ),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            archive
                .append_data(&mut header, path, contents.as_bytes())
                .unwrap();
        }
        let bytes = archive.into_inner().unwrap();
        let checksum = format!("{:x}", Sha256::digest(&bytes));
        let metadata = serde_json::to_vec(&json!({
            "info":{"name":"demo","version":"1.2","project_urls":{
                "Source":"https://example.invalid/demo", "Documentation":"https://example.invalid/docs"
            }},
            "urls":[{"packagetype":"sdist","url":format!("{address}/demo-1.2.tar"),"digests":{"sha256":checksum}}]
        })).unwrap();
        let artifact = Arc::new(Mutex::new(bytes));
        let paths = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (thread_artifact, thread_paths, thread_stop) =
            (artifact.clone(), paths.clone(), stop.clone());
        let thread = std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                if thread_stop.load(Ordering::SeqCst) {
                    break;
                }
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut request = [0; 8192];
                let count = stream.read(&mut request).unwrap();
                let text = String::from_utf8_lossy(&request[..count]);
                let path = text.split_whitespace().nth(1).unwrap_or("/").to_owned();
                thread_paths.lock().unwrap().push(path.clone());
                let (status, body) = match path.as_str() {
                    "/pypi/demo/1.2/json" => ("200 OK", metadata.clone()),
                    "/demo-1.2.tar" => ("200 OK", thread_artifact.lock().unwrap().clone()),
                    _ => ("404 Not Found", b"unknown fixture endpoint".to_vec()),
                };
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                stream.write_all(&body).unwrap();
            }
        });
        Self {
            address,
            artifact,
            paths,
            stop,
            thread: Some(thread),
        }
    }
    fn requests(&self) -> Vec<String> {
        self.paths.lock().unwrap().clone()
    }
}
impl Drop for Registry {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.address.trim_start_matches("http://"));
        self.thread.take().unwrap().join().unwrap();
    }
}
struct Project {
    dir: TempDir,
    registry: Registry,
}
impl Project {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("pyproject.toml"),
            "[project]\nname='consumer'\nversion='0.1.0'\ndependencies=['demo>=1', 'unknown>=2']\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("uv.lock"),
            "[[package]]\nname='demo'\nversion='1.2'\nsource={registry='https://pypi.org/simple'}\n",
        )
        .unwrap();
        let project = Self {
            dir,
            registry: Registry::new(),
        };
        project.ok(&["--config", ".rtfm.toml", "init"]);
        project
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_rtfm"))
            .current_dir(self.dir.path())
            .env_remove("RTFM_CONFIG")
            .env_remove("RTFM_CACHE_DIR")
            .env("RTFM_PYPI_REGISTRY", &self.registry.address)
            .env("XDG_CONFIG_HOME", self.dir.path().join("user-config"))
            .args([
                "--cache-dir",
                self.dir.path().join("cache").to_str().unwrap(),
            ])
            .args(args)
            .output()
            .unwrap()
    }
    fn ok(&self, args: &[&str]) -> String {
        let result = self.run(args);
        assert!(
            result.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        String::from_utf8(result.stdout).unwrap()
    }
    fn json(&self, args: &[&str]) -> Value {
        serde_json::from_str(&self.ok(args)).unwrap()
    }
    fn coverage(&self, name: &str) -> Value {
        self.json(&["deps", "--json"])
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["package"] == name)
            .unwrap()
            .clone()
    }
}

#[test]
fn discovery_and_add_do_not_fetch_artifacts_and_update_preserves_on_checksum_failure() {
    let project = Project::new();
    let discovered = project.json(&["discover", "pypi:demo@1.2", "--json"]);
    assert_eq!(discovered["package"]["version"], "1.2");
    assert!(
        discovered["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["kind"] == "artifact"
                && c["checksum"].as_str().unwrap().starts_with("sha256:"))
    );
    project.ok(&["add", "pypi:demo@1.2", "--project"]);
    assert!(project
        .registry
        .requests()
        .iter()
        .all(|p| p.ends_with("/json")));
    let requests = project.registry.requests().len();
    let before = project.coverage("pypi:demo");
    assert_eq!(before["version"], "1.2");
    assert_ne!(before["docs"]["status"], "cached");
    let source = before["docs"]["source"].as_str().unwrap();
    assert_eq!(
        project.coverage("pypi:unknown")["docs"]["status"],
        "unresolved"
    );
    assert_eq!(
        project.registry.requests().len(),
        requests,
        "deps must remain offline"
    );
    project.ok(&["update", source]);
    let coverage = project.coverage("pypi:demo");
    assert_eq!(coverage["docs"]["status"], "cached");
    assert_eq!(coverage["code"]["status"], "cached");
    let hits = project.json(&["search", "registry_documentation_token", "--json"]);
    assert!(!hits.as_array().unwrap().is_empty());
    assert!(!project
        .json(&["search-code", "registry_source_token", "--json"])
        .as_array()
        .unwrap()
        .is_empty());
    let revision = hits[0]["revision"].clone();
    *project.registry.artifact.lock().unwrap() = b"corrupt replacement artifact".to_vec();
    let failed = project.run(&["update", source]);
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("checksum"));
    assert_eq!(
        project.json(&["search", "registry_documentation_token", "--json"])[0]["revision"],
        revision
    );
    assert_eq!(project.coverage("pypi:demo")["docs"]["status"], "cached");
}

#[test]
fn manual_role_mappings_are_independent_and_do_not_infer_versions_from_source_names() {
    let project = Project::new();
    let docs = project.dir.path().join("manual-docs");
    let code = project.dir.path().join("manual-code");
    fs::create_dir_all(&docs).unwrap();
    fs::create_dir_all(&code).unwrap();
    fs::write(docs.join("guide.md"), "# Guide\nmanual_document_token\n").unwrap();
    fs::write(code.join("api.py"), "def manual_source_token(): pass\n").unwrap();
    project.ok(&[
        "add",
        "docs-demo",
        docs.to_str().unwrap(),
        "--kind",
        "local",
        "--ref",
        "1.2",
        "--package",
        "pypi:demo@1.2",
        "--project",
        "--docs",
        "**/*.md",
        "--role",
        "docs",
    ]);
    let initial = project.coverage("pypi:demo");
    assert_eq!(initial["docs"]["source"], "docs-demo");
    assert_eq!(initial["code"]["status"], "unmapped");
    project.ok(&[
        "add",
        "code-demo",
        code.to_str().unwrap(),
        "--kind",
        "local",
        "--ref",
        "1.2",
        "--package",
        "pypi:demo@1.2",
        "--project",
        "--code",
        "**/*.py",
        "--role",
        "code",
    ]);
    project.ok(&["update"]);
    let coverage = project.coverage("pypi:demo");
    assert_eq!(coverage["docs"]["source"], "docs-demo");
    assert_eq!(coverage["code"]["source"], "code-demo");
    assert_eq!(coverage["docs"]["status"], "cached");
    assert_eq!(coverage["code"]["status"], "cached");
    fs::write(
        project.dir.path().join("uv.lock"),
        "[[package]]\nname='demo'\nversion='1.3'\nsource={registry='https://pypi.org/simple'}\n",
    )
    .unwrap();
    let changed = project.coverage("pypi:demo");
    assert_ne!(changed["docs"]["status"], "cached");
    assert_ne!(changed["code"]["status"], "cached");
    assert!(
        project.registry.requests().is_empty(),
        "manual configuration and inventory must not query registries"
    );
}

#[test]
fn project_docs_overlay_preserves_inherited_user_code_mapping() {
    let project = Project::new();
    let upstream = project.dir.path().join("shared-upstream");
    let docs = project.dir.path().join("project-docs");
    fs::create_dir_all(&upstream).unwrap();
    fs::create_dir_all(&docs).unwrap();
    fs::write(
        upstream.join("api.py"),
        "def inherited_code_token(): pass\n",
    )
    .unwrap();
    fs::write(
        docs.join("guide.md"),
        "# Local guide\noverridden_docs_token\n",
    )
    .unwrap();
    let user_path = project.dir.path().join("user-config/rtfm/config.toml");
    fs::create_dir_all(user_path.parent().unwrap()).unwrap();
    let user_config = format!(
        "[sources.shared]\nkind='local'\nlocation={}\nrefs=['1.2']\ncode=['**/*.py']\n[packages.'pypi:demo@1.2'.docs]\nsource='shared'\nreference='1.2'\nevidence='user docs'\n[packages.'pypi:demo@1.2'.code]\nsource='shared'\nreference='1.2'\nevidence='user code'\n",
        serde_json::to_string(&upstream.to_str().unwrap()).unwrap()
    );
    fs::write(&user_path, &user_config).unwrap();
    project.ok(&[
        "add",
        "local-docs",
        docs.to_str().unwrap(),
        "--kind",
        "local",
        "--ref",
        "1.2",
        "--package",
        "pypi:demo@1.2",
        "--project",
        "--docs",
        "**/*.md",
        "--role",
        "docs",
    ]);
    let coverage = project.coverage("pypi:demo");
    assert_eq!(coverage["docs"]["source"], "local-docs");
    assert_eq!(coverage["code"]["source"], "shared");
    assert_eq!(coverage["code"]["evidence"], "user code");
    let local_config: toml::Value = fs::read_to_string(project.dir.path().join(".rtfm.toml"))
        .unwrap()
        .parse()
        .unwrap();
    assert!(
        local_config["packages"]["pypi:demo@1.2"]
            .get("code")
            .is_none(),
        "saving the overlay must not materialize inherited bindings"
    );
    assert!(local_config["sources"].get("shared").is_none());
    assert_eq!(fs::read_to_string(&user_path).unwrap(), user_config);
    project.ok(&["update"]);
    let cached = project.coverage("pypi:demo");
    assert_eq!(cached["docs"]["status"], "cached");
    assert_eq!(cached["code"]["status"], "cached");
    assert_eq!(
        project.json(&["search-code", "inherited_code_token", "--json"])[0]["library"],
        "shared"
    );
    assert_eq!(
        project.json(&["search", "overridden_docs_token", "--json"])[0]["library"],
        "local-docs"
    );
    assert!(project.registry.requests().is_empty());
}

#[test]
fn registry_add_rejects_source_kind_and_reference_overrides_without_changes() {
    let project = Project::new();
    let config_path = project.dir.path().join(".rtfm.toml");
    let initial_config = fs::read(&config_path).unwrap();
    for args in [
        vec!["add", "pypi:demo@1.2", "--project", "--kind", "local"],
        vec![
            "add",
            "pypi:demo@1.2",
            "--project",
            "--ref",
            "arbitrary-branch",
        ],
    ] {
        let output = project.run(&args);
        assert!(!output.status.success(), "{args:?} unexpectedly succeeded");
        assert!(String::from_utf8_lossy(&output.stderr).contains("published version"));
        assert_eq!(fs::read(&config_path).unwrap(), initial_config);
    }
    assert!(
        project.registry.requests().is_empty(),
        "invalid overrides must fail before registry access"
    );
}
