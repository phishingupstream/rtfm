use crate::{
    cache::Snapshot,
    chunker::{Chunk, FileDigest},
};
use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
pub struct Store {
    pub db: Connection,
}
#[derive(Debug, Serialize)]
pub struct Version {
    pub library: String,
    pub version: String,
    pub revision: String,
    pub current: bool,
    pub root: PathBuf,
    pub provenance: String,
    pub updated_at: u64,
}
#[derive(Debug, Serialize)]
pub struct VersionSummary {
    #[serde(flatten)]
    pub snapshot: Version,
    pub chunks: usize,
}
#[derive(Debug, Serialize)]
pub struct Hit {
    pub id: i64,
    pub library: String,
    pub version: String,
    pub revision: String,
    pub kind: String,
    pub path: String,
    pub start_line: usize,
    pub end_line: usize,
    pub heading: String,
    pub heading_truncated: bool,
    pub text: String,
    pub score: f64,
    pub provenance: String,
    pub local_path: PathBuf,
}
impl Store {
    pub fn open(cache: &Path) -> Result<Self> {
        std::fs::create_dir_all(cache)?;
        let db = Connection::open(cache.join("index.sqlite3"))?;
        db.busy_timeout(std::time::Duration::from_secs(30))?;
        db.execute_batch("PRAGMA journal_mode=WAL;
            CREATE TABLE IF NOT EXISTS versions (
                library TEXT NOT NULL, reference TEXT NOT NULL, revision TEXT NOT NULL,
                current INTEGER NOT NULL, root TEXT NOT NULL, provenance TEXT NOT NULL,
                updated_at INTEGER NOT NULL, PRIMARY KEY(library,reference,revision));
            CREATE VIRTUAL TABLE IF NOT EXISTS chunks USING fts5(
                library UNINDEXED, revision UNINDEXED, kind UNINDEXED, path,
                heading, start_line UNINDEXED, end_line UNINDEXED, text,
                tokenize='unicode61 tokenchars _');
            CREATE TABLE IF NOT EXISTS indexed (library TEXT NOT NULL, revision TEXT NOT NULL, signature TEXT NOT NULL, PRIMARY KEY(library,revision));
            CREATE TABLE IF NOT EXISTS file_entries (library TEXT NOT NULL,revision TEXT NOT NULL,path TEXT NOT NULL,digest TEXT NOT NULL,PRIMARY KEY(library,revision,path));
            CREATE INDEX IF NOT EXISTS file_lookup ON file_entries(library,path,digest);
            CREATE TABLE IF NOT EXISTS update_checks (library TEXT NOT NULL,reference TEXT NOT NULL,checked_at INTEGER NOT NULL,PRIMARY KEY(library,reference));")?;
        Ok(Self { db })
    }
    pub fn last_update(&self, library: &str, reference: &str) -> Result<Option<u64>> {
        Ok(self
            .db
            .query_row(
                "SELECT updated_at FROM versions WHERE library=?1 AND reference=?2 AND current=1",
                params![library, reference],
                |r| r.get(0),
            )
            .optional()?)
    }
    pub fn last_check(&self, library: &str, reference: &str) -> Result<Option<u64>> {
        Ok(self
            .db
            .query_row(
                "SELECT checked_at FROM update_checks WHERE library=?1 AND reference=?2",
                params![library, reference],
                |r| r.get(0),
            )
            .optional()?)
    }
    pub fn record_check(&self, library: &str, reference: &str) -> Result<()> {
        self.db.execute("INSERT INTO update_checks VALUES (?1,?2,?3) ON CONFLICT(library,reference) DO UPDATE SET checked_at=excluded.checked_at",params![library,reference,now()])?;
        Ok(())
    }
    pub fn needs_index(&self, library: &str, revision: &str, signature: &str) -> Result<bool> {
        let previous: Option<String> = self
            .db
            .query_row(
                "SELECT signature FROM indexed WHERE library=?1 AND revision=?2",
                params![library, revision],
                |r| r.get(0),
            )
            .optional()?;
        Ok(previous.as_deref() != Some(signature))
    }
    pub fn cached_files<'a>(
        &'a mut self,
        library: &'a str,
        signature: &'a str,
    ) -> Result<impl FnMut(&str, &str) -> Result<Option<Vec<Chunk>>> + 'a> {
        // FTS cannot index equality on its UNINDEXED identity columns. Build a
        // connection-local row-ID lookup once; keep chunk text in the FTS table.
        self.db.execute_batch("CREATE TEMP TABLE IF NOT EXISTS reuse_chunks (id INTEGER PRIMARY KEY,revision TEXT,path TEXT);
            CREATE INDEX IF NOT EXISTS reuse_lookup ON reuse_chunks(revision,path);
            DELETE FROM reuse_chunks;")?;
        let has_previous: bool = self.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM indexed WHERE library=?1 AND signature=?2)",
            params![library, signature],
            |r| r.get(0),
        )?;
        if has_previous {
            self.db.execute(
                "INSERT INTO reuse_chunks SELECT rowid,revision,path FROM chunks c
                 WHERE library=?1 AND kind='docs' AND EXISTS (
                     SELECT 1 FROM indexed i WHERE i.library=c.library AND i.revision=c.revision AND i.signature=?2
                 )",
                params![library, signature],
            )?;
        }
        Ok(move |path: &str, digest: &str| self.cached_file(library, path, digest, signature))
    }
    fn cached_file(
        &self,
        library: &str,
        path: &str,
        digest: &str,
        signature: &str,
    ) -> Result<Option<Vec<Chunk>>> {
        let revision: Option<String>=self.db.query_row("SELECT f.revision FROM file_entries f JOIN indexed i ON i.library=f.library AND i.revision=f.revision WHERE f.library=?1 AND f.path=?2 AND f.digest=?3 AND i.signature=?4 LIMIT 1",params![library,path,digest,signature],|r|r.get(0)).optional()?;
        let Some(revision) = revision else {
            return Ok(None);
        };
        let mut stmt = self.db.prepare(
            "SELECT c.path,c.kind,c.heading,c.start_line,c.end_line,c.text
            FROM reuse_chunks r JOIN chunks c ON c.rowid=r.id
            WHERE r.revision=?1 AND r.path=?2 ORDER BY c.start_line,c.rowid",
        )?;
        let chunks = stmt
            .query_map(params![revision, path], |r| {
                Ok(Chunk {
                    path: r.get(0)?,
                    kind: "docs",
                    heading: r.get(2)?,
                    start_line: r.get(3)?,
                    end_line: r.get(4)?,
                    text: r.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(Some(chunks))
    }
    pub fn index(
        &mut self,
        library: &str,
        reference: &str,
        snapshot: &Snapshot,
        chunks: &[Chunk],
        files: &[FileDigest],
        signature: &str,
    ) -> Result<()> {
        // Acquire the writer before reading its publication state. A deferred
        // read-to-write upgrade can fail immediately when another writer probes.
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let previous: Option<String> = tx
            .query_row(
                "SELECT signature FROM indexed WHERE library=?1 AND revision=?2",
                params![library, snapshot.revision],
                |r| r.get(0),
            )
            .optional()?;
        if previous.as_deref() != Some(signature) {
            // Seed legacy IDs before deleting chunks, inside the writer transaction.
            // Readers must not acquire a write lock just to open the cache.
            tx.execute_batch("CREATE TABLE IF NOT EXISTS chunk_ids (id INTEGER PRIMARY KEY AUTOINCREMENT);
                INSERT OR IGNORE INTO chunk_ids(id) SELECT max(rowid) FROM chunks HAVING max(rowid) IS NOT NULL;")?;
            tx.execute(
                "DELETE FROM chunks WHERE library=?1 AND revision=?2",
                params![library, snapshot.revision],
            )?;
            {
                let mut insert = tx.prepare("INSERT INTO chunks(rowid,library,revision,kind,path,heading,start_line,end_line,text) VALUES (?9,?1,?2,?3,?4,?5,?6,?7,?8)")?;
                for chunk in chunks {
                    tx.execute("INSERT INTO chunk_ids DEFAULT VALUES", [])?;
                    let id = tx.last_insert_rowid();
                    insert.execute(params![
                        library,
                        snapshot.revision,
                        chunk.kind,
                        chunk.path,
                        chunk.heading,
                        chunk.start_line,
                        chunk.end_line,
                        chunk.text,
                        id
                    ])?;
                }
            }
            tx.execute(
                "DELETE FROM file_entries WHERE library=?1 AND revision=?2",
                params![library, snapshot.revision],
            )?;
            for file in files {
                tx.execute(
                    "INSERT INTO file_entries VALUES (?1,?2,?3,?4)",
                    params![library, snapshot.revision, file.path, file.digest],
                )?;
            }
            tx.execute("INSERT INTO indexed VALUES (?1,?2,?3) ON CONFLICT(library,revision) DO UPDATE SET signature=excluded.signature",params![library,snapshot.revision,signature])?;
        }
        tx.execute(
            "UPDATE versions SET current=0 WHERE library=?1 AND reference=?2",
            params![library, reference],
        )?;
        tx.execute("INSERT INTO versions VALUES (?1,?2,?3,1,?4,?5,?6) ON CONFLICT(library,reference,revision) DO UPDATE SET current=1,root=excluded.root,provenance=excluded.provenance,updated_at=excluded.updated_at", params![library,reference,snapshot.revision,snapshot.root.to_string_lossy(),snapshot.provenance,now()])?;
        crate::cache::check_interrupted()?;
        if previous.as_deref() != Some(signature) {
            tx.execute("DELETE FROM chunk_ids", [])?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn versions(&self, library: Option<&str>) -> Result<Vec<Version>> {
        let mut stmt = self.db.prepare("SELECT library,reference,revision,current,root,provenance,updated_at FROM versions WHERE (?1 IS NULL OR library=?1) ORDER BY library,reference,updated_at DESC,revision")?;
        let rows = stmt.query_map([library], version_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
    pub fn chunk_count(&self, library: &str, revision: &str) -> Result<usize> {
        Ok(self.db.query_row(
            "SELECT count(*) FROM chunks WHERE library=?1 AND revision=?2",
            params![library, revision],
            |r| r.get(0),
        )?)
    }
    pub fn version_summaries(&self, library: Option<&str>) -> Result<Vec<VersionSummary>> {
        // Count the FTS content once, rather than scanning it for each retained version.
        let mut stmt = self.db.prepare("SELECT v.library,v.reference,v.revision,v.current,v.root,v.provenance,v.updated_at,coalesce(c.chunks,0)
            FROM versions v LEFT JOIN (
                SELECT library,revision,count(*) AS chunks FROM chunks
                WHERE (?1 IS NULL OR library=?1) GROUP BY library,revision
            ) c ON c.library=v.library AND c.revision=v.revision
            WHERE (?1 IS NULL OR v.library=?1)
            ORDER BY v.library,v.reference,v.updated_at DESC,v.revision")?;
        let rows = stmt.query_map([library], |r| {
            Ok(VersionSummary {
                snapshot: version_row(r)?,
                chunks: r.get(7)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
    fn select_versions(&self, selected: &[Version]) -> Result<()> {
        // Exact resolver identities are the sole authority for eligible evidence.
        self.db.execute_batch("CREATE TEMP TABLE IF NOT EXISTS search_selection (library TEXT, revision TEXT, reference TEXT, PRIMARY KEY(library,revision)); DELETE FROM search_selection;")?;
        for v in selected {
            self.db.execute(
                "INSERT OR IGNORE INTO search_selection VALUES (?1,?2,?3)",
                params![v.library, v.revision, v.version],
            )?;
        }
        Ok(())
    }
    pub fn documentation_count(&self, selected: &[Version]) -> Result<usize> {
        if selected.is_empty() {
            return Ok(0);
        }
        if let [version] = selected {
            return Ok(self.db.query_row(
                "SELECT count(*) FROM chunks WHERE library=?1 AND revision=?2 AND kind='docs'",
                params![version.library, version.revision],
                |r| r.get(0),
            )?);
        }
        self.select_versions(selected)?;
        // Keep FTS as the outer scan; probing it once per selected revision is quadratic.
        Ok(self.db.query_row(
            "SELECT count(*) FROM chunks c WHERE c.kind='docs' AND EXISTS (
                SELECT 1 FROM search_selection s WHERE s.library=c.library AND s.revision=c.revision
            )",
            [],
            |r| r.get(0),
        )?)
    }
    pub fn search_selected(
        &self,
        query: &str,
        selected: &[Version],
        limit: usize,
    ) -> Result<Vec<Hit>> {
        let tokens: Vec<String> = query
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .filter(|s| !s.is_empty())
            .map(|s| format!("\"{}\"", s.replace('"', "\"\"")))
            .collect();
        if tokens.is_empty() {
            return Err(crate::output::Failure::new(
                "query_invalid",
                "query must contain a letter, digit, or identifier",
                serde_json::json!({}),
            )
            .into());
        }
        if limit == 0 {
            return Err(crate::output::Failure::new(
                "argument_invalid",
                "result limit must be greater than zero",
                serde_json::json!({}),
            )
            .into());
        }
        // Treat punctuation as plain input, never as executable FTS query syntax.
        let expression = tokens.join(" OR ");
        self.select_versions(selected)?;
        let mut stmt = self.db.prepare("SELECT c.rowid,c.library,v.reference,c.revision,c.kind,c.path,c.start_line,c.end_line,c.heading,c.text,bm25(chunks,0,0,0,2,3,0,0,1),v.provenance,v.root
            FROM chunks c JOIN search_selection s ON s.library=c.library AND s.revision=c.revision
            JOIN versions v ON v.library=s.library AND v.revision=s.revision AND v.reference=s.reference
            WHERE chunks MATCH ?1 AND c.kind='docs'
            ORDER BY bm25(chunks,0,0,0,2,3,0,0,1),c.library,c.path,c.start_line,c.rowid LIMIT ?2")?;
        let rows = stmt
            .query_map(params![expression, limit], hit_row)
            .context("search local FTS5 index")?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
    pub fn show(&self, id: i64) -> Result<Hit> {
        self.db.query_row("SELECT c.rowid,c.library,v.reference,c.revision,c.kind,c.path,c.start_line,c.end_line,c.heading,c.text,0.0,v.provenance,v.root FROM chunks c JOIN versions v ON v.library=c.library AND v.revision=c.revision WHERE c.rowid=?1 ORDER BY v.current DESC LIMIT 1", [id], hit_row).optional()?.with_context(|| crate::output::Failure::new("chunk_missing", format!("chunk {id} is not in the cache; search again for its current ID"), serde_json::json!({"id":id})))
    }
    pub fn counts(&self) -> Result<(usize, usize, usize)> {
        Ok((
            self.db
                .query_row("SELECT count(DISTINCT library) FROM versions", [], |r| {
                    r.get(0)
                })?,
            self.db
                .query_row("SELECT count(*) FROM versions", [], |r| r.get(0))?,
            self.db
                .query_row("SELECT count(*) FROM chunks", [], |r| r.get(0))?,
        ))
    }
}
fn version_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Version> {
    Ok(Version {
        library: r.get(0)?,
        version: r.get(1)?,
        revision: r.get(2)?,
        current: r.get(3)?,
        root: PathBuf::from(r.get::<_, String>(4)?),
        provenance: r.get(5)?,
        updated_at: r.get(6)?,
    })
}
fn hit_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Hit> {
    let path: String = r.get(5)?;
    let root: String = r.get(12)?;
    let mut heading: String = r.get(8)?;
    let heading_truncated = heading.len() > 512;
    if heading_truncated {
        let mut end = 512;
        while !heading.is_char_boundary(end) {
            end -= 1;
        }
        heading.truncate(end);
    }
    Ok(Hit {
        id: r.get(0)?,
        library: r.get(1)?,
        version: r.get(2)?,
        revision: r.get(3)?,
        kind: r.get(4)?,
        local_path: Path::new(&root).join(&path),
        path,
        start_line: r.get(6)?,
        end_line: r.get(7)?,
        heading,
        heading_truncated,
        text: r.get(9)?,
        score: -r.get::<_, f64>(10)?,
        provenance: r.get(11)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Source, SourceKind};
    #[test]
    fn summaries_count_shared_revisions_retained_versions_and_empty_snapshots() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let mut store = Store::open(temp.path())?;
        let mut snapshot = Snapshot {
            revision: "old".into(),
            root: temp.path().into(),
            provenance: "fixture".into(),
        };
        let chunks = crate::chunker::chunk("# Guide\ncachetoken", "guide.md", "docs");
        store.index("demo", "stable", &snapshot, &chunks, &[], "rules")?;
        store.index("demo", "preview", &snapshot, &[], &[], "rules")?;
        snapshot.revision = "new".into();
        store.index("demo", "stable", &snapshot, &[], &[], "rules")?;
        store.index("other", "stable", &snapshot, &chunks, &[], "rules")?;

        let all = store.version_summaries(None)?;
        assert_eq!(all.len(), 4);
        for summary in &all {
            assert_eq!(
                summary.chunks,
                store.chunk_count(&summary.snapshot.library, &summary.snapshot.revision)?
            );
        }
        let scoped = store.version_summaries(Some("demo"))?;
        assert_eq!(scoped.len(), 3);
        assert!(scoped.iter().all(|v| v.snapshot.library == "demo"));
        assert!(scoped.iter().any(|v| v.chunks == 0 && v.snapshot.current));
        assert!(scoped.iter().any(|v| v.chunks == 1 && !v.snapshot.current));
        assert!(store.version_summaries(Some("missing"))?.is_empty());
        // Diagnostics deduplicate aliases, select exact revisions, and exclude legacy code.
        store.db.execute("INSERT INTO chunks(library,revision,kind,path,heading,start_line,end_line,text) VALUES ('demo','old','code','api.rs','',1,1,'legacy code')", [])?;
        assert_eq!(store.documentation_count(&store.versions(None)?)?, 2);
        assert_eq!(
            store.documentation_count(&store.versions(Some("demo"))?)?,
            1
        );
        assert_eq!(
            store.documentation_count(&store.versions(Some("other"))?)?,
            1
        );
        assert_eq!(
            store.documentation_count(&store.versions(Some("missing"))?)?,
            0
        );
        let empty = store
            .versions(Some("demo"))?
            .into_iter()
            .filter(|v| v.revision == "new")
            .collect::<Vec<_>>();
        assert_eq!(store.documentation_count(&empty)?, 0);
        Ok(())
    }

    #[test]
    fn reuse_keeps_library_signature_digest_and_fragment_order() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let mut store = Store::open(temp.path())?;
        let snapshot = Snapshot {
            revision: "same-revision".into(),
            root: temp.path().into(),
            provenance: "fixture".into(),
        };
        let path = "文档/guide.md";
        let files = [FileDigest {
            path: path.into(),
            digest: "same-digest".into(),
        }];
        let text = "🦀".repeat(2000);
        let chunks = crate::chunker::chunk(&text, path, "docs");
        store.index("demo", "stable", &snapshot, &chunks, &files, "rules")?;
        let other = crate::chunker::chunk("wrong library", path, "docs");
        store.index("other", "stable", &snapshot, &other, &files, "rules")?;
        // Legacy code rows must never become reused documentation.
        store.db.execute("INSERT INTO chunks(library,revision,kind,path,heading,start_line,end_line,text) VALUES ('demo','same-revision','code',?1,'',1,1,'legacy code')", [path])?;
        {
            let mut reuse = store.cached_files("demo", "rules")?;
            let reused = reuse(path, "same-digest")?.unwrap();
            assert_eq!(reused.len(), chunks.len());
            assert_eq!(
                reused.iter().map(|c| c.text.as_str()).collect::<String>(),
                format!("{text}\n")
            );
            assert!(reused
                .iter()
                .all(|c| c.kind == "docs" && c.start_line == 1 && c.end_line == 1));
            assert!(reuse(path, "changed-digest")?.is_none());
            assert!(reuse("missing.md", "same-digest")?.is_none());
        }
        assert!(store.cached_files("demo", "changed-rules")?(path, "same-digest")?.is_none());
        assert_eq!(
            store.cached_files("other", "rules")?(path, "same-digest")?.unwrap()[0].text,
            "wrong library\n"
        );
        assert!(store.cached_files("missing", "rules")?(path, "same-digest")?.is_none());
        Ok(())
    }

    #[test]
    fn simultaneous_connections_keep_project_selections_private() {
        let temp = tempfile::tempdir().unwrap();
        let mut one = Store::open(temp.path()).unwrap();
        let snapshot = Snapshot {
            revision: "immutable".into(),
            root: temp.path().into(),
            provenance: "fixture".into(),
        };
        let chunks = [Chunk {
            path: "guide.md".into(),
            kind: "docs",
            heading: "Guide".into(),
            start_line: 1,
            end_line: 1,
            text: "tracerbullet".into(),
        }];
        one.index("demo", "stable", &snapshot, &chunks, &[], "rules")
            .unwrap();
        one.index("demo", "preview", &snapshot, &chunks, &[], "rules")
            .unwrap();
        let mut config = Config::default();
        config.sources.insert(
            "demo".into(),
            Source {
                checksum: None,
                kind: SourceKind::Local,
                location: "fixture".into(),
                refs: vec!["stable".into()],
                docs: vec![],
                code: vec![],
                exclude: vec![],
                enabled: true,
            },
        );
        let stable = crate::selection::resolve(&config, &one, None, None).unwrap();
        let two = Store::open(temp.path()).unwrap();
        config.selections.insert("demo".into(), "preview".into());
        let preview = crate::selection::resolve(&config, &two, None, None).unwrap();
        assert_eq!(
            one.search_selected("tracerbullet", &stable, 5).unwrap()[0].version,
            "stable"
        );
        assert_eq!(
            two.search_selected("tracerbullet", &preview, 5).unwrap()[0].version,
            "preview"
        );
    }
}
