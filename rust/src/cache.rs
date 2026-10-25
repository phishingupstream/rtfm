//! Complete immutable source snapshots. Publication happens only after acquisition succeeds.
use crate::config::{Source, SourceKind};
use anyhow::{bail, ensure, Context, Result};
use base64::Engine;
use fs2::FileExt;
use sha2::{Digest, Sha256, Sha512};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, Write};
use std::os::unix::fs::{symlink, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

static INTERRUPTED: AtomicI32 = AtomicI32::new(0);
extern "C" fn interrupted(signal: libc::c_int) {
    INTERRUPTED.store(signal, Ordering::Relaxed);
}
/// Signal handlers only set an atomic flag; normal control flow cleans staging and children.
pub fn install_signal_handlers() -> Result<()> {
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = interrupted as *const () as usize;
        libc::sigemptyset(&mut action.sa_mask);
        for signal in [libc::SIGINT, libc::SIGTERM] {
            ensure!(
                libc::sigaction(signal, &action, std::ptr::null_mut()) == 0,
                "install signal handler: {}",
                std::io::Error::last_os_error()
            );
        }
    }
    Ok(())
}
#[derive(Debug)]
pub struct Interrupted(pub i32);
impl std::fmt::Display for Interrupted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "interrupted by signal {}; previous publication is preserved",
            self.0
        )
    }
}
impl std::error::Error for Interrupted {}
pub fn check_interrupted() -> Result<()> {
    let signal = INTERRUPTED.load(Ordering::Relaxed);
    if signal != 0 {
        return Err(Interrupted(signal).into());
    }
    Ok(())
}
#[derive(Debug)]
pub struct ProcessTimeout;
impl std::fmt::Display for ProcessTimeout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Git command exceeded its 120 second deadline; check remote access and retry the explicit update")
    }
}
impl std::error::Error for ProcessTimeout {}

#[derive(Debug)]
pub struct Snapshot {
    pub revision: String,
    pub root: PathBuf,
    pub provenance: String,
}

/// Fetching never modifies a previously published snapshot. The caller publishes its
/// active version only after indexing succeeds, preserving the last good version.
pub fn fetch(source: &Source, cache_root: &Path, reference: &str) -> Result<Snapshot> {
    fs::create_dir_all(cache_root)?;
    let cache_root = fs::canonicalize(cache_root)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(cache_root.join("fetch.lock"))?;
    let lock_deadline = Instant::now() + Duration::from_secs(120);
    while let Err(error) = lock.try_lock_exclusive() {
        ensure!(
            Instant::now() < lock_deadline,
            "source cache is busy; retry after the other update finishes"
        );
        if error.kind() != std::io::ErrorKind::WouldBlock {
            return Err(error).context("lock source cache");
        }
        check_interrupted()?;
        std::thread::sleep(Duration::from_millis(50));
    }
    check_interrupted()?;
    match source.kind {
        SourceKind::Git => fetch_git(source, &cache_root, reference),
        SourceKind::Local => fetch_local(source, &cache_root),
        SourceKind::Url => fetch_url(source, &cache_root),
        SourceKind::Archive => fetch_archive(source, &cache_root),
    }
}

/// Validate retained snapshots before a cached immutable selection is reused.
pub fn verify(snapshot: &Snapshot) -> Result<()> {
    ensure!(
        snapshot.root.is_dir(),
        "cached snapshot is missing: {}",
        snapshot.root.display()
    );
    if snapshot.revision.starts_with("sha256-") {
        ensure!(
            hash_tree(&snapshot.root)? == snapshot.revision,
            "cached snapshot was modified: {}",
            snapshot.root.display()
        );
        Ok(())
    } else {
        verify_git_snapshot(&snapshot.root, &snapshot.revision)
    }
}

fn git(repository: &Path, arguments: &[&str]) -> Result<Output> {
    check_interrupted()?;
    let mut command = Command::new("git");
    command.arg("-C").arg(repository).args(arguments);
    if std::env::var_os("GIT_SSH_COMMAND").is_none() && std::env::var_os("GIT_SSH").is_none() {
        command.env("GIT_SSH_COMMAND", "ssh -oBatchMode=yes -oConnectTimeout=15 -oServerAliveInterval=15 -oServerAliveCountMax=2");
    }
    let output = run_git(&mut command, Duration::from_secs(120), 16 * 1024 * 1024)?;
    ensure!(
        output.status.success(),
        "git {} failed: {}",
        arguments.join(" "),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(output)
}

fn run_git(command: &mut Command, timeout: Duration, output_limit: u64) -> Result<Output> {
    // Files avoid pipe-buffer deadlocks while we monitor the process deadline.
    let mut stdout = tempfile::tempfile()?;
    let mut stderr = tempfile::tempfile()?;
    let mut child = command
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("GIT_ASKPASS", "/bin/false")
        .env("SSH_ASKPASS", "/bin/false")
        .stdin(Stdio::null())
        .stdout(stdout.try_clone()?)
        .stderr(stderr.try_clone()?)
        .process_group(0)
        .spawn()
        .context("run installed git")?;
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        let interrupted = check_interrupted();
        let output_overflow =
            stdout.metadata()?.len() > output_limit || stderr.metadata()?.len() > 1024 * 1024;
        if interrupted.is_err() || Instant::now() >= deadline || output_overflow {
            // Each Git invocation owns its group, including SSH and recursive submodule helpers.
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.wait();
            interrupted?;
            ensure!(
                !output_overflow,
                "Git output exceeded its capture bound; refine the source or inspect it directly"
            );
            return Err(ProcessTimeout.into());
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    stdout.rewind()?;
    stderr.rewind()?;
    let mut output = Output {
        status,
        stdout: Vec::new(),
        stderr: Vec::new(),
    };
    stdout
        .take(output_limit + 1)
        .read_to_end(&mut output.stdout)?;
    ensure!(
        output.stdout.len() as u64 <= output_limit,
        "Git stdout exceeded its capture bound; source verification was not accepted"
    );
    stderr.take(64 * 1024 + 1).read_to_end(&mut output.stderr)?;
    if output.stderr.len() > 64 * 1024 {
        output.stderr.truncate(64 * 1024);
        output.stderr.extend_from_slice(b"\n[stderr truncated]");
    }
    Ok(output)
}

fn git_text(repository: &Path, arguments: &[&str]) -> Result<String> {
    Ok(String::from_utf8(git(repository, arguments)?.stdout)?
        .trim()
        .to_owned())
}

fn fetch_git(source: &Source, cache: &Path, reference: &str) -> Result<Snapshot> {
    ensure!(
        !source.location.starts_with('-'),
        "Git location cannot begin with '-'"
    );
    let repository = cache.join("objects.git");
    if !repository.exists() {
        let stage = tempfile::tempdir_in(cache)?;
        let staged_repository = stage.path().join("objects.git");
        fs::create_dir(&staged_repository)?;
        git(&staged_repository, &["init", "--bare", "--quiet"])?;
        git(
            &staged_repository,
            &["remote", "add", "origin", &source.location],
        )?;
        check_interrupted()?;
        fs::rename(staged_repository, &repository)?;
    }
    ensure!(
        git_text(&repository, &["remote", "get-url", "origin"])? == source.location,
        "source location changed; use a new source name/cache to preserve existing Git provenance"
    );
    ensure!(
        !reference.is_empty() && !reference.starts_with('-') && !reference.contains(['\n', '\r']),
        "invalid Git reference"
    );
    let (requested, pinned) = if let Some(tag) = reference.strip_prefix("tag:") {
        (format!("refs/tags/{tag}"), true)
    } else if reference.starts_with("refs/tags/") {
        (reference.to_owned(), true)
    } else if let Some(commit) = reference.strip_prefix("commit:") {
        ensure!(
            (7..=64).contains(&commit.len()) && commit.chars().all(|c| c.is_ascii_hexdigit()),
            "commit: requires a hexadecimal Git revision"
        );
        (commit.to_owned(), true)
    } else if reference.chars().all(|c| c.is_ascii_hexdigit())
        && (40..=64).contains(&reference.len())
    {
        (reference.to_owned(), true)
    } else if reference == "HEAD" {
        ("HEAD".to_owned(), false)
    } else {
        let branch = reference.strip_prefix("branch:").unwrap_or(reference);
        let branch = branch.strip_prefix("refs/heads/").unwrap_or(branch);
        git(
            &repository,
            &["check-ref-format", &format!("refs/heads/{branch}")],
        )?;
        (format!("refs/heads/{branch}"), false)
    };
    // A pin is local once acquired: a deleted/moved remote tag cannot silently
    // replace it. Branch/default refs are always refreshed and resolved via FETCH_HEAD.
    let pin_ref = format!("refs/rtfm/pins/{:x}", Sha256::digest(requested.as_bytes()));
    let existing_pin = pinned
        .then(|| {
            git_text(
                &repository,
                &["rev-parse", "--verify", &format!("{pin_ref}^{{commit}}")],
            )
        })
        .transpose();
    let revision = match existing_pin {
        Ok(Some(revision)) => revision,
        _ => {
            git(
                &repository,
                &[
                    "fetch",
                    "--quiet",
                    "--depth=1",
                    "--no-tags",
                    "origin",
                    &requested,
                ],
            )?;
            let revision = git_text(
                &repository,
                &["rev-parse", "--verify", "FETCH_HEAD^{commit}"],
            )?;
            if pinned {
                git(&repository, &["update-ref", &pin_ref, &revision])?;
            }
            revision
        }
    };
    // Canonical object reachability must not depend on FETCH_HEAD or a mutable
    // branch. Shared snapshot clones borrow these retained objects indefinitely.
    git(
        &repository,
        &[
            "update-ref",
            &format!("refs/rtfm/revisions/{revision}"),
            &revision,
        ],
    )?;
    let snapshots = cache.join("revisions");
    fs::create_dir_all(&snapshots)?;
    check_interrupted()?;
    let root = snapshots.join(&revision);
    if !root.exists() {
        let stage = tempfile::tempdir_in(cache)?;
        let checkout = stage.path().join("checkout");
        fs::create_dir(&checkout)?;
        git(&checkout, &["init", "--quiet"])?;
        git(&checkout, &["remote", "add", "origin", &source.location])?;
        // Native shared-clone optimization is disabled by Git for shallow
        // sources. Use Git's standard alternates configuration explicitly.
        // Both staging and final checkouts sit two levels below this cache,
        // so the relative object-store path survives atomic publication.
        fs::write(
            checkout.join(".git/objects/info/alternates"),
            "../../../../objects.git/objects\n",
        )?;
        if repository.join("shallow").is_file() {
            fs::copy(repository.join("shallow"), checkout.join(".git/shallow"))?;
        }
        git(&checkout, &["checkout", "--quiet", "--detach", &revision])?;
        // Git resolves relative submodule URLs against the original origin and
        // checks out each recorded gitlink, including recursive dependencies.
        // Transport policy remains the installed Git configuration's decision.
        git(
            &checkout,
            &[
                "submodule",
                "update",
                "--init",
                "--recursive",
                "--depth=1",
                "--checkout",
            ],
        )?;
        verify_git_snapshot(&checkout, &revision)?;
        fs::rename(&checkout, &root).context("publish complete Git snapshot")?;
    }
    verify_git_snapshot(&root, &revision)?;
    Ok(Snapshot {
        revision,
        root,
        provenance: format!("git {} {reference}", source.location),
    })
}

fn verify_git_snapshot(root: &Path, revision: &str) -> Result<()> {
    ensure!(
        git_text(root, &["rev-parse", "HEAD"])? == revision,
        "cached Git snapshot revision mismatch at {}",
        root.display()
    );
    ensure!(
        git_text(
            root,
            &[
                "status",
                "--porcelain",
                "--untracked-files=all",
                "--ignore-submodules=none"
            ]
        )?
        .is_empty(),
        "cached snapshot was modified: {}; preserve or repair it before updating",
        root.display()
    );
    let submodules = git(root, &["submodule", "status", "--recursive"])?;
    ensure!(
        submodules
            .stdout
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .all(|line| line.starts_with(b" ")),
        "cached Git snapshot has an uninitialized or modified submodule at {}",
        root.display()
    );
    Ok(())
}

fn fetch_local(source: &Source, cache: &Path) -> Result<Snapshot> {
    let original = fs::canonicalize(&source.location)
        .with_context(|| format!("open local source {}", source.location))?;
    ensure!(
        !cache.starts_with(&original),
        "cache cannot be inside the local source directory"
    );
    let stage = tempfile::tempdir_in(cache)?;
    let payload = stage.path().join("payload");
    let revision = if original.is_dir() {
        copy_tree(&original, &original, &payload)?;
        let original_revision = hash_tree(&original)?;
        let revision = hash_tree(&payload)?;
        ensure!(
            original_revision == revision,
            "local source changed while copying; retry the update"
        );
        revision
    } else if original.is_file() {
        fs::create_dir(&payload)?;
        let name = original
            .file_name()
            .context("local source file has no name")?;
        ensure!(
            name != ".git",
            "local source filename .git is reserved for Git metadata"
        );
        let copied = payload.join(name);
        fs::copy(&original, &copied)?;
        ensure!(
            hash_file(&original)? == hash_file(&copied)?
                && fs::metadata(&original)?.permissions().mode() & 0o111
                    == fs::metadata(&copied)?.permissions().mode() & 0o111,
            "local source changed while copying; retry the update"
        );
        hash_tree(&payload)?
    } else {
        bail!("local source must be a regular file or directory");
    };
    publish(
        cache,
        &payload,
        revision,
        format!("local {}", original.display()),
    )
}

fn hash_file(path: &Path) -> Result<String> {
    let mut input = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        check_interrupted()?;
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Check registry-provided integrity against the exact bytes before extraction.
fn verify_archive_checksum(path: &Path, checksum: &str, sha256: &str) -> Result<()> {
    let matches = if let Some(expected) = checksum.strip_prefix("sha256:") {
        ensure!(
            expected.len() == 64 && expected.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "invalid archive SHA-256 checksum"
        );
        expected.eq_ignore_ascii_case(sha256)
    } else if let Some(expected) = checksum
        .strip_prefix("sha512-base64:")
        .or_else(|| checksum.strip_prefix("sha512-"))
    {
        let expected = base64::engine::general_purpose::STANDARD
            .decode(expected)
            .context("invalid archive SHA-512 base64 checksum")?;
        ensure!(
            expected.len() == 64,
            "invalid archive SHA-512 checksum length"
        );
        let mut input = File::open(path)?;
        let mut hasher = Sha512::new();
        let mut buffer = [0u8; 65536];
        loop {
            check_interrupted()?;
            let count = input.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            hasher.update(&buffer[..count]);
        }
        hasher.finalize().as_slice() == expected.as_slice()
    } else {
        bail!("unsupported archive checksum; use sha256:<hex> or sha512-base64:<base64>");
    };
    ensure!(
        matches,
        "archive checksum mismatch: downloaded bytes differ from registry integrity"
    );
    Ok(())
}

fn entries(directory: &Path) -> Result<Vec<fs::DirEntry>> {
    let mut entries = fs::read_dir(directory)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.retain(|entry| entry.file_name() != ".git");
    entries.sort_by_key(|entry| entry.file_name());
    Ok(entries)
}

fn copy_tree(original: &Path, from: &Path, to: &Path) -> Result<()> {
    fs::create_dir(to)?;
    for entry in entries(from)? {
        check_interrupted()?;
        let path = entry.path();
        let destination = to.join(entry.file_name());
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.is_dir() {
            copy_tree(original, &path, &destination)?;
        } else if metadata.is_file() {
            fs::copy(&path, &destination)?;
        } else if metadata.file_type().is_symlink() {
            let resolved = fs::canonicalize(&path)
                .with_context(|| format!("resolve source symlink {}", path.display()))?;
            ensure!(
                resolved.starts_with(original),
                "source symlink escapes snapshot: {}",
                path.display()
            );
            ensure!(
                !resolved
                    .strip_prefix(original)?
                    .components()
                    .any(|c| c.as_os_str() == ".git"),
                "source symlink targets excluded Git metadata: {}",
                path.display()
            );
            let target = fs::read_link(&path)?;
            ensure!(
                !target.is_absolute(),
                "absolute source symlink cannot be relocated: {}",
                path.display()
            );
            symlink(target, destination)?;
        } else {
            bail!("unsupported source file type: {}", path.display());
        }
    }
    fs::set_permissions(to, fs::metadata(from)?.permissions())?;
    Ok(())
}

fn hash_tree(root: &Path) -> Result<String> {
    fn visit(root: &Path, path: &Path, hasher: &mut Sha256) -> Result<()> {
        for entry in entries(path)? {
            check_interrupted()?;
            let path = entry.path();
            let relative = path.strip_prefix(root)?;
            use std::os::unix::ffi::OsStrExt;
            let bytes = relative.as_os_str().as_bytes();
            hasher.update((bytes.len() as u64).to_le_bytes());
            hasher.update(bytes);
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.is_dir() {
                hasher.update(b"d");
                visit(root, &path, hasher)?;
            } else if metadata.is_file() {
                hasher.update(b"f");
                hasher.update((metadata.permissions().mode() & 0o111).to_le_bytes());
                hasher.update(metadata.len().to_le_bytes());
                let mut file = File::open(&path)?;
                let mut buffer = [0u8; 65536];
                loop {
                    check_interrupted()?;
                    let count = file.read(&mut buffer)?;
                    if count == 0 {
                        break;
                    }
                    hasher.update(&buffer[..count]);
                }
            } else if metadata.file_type().is_symlink() {
                hasher.update(b"l");
                let target = fs::read_link(path)?;
                let bytes = target.as_os_str().as_bytes();
                hasher.update((bytes.len() as u64).to_le_bytes());
                hasher.update(bytes);
            } else {
                bail!("unsupported source file type: {}", path.display());
            }
        }
        Ok(())
    }
    let mut hasher = Sha256::new();
    visit(root, root, &mut hasher)?;
    Ok(format!("sha256-{:x}", hasher.finalize()))
}

fn publish(cache: &Path, payload: &Path, revision: String, provenance: String) -> Result<Snapshot> {
    let snapshots = cache.join("revisions");
    fs::create_dir_all(&snapshots)?;
    check_interrupted()?;
    let root = snapshots.join(&revision);
    if root.exists() {
        ensure!(
            hash_tree(&root)? == revision,
            "cached snapshot was modified: {}",
            root.display()
        );
    } else {
        fs::rename(payload, &root).context("publish complete snapshot")?;
    }
    Ok(Snapshot {
        revision,
        root,
        provenance,
    })
}

fn client() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(30))
        .user_agent(concat!("rtfm/", env!("CARGO_PKG_VERSION")))
        .build()?)
}

fn document_bytes(response: reqwest::blocking::Response) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    response
        .take(16 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= 16 * 1024 * 1024,
        "web document exceeds 16 MiB; configure a focused documentation source"
    );
    check_interrupted()?;
    Ok(bytes)
}

fn markdown_links(index: &str, base: &url::Url) -> Result<Vec<url::Url>> {
    let mut candidates = Vec::new();
    // llms.txt uses ordinary inline Markdown links. Also accept bare absolute
    // URLs, as used by several documentation manifests.
    for fragment in index.split("](").skip(1) {
        if let Some(end) = fragment.find(')') {
            candidates.push(
                fragment[..end]
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .trim_matches(['<', '>']),
            );
        }
    }
    candidates.extend(
        index
            .split(|c: char| c.is_whitespace() || "[]()<>\"'`".contains(c))
            .filter(|token| token.starts_with("http://") || token.starts_with("https://")),
    );
    let mut links = std::collections::BTreeMap::new();
    for candidate in candidates {
        let mut url = base
            .join(candidate)
            .with_context(|| format!("invalid llms.txt link {candidate}"))?;
        if matches!(url.scheme(), "http" | "https")
            && (url.path().ends_with(".md") || url.path().ends_with(".mdx"))
        {
            url.set_fragment(None);
            links.insert(url.to_string(), url);
        }
    }
    Ok(links.into_values().collect())
}

fn fetch_url(source: &Source, cache: &Path) -> Result<Snapshot> {
    let url = url::Url::parse(&source.location)?;
    ensure!(
        matches!(url.scheme(), "http" | "https"),
        "web source requires http:// or https://"
    );
    let client = client()?;
    let deadline = Instant::now() + Duration::from_secs(120);
    let response = client.get(url.clone()).send()?.error_for_status()?;
    let final_url = response.url().clone();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let bytes = document_bytes(response)?;
    ensure!(!bytes.is_empty(), "web source returned an empty document");
    let stage = tempfile::tempdir_in(cache)?;
    let payload = stage.path().join("payload");
    fs::create_dir(&payload)?;
    let extension = if content_type.contains("html") {
        "html"
    } else {
        "md"
    };
    fs::write(payload.join(format!("document.{extension}")), &bytes)?;
    let mut documents = std::collections::BTreeMap::new();
    documents.insert(format!("document.{extension}"), final_url.to_string());
    if url.path().ends_with("/llms.txt") || final_url.path().ends_with("/llms.txt") {
        let links = markdown_links(
            std::str::from_utf8(&bytes).context("llms.txt is not UTF-8")?,
            &final_url,
        )?;
        ensure!(
            !links.is_empty(),
            "llms.txt contains no linked markdown documents"
        );
        let pages = payload.join("pages");
        fs::create_dir(&pages)?;
        ensure!(
            links.len() <= 128,
            "llms.txt exceeds 128 linked documents; use a focused source"
        );
        for linked in links {
            check_interrupted()?;
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .context("web snapshot exceeded its 120 second deadline")?;
            let response = client
                .get(linked.clone())
                .timeout(remaining.min(Duration::from_secs(30)))
                .send()
                .with_context(|| format!("fetch linked document {linked}"))?
                .error_for_status()
                .with_context(|| format!("fetch linked document {linked}"))?;
            let resolved = response.url().to_string();
            let bytes = document_bytes(response)?;
            ensure!(!bytes.is_empty(), "linked document is empty: {linked}");
            // Hash the URL as well as retaining its basename: two sections with
            // index.md or URLs differing only in their query never collide.
            let suffix = linked.path().rsplit('/').next().unwrap_or("document.md");
            let filename = format!("{:x}-{suffix}", Sha256::digest(linked.as_str().as_bytes()));
            fs::write(pages.join(&filename), bytes)?;
            documents.insert(format!("pages/{filename}"), resolved);
        }
    }
    fs::write(
        payload.join(".rtfm-urls.json"),
        serde_json::to_vec_pretty(&documents)?,
    )?;
    publish(
        cache,
        &payload,
        hash_tree(&payload)?,
        format!("url {} -> {final_url}", source.location),
    )
}

fn safe_archive_path(path: &Path) -> Result<()> {
    ensure!(
        !path.as_os_str().is_empty()
            && path
                .components()
                .all(|c| matches!(c, Component::Normal(_) | Component::CurDir)),
        "archive contains unsafe path: {}",
        path.display()
    );
    ensure!(
        !path.components().any(|c| c.as_os_str() == ".git"),
        "archive contains reserved .git metadata"
    );
    Ok(())
}

fn fetch_archive(source: &Source, cache: &Path) -> Result<Snapshot> {
    let stage = tempfile::tempdir_in(cache)?;
    let archive_path = stage.path().join("download");
    let mut archive = File::create(&archive_path)?;
    let provenance;
    if source.location.starts_with("http://") || source.location.starts_with("https://") {
        let mut response = client()?.get(&source.location).send()?.error_for_status()?;
        provenance = format!("archive {} -> {}", source.location, response.url());
        std::io::copy(&mut response, &mut archive)?;
    } else {
        let original = fs::canonicalize(&source.location)?;
        provenance = format!("archive {}", original.display());
        std::io::copy(&mut File::open(original)?, &mut archive)?;
    }
    archive.flush()?;
    drop(archive);
    let digest = hash_file(&archive_path)?;
    if let Some(checksum) = &source.checksum {
        verify_archive_checksum(&archive_path, checksum, &digest)?;
    }
    let mut signature = [0u8; 4];
    let signature_length = File::open(&archive_path)?.read(&mut signature)?;
    ensure!(signature_length > 0, "archive is empty");
    let payload = stage.path().join("payload");
    fs::create_dir(&payload)?;
    if signature.starts_with(b"PK\x03\x04") {
        let mut zip = zip::ZipArchive::new(File::open(&archive_path)?)?;
        for index in 0..zip.len() {
            check_interrupted()?;
            let mut entry = zip.by_index(index)?;
            let path = entry
                .enclosed_name()
                .context("ZIP contains an unsafe path")?;
            safe_archive_path(&path)?;
            ensure!(
                entry
                    .unix_mode()
                    .is_none_or(|mode| matches!(mode & 0o170000, 0 | 0o100000 | 0o040000)),
                "archive links/special files require a Git or local source"
            );
            let destination = payload.join(&path);
            if entry.is_dir() {
                fs::create_dir_all(destination)?;
            } else {
                fs::create_dir_all(destination.parent().context("ZIP file has no parent")?)?;
                let mut output = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&destination)?;
                std::io::copy(&mut entry, &mut output)?;
                if let Some(mode) = entry.unix_mode() {
                    fs::set_permissions(destination, fs::Permissions::from_mode(mode & 0o777))?;
                }
            }
        }
    } else {
        let input: Box<dyn Read> = if signature.starts_with(&[0x1f, 0x8b]) {
            Box::new(flate2::read::GzDecoder::new(File::open(&archive_path)?))
        } else {
            Box::new(File::open(&archive_path)?)
        };
        let mut archive = tar::Archive::new(input);
        for entry in archive.entries()? {
            check_interrupted()?;
            let mut entry = entry?;
            let path = entry.path()?.into_owned();
            safe_archive_path(&path)?;
            let kind = entry.header().entry_type();
            ensure!(
                kind.is_file() || kind.is_dir(),
                "archive links/special files require a Git or local source: {}",
                path.display()
            );
            let destination = payload.join(path);
            if kind.is_dir() {
                fs::create_dir_all(destination)?;
            } else {
                fs::create_dir_all(destination.parent().context("tar file has no parent")?)?;
                let mut output = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&destination)?;
                std::io::copy(&mut entry, &mut output)?;
                fs::set_permissions(
                    destination,
                    fs::Permissions::from_mode(entry.header().mode()? & 0o777),
                )?;
            }
        }
    }
    ensure!(!entries(&payload)?.is_empty(), "archive contains no files");
    // Exact downloaded bytes are retained separately from the expanded tree.
    let downloads = cache.join("archives");
    fs::create_dir_all(&downloads)?;
    let retained = downloads.join(&digest);
    if !retained.exists() {
        fs::rename(&archive_path, &retained)?;
    }
    publish(
        cache,
        &payload,
        hash_tree(&payload)?,
        format!("{provenance} sha256:{digest} bytes:{}", retained.display()),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn git_stdout_overflow_is_an_error_not_partial_evidence() {
        let mut command = std::process::Command::new("/bin/sh");
        command.args(["-c", "printf '%0100d' 0"]);
        let error =
            super::run_git(&mut command, std::time::Duration::from_secs(2), 64).unwrap_err();
        assert!(error.to_string().contains("capture bound"));
    }
    #[test]
    fn git_deadline_terminates_and_reaps_the_child() {
        let mut command = std::process::Command::new("/bin/sh");
        command.args(["-c", "exec sleep 9"]);
        let started = std::time::Instant::now();
        let error =
            super::run_git(&mut command, std::time::Duration::from_millis(40), 1024).unwrap_err();
        assert!(error.downcast_ref::<super::ProcessTimeout>().is_some());
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }

    use super::*;
    fn source(kind: SourceKind, location: &Path) -> Source {
        Source {
            kind,
            location: location.to_str().unwrap().into(),
            refs: vec!["HEAD".into()],
            docs: vec!["**/*.md".into()],
            code: vec![],
            exclude: vec![],
            enabled: true,
            checksum: None,
        }
    }
    #[test]
    fn local_update_preserves_old_docs_code_and_deleted_files() -> Result<()> {
        let fixture = tempfile::tempdir()?;
        let original = fixture.path().join("original");
        fs::create_dir(&original)?;
        fs::write(original.join("guide.md"), "old documentation")?;
        fs::write(original.join("lib.rs"), "pub fn old() {}")?;
        fs::write(original.join("removed.md"), "retained in old version")?;
        let source = source(SourceKind::Local, &original);
        let cache = fixture.path().join("cache");
        let old = fetch(&source, &cache, "HEAD")?;
        fs::write(original.join("guide.md"), "changed documentation")?;
        fs::write(original.join("lib.rs"), "pub fn new() {}")?;
        fs::remove_file(original.join("removed.md"))?;
        let new = fetch(&source, &cache, "HEAD")?;
        assert_ne!(old.revision, new.revision);
        assert_eq!(
            fs::read_to_string(old.root.join("guide.md"))?,
            "old documentation"
        );
        assert_eq!(
            fs::read_to_string(old.root.join("lib.rs"))?,
            "pub fn old() {}"
        );
        assert!(old.root.join("removed.md").is_file());
        assert!(!new.root.join("removed.md").exists());
        assert_eq!(fetch(&source, &cache, "HEAD")?.root, new.root);
        Ok(())
    }
    #[test]
    fn failed_local_update_preserves_last_good_snapshot() -> Result<()> {
        let fixture = tempfile::tempdir()?;
        let original = fixture.path().join("original");
        fs::create_dir(&original)?;
        fs::write(original.join("guide.md"), "good")?;
        let source = source(SourceKind::Local, &original);
        let cache = fixture.path().join("cache");
        let good = fetch(&source, &cache, "HEAD")?;
        symlink("/etc/passwd", original.join("outside"))?;
        assert!(fetch(&source, &cache, "HEAD").is_err());
        assert_eq!(fs::read_to_string(good.root.join("guide.md"))?, "good");
        assert_eq!(fs::read_dir(cache.join("revisions"))?.count(), 1);
        Ok(())
    }
    #[test]
    fn git_branch_tracks_and_tag_remains_pinned_even_offline() -> Result<()> {
        let fixture = tempfile::tempdir()?;
        let original = fixture.path().join("original");
        fs::create_dir(&original)?;
        git(&original, &["init", "--quiet", "-b", "main"])?;
        git(&original, &["config", "user.name", "RTFM test"])?;
        git(&original, &["config", "user.email", "rtfm@example.invalid"])?;
        fs::write(original.join("guide.md"), "old")?;
        fs::write(original.join("lib.rs"), "source")?;
        git(&original, &["add", "."])?;
        git(&original, &["commit", "--quiet", "-m", "old"])?;
        git(&original, &["tag", "v1"])?;
        let source = source(SourceKind::Git, &original);
        let cache = fixture.path().join("cache");
        let old = fetch(&source, &cache, "branch:main")?;
        let tag = fetch(&source, &cache, "tag:v1")?;
        assert_eq!(old.revision, tag.revision);
        fs::write(original.join("guide.md"), "new")?;
        git(&original, &["add", "."])?;
        git(&original, &["commit", "--quiet", "-m", "new"])?;
        git(&original, &["tag", "--force", "v1"])?;
        let new = fetch(&source, &cache, "branch:main")?;
        assert_ne!(old.revision, new.revision);
        assert_eq!(fs::read_to_string(old.root.join("guide.md"))?, "old");
        assert_eq!(fs::read_to_string(new.root.join("guide.md"))?, "new");
        assert_eq!(fs::read_to_string(new.root.join("lib.rs"))?, "source");
        fs::rename(&original, fixture.path().join("offline"))?;
        assert_eq!(fetch(&source, &cache, "tag:v1")?.revision, old.revision);
        assert!(fetch(&source, &cache, "branch:main").is_err());
        assert!(old.root.is_dir() && new.root.is_dir());
        Ok(())
    }
    #[test]
    fn archive_retains_exact_bytes_and_rejects_links() -> Result<()> {
        let fixture = tempfile::tempdir()?;
        let original = fixture.path().join("source.tar");
        {
            let mut archive = tar::Builder::new(File::create(&original)?);
            let mut header = tar::Header::new_gnu();
            header.set_size(5);
            header.set_mode(0o644);
            header.set_cksum();
            archive.append_data(&mut header, "project/guide.md", &b"hello"[..])?;
            archive.finish()?;
        }
        let source = source(SourceKind::Archive, &original);
        let cache = fixture.path().join("cache");
        let good = fetch(&source, &cache, "HEAD")?;
        assert_eq!(
            fs::read_to_string(good.root.join("project/guide.md"))?,
            "hello"
        );
        let retained = fs::read_dir(cache.join("archives"))?
            .next()
            .unwrap()?
            .path();
        assert_eq!(fs::read(&original)?, fs::read(retained)?);
        // Different archive metadata keeps its exact bytes, while the identical
        // expanded source tree retains the same immutable revision.
        {
            let mut archive = tar::Builder::new(File::create(&original)?);
            let mut header = tar::Header::new_gnu();
            header.set_size(5);
            header.set_mode(0o644);
            header.set_mtime(123);
            header.set_cksum();
            archive.append_data(&mut header, "project/guide.md", &b"hello"[..])?;
            archive.finish()?;
        }
        assert_eq!(fetch(&source, &cache, "HEAD")?.revision, good.revision);
        assert_eq!(fs::read_dir(cache.join("archives"))?.count(), 2);
        assert_eq!(fs::read_dir(cache.join("revisions"))?.count(), 1);
        {
            let mut archive = tar::Builder::new(File::create(&original)?);
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_size(0);
            header.set_mode(0o777);
            archive.append_link(&mut header, "outside", "/etc/passwd")?;
            archive.finish()?;
        }
        assert!(fetch(&source, &cache, "HEAD").is_err());
        assert_eq!(
            fs::read_to_string(good.root.join("project/guide.md"))?,
            "hello"
        );
        Ok(())
    }
    #[test]
    fn archive_integrity_verifies_bytes_and_preserves_previous_snapshot() -> Result<()> {
        let fixture = tempfile::tempdir()?;
        let original = fixture.path().join("source.crate");
        let write_archive = |content: &[u8]| -> Result<()> {
            let mut archive = tar::Builder::new(File::create(&original)?);
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            archive.append_data(&mut header, "project/guide.md", content)?;
            archive.finish()?;
            Ok(())
        };
        write_archive(b"good")?;
        let mut source = source(SourceKind::Archive, &original);
        source.checksum = Some(format!("sha256:{}", hash_file(&original)?));
        let cache = fixture.path().join("cache");
        let good = fetch(&source, &cache, "HEAD")?;
        write_archive(b"changed")?;
        let error = fetch(&source, &cache, "HEAD").unwrap_err();
        assert!(error.to_string().contains("checksum mismatch"));
        assert_eq!(
            fs::read_to_string(good.root.join("project/guide.md"))?,
            "good"
        );
        assert_eq!(fs::read_dir(cache.join("revisions"))?.count(), 1);
        assert_eq!(fs::read_dir(cache.join("archives"))?.count(), 1);
        let bytes = fs::read(&original)?;
        source.checksum = Some(format!(
            "sha512-base64:{}",
            base64::engine::general_purpose::STANDARD.encode(Sha512::digest(&bytes))
        ));
        let updated = fetch(&source, &cache, "HEAD")?;
        assert_eq!(
            fs::read_to_string(updated.root.join("project/guide.md"))?,
            "changed"
        );
        // Invalid archive bytes fail integrity validation before any extraction.
        fs::write(&original, b"not an archive")?;
        assert!(fetch(&source, &cache, "HEAD")
            .unwrap_err()
            .to_string()
            .contains("checksum mismatch"));
        assert_eq!(fs::read_dir(cache.join("revisions"))?.count(), 2);
        assert_eq!(fs::read_dir(cache.join("archives"))?.count(), 2);
        Ok(())
    }
    #[test]
    fn web_markdown_refresh_is_content_addressed_and_keeps_old_response() -> Result<()> {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let server = std::thread::spawn(move || {
            for body in ["# First docs", "# Changed docs"] {
                let (mut socket, _) = listener.accept().unwrap();
                let mut request = [0u8; 4096];
                assert!(socket.read(&mut request).unwrap() > 0);
                write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
        });
        let fixture = tempfile::tempdir()?;
        let mut source = source(SourceKind::Url, fixture.path());
        source.location = format!("http://{address}/llms-full.txt");
        let cache = fixture.path().join("cache");
        let old = fetch(&source, &cache, "HEAD")?;
        let new = fetch(&source, &cache, "HEAD")?;
        server.join().unwrap();
        assert_ne!(old.revision, new.revision);
        assert_eq!(
            fs::read_to_string(old.root.join("document.md"))?,
            "# First docs"
        );
        assert_eq!(
            fs::read_to_string(new.root.join("document.md"))?,
            "# Changed docs"
        );
        assert!(new.provenance.contains("/llms-full.txt"));
        Ok(())
    }
    #[test]
    fn llms_manifest_snapshots_all_pages_and_rejects_partial_refresh() -> Result<()> {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let server = std::thread::spawn(move || {
            let index = "# Docs\n- [A](a.md)\n- [B](b.md)\n";
            for _ in 0..6 {
                let (mut socket, _) = listener.accept().unwrap();
                let mut request = [0u8; 4096];
                let count = socket.read(&mut request).unwrap();
                assert!(count > 0);
                let request = String::from_utf8_lossy(&request[..count]);
                let (status, body) = if request.starts_with("GET /llms.txt ") {
                    ("200 OK", index)
                } else if request.starts_with("GET /a.md ") {
                    ("200 OK", "# Page A")
                } else {
                    ("200 OK", "# Page B")
                };
                write!(socket, "HTTP/1.1 {status}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0u8; 4096];
            assert!(socket.read(&mut request).unwrap() > 0);
            let body = "[Missing](missing.md)";
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            let (mut socket, _) = listener.accept().unwrap();
            assert!(socket.read(&mut request).unwrap() > 0);
            write!(
                socket,
                "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
        });
        let fixture = tempfile::tempdir()?;
        let mut source = source(SourceKind::Url, fixture.path());
        source.location = format!("http://{address}/llms.txt");
        let cache = fixture.path().join("cache");
        let first = fetch(&source, &cache, "HEAD")?;
        assert_eq!(fs::read_dir(first.root.join("pages"))?.count(), 2);
        let urls: std::collections::BTreeMap<String, String> =
            serde_json::from_slice(&fs::read(first.root.join(".rtfm-urls.json"))?)?;
        assert_eq!(urls.len(), 3);
        assert!(urls.values().any(|url| url.ends_with("/a.md")));
        assert_eq!(fetch(&source, &cache, "HEAD")?.revision, first.revision);
        assert!(fetch(&source, &cache, "HEAD").is_err());
        server.join().unwrap();
        assert_eq!(fs::read_dir(cache.join("revisions"))?.count(), 1);
        assert_eq!(fs::read_dir(first.root.join("pages"))?.count(), 2);
        Ok(())
    }
    #[test]
    fn local_single_file_import_retains_name_and_previous_content() -> Result<()> {
        let fixture = tempfile::tempdir()?;
        let original = fixture.path().join("api.md");
        fs::write(&original, "old API")?;
        let source = source(SourceKind::Local, &original);
        let cache = fixture.path().join("cache");
        let old = fetch(&source, &cache, "HEAD")?;
        fs::write(&original, "new API")?;
        let new = fetch(&source, &cache, "HEAD")?;
        assert_ne!(old.revision, new.revision);
        assert_eq!(fs::read_to_string(old.root.join("api.md"))?, "old API");
        assert_eq!(fs::read_to_string(new.root.join("api.md"))?, "new API");
        Ok(())
    }
    #[test]
    fn retained_snapshot_verification_rejects_manual_changes_and_missing_roots() -> Result<()> {
        let fixture = tempfile::tempdir()?;
        let original = fixture.path().join("guide.md");
        fs::write(&original, "original docs")?;
        let source = source(SourceKind::Local, &original);
        let snapshot = fetch(&source, &fixture.path().join("cache"), "HEAD")?;
        verify(&snapshot)?;
        fs::write(snapshot.root.join("guide.md"), "manual edit")?;
        assert!(verify(&snapshot).is_err());
        fs::remove_dir_all(&snapshot.root)?;
        assert!(verify(&snapshot).is_err());
        Ok(())
    }
}
