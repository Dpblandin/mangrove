mod cache;
mod fetch;
mod file_browser;
mod graph;
pub mod model;
mod repository_context;
mod working_tree;
mod worktree_filter;

use std::{
    collections::{HashMap, VecDeque},
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc, Condvar, Mutex, Weak,
    },
    thread,
    time::Instant,
};

use git2::{DiffFindOptions, DiffOptions, Oid, Repository, Sort};
use model::*;
use rusqlite::{params, OptionalExtension};
use sha2::{Digest, Sha256};

pub type Result<T> = std::result::Result<T, String>;
const BATCH_SIZE: usize = 256;
const MAX_FILE_SIZE: i64 = 2 * 1024 * 1024;
const MAX_DIFF_LINES: usize = 2_000;
const MAX_CHANGED_FILES: usize = 5_000;
const MAX_OPEN_REPOSITORIES: usize = 8;
const MAX_REPOSITORY_NAMES: usize = 6;

pub fn repository_names(paths: &[String]) -> Result<Vec<RepositoryName>> {
    if paths.len() > MAX_REPOSITORY_NAMES {
        return Err("La lecture des noms est limitée à six dépôts.".into());
    }
    Ok(paths
        .iter()
        .map(|path| {
            let name = (|| -> Result<String> {
                let repository = open_source_repository(path)?;
                let source = repository
                    .workdir()
                    .unwrap_or(repository.path())
                    .canonicalize()
                    .map_err(|e| e.to_string())?;
                Ok(repository_display_name(&repository, &source))
            })()
            .ok();
            RepositoryName {
                path: path.clone(),
                name,
            }
        })
        .collect())
}

fn search_cte(include_sha: bool) -> String {
    let sha = if include_sha {
        "UNION SELECT id FROM commits WHERE sha GLOB ?2"
    } else {
        ""
    };
    format!(
        "WITH matched AS (
            SELECT rowid AS id FROM search WHERE search MATCH ?1
            {sha}
            UNION
            SELECT c.id FROM commits c JOIN refs r ON r.sha = c.sha
            WHERE r.name LIKE ?3 ESCAPE '\\' OR r.label LIKE ?3 ESCAPE '\\'
        )"
    )
}

struct Session {
    info: RepositoryInfo,
    fingerprint: String,
    index_run: String,
    source: PathBuf,
    database: PathBuf,
    write_lock: Arc<Mutex<()>>,
    indexed: AtomicUsize,
    complete: AtomicBool,
    cancel: AtomicBool,
    error: Mutex<Option<String>>,
    opened: Instant,
}

struct CommitMetadata {
    summary: String,
    message: String,
    author: String,
    email: String,
    time: i64,
}

struct IndexedCommit {
    sha: String,
    parents: String,
    graph: String,
    metadata: Option<CommitMetadata>,
}

impl Session {
    fn progress(&self) -> IndexProgress {
        IndexProgress {
            indexed: self.indexed.load(Ordering::Acquire),
            complete: self.complete.load(Ordering::Acquire),
            error: self.error.lock().unwrap().clone(),
        }
    }
}

#[derive(Default)]
struct IndexQueue {
    pending: VecDeque<Arc<Session>>,
    stopped: bool,
}

#[derive(Default)]
struct IndexPool {
    queue: Mutex<IndexQueue>,
    changed: Condvar,
}

impl IndexPool {
    fn start() -> Arc<Self> {
        let pool = Arc::new(Self::default());
        for _ in 0..2 {
            let worker = pool.clone();
            thread::spawn(move || worker.run());
        }
        pool
    }

    fn submit(&self, session: Arc<Session>) {
        let mut queue = self.queue.lock().unwrap();
        queue
            .pending
            .retain(|pending| !pending.cancel.load(Ordering::Acquire));
        // At most eight live sessions exist; cancelled generations are removed
        // before enqueueing. Rapid refreshes never create extra OS threads.
        queue.pending.push_back(session);
        self.changed.notify_one();
    }

    fn run(&self) {
        loop {
            let session = {
                let mut queue = self.queue.lock().unwrap();
                while queue.pending.is_empty() && !queue.stopped {
                    queue = self.changed.wait(queue).unwrap();
                }
                if queue.stopped {
                    return;
                }
                queue.pending.pop_front().unwrap()
            };
            if session.cancel.load(Ordering::Acquire) {
                continue;
            }
            if let Err(error) = index_repository(&session) {
                if !session.cancel.load(Ordering::Acquire) {
                    *session.error.lock().unwrap() = Some(error);
                    session.complete.store(true, Ordering::Release);
                }
            }
        }
    }

    fn stop(&self) {
        let mut queue = self.queue.lock().unwrap();
        queue.stopped = true;
        queue.pending.clear();
        self.changed.notify_all();
    }
}

pub struct Workspace {
    cache_dir: PathBuf,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    write_locks: Mutex<HashMap<String, Weak<Mutex<()>>>>,
    generation: AtomicU64,
    workers: Arc<IndexPool>,
    fetch_lock: Mutex<()>,
}

impl Workspace {
    pub fn new(cache_dir: impl Into<PathBuf>) -> Result<Self> {
        let cache_dir = resolve_cache_path(&cache_dir.into())?;
        Ok(Self {
            cache_dir,
            sessions: Mutex::new(HashMap::new()),
            write_locks: Mutex::new(HashMap::new()),
            generation: AtomicU64::new(1),
            workers: IndexPool::start(),
            fetch_lock: Mutex::new(()),
        })
    }

    pub fn fetch_repository(&self, session_id: &str) -> Result<FetchResult> {
        let session = self.session(session_id)?;
        let _guard = self
            .fetch_lock
            .try_lock()
            .map_err(|_| "Une récupération des références distantes est déjà en cours.")?;
        fetch::fetch(&session.source)
    }

    pub fn open_repository(&self, path: &str) -> Result<RepositoryInfo> {
        self.open_repository_with_solo(path, &[])
    }

    pub fn open_repository_with_solo(&self, path: &str, solo: &[String]) -> Result<RepositoryInfo> {
        if solo.len() > 64 || solo.iter().any(|name| name.len() > 1024) {
            return Err(
                "Solo accepte au plus 64 branches, avec des noms de 1 024 octets maximum.".into(),
            );
        }
        // Read repository state in the same critical section that publishes its
        // session; a delayed open must not replace a newer fingerprint.
        let mut sessions = self.sessions.lock().unwrap();
        let repository = open_source_repository(path)?;
        let git_dir = repository
            .path()
            .canonicalize()
            .map_err(|e| e.to_string())?;
        let source = repository
            .workdir()
            .unwrap_or(repository.path())
            .canonicalize()
            .map_err(|e| e.to_string())?;
        let common_dir = repository
            .commondir()
            .canonicalize()
            .map_err(|e| e.to_string())?;
        let cache_dir = resolve_cache_path(&self.cache_dir)?;
        let common_workdir = if git_dir != common_dir {
            Repository::open(&common_dir).ok().and_then(|repository| {
                repository
                    .workdir()
                    .and_then(|workdir| workdir.canonicalize().ok())
            })
        } else {
            None
        };
        if [&source, &git_dir, &common_dir]
            .iter()
            .any(|root| cache_dir.starts_with(root))
            || common_workdir
                .as_ref()
                .is_some_and(|root| cache_dir.starts_with(root))
        {
            return Err("Le cache doit être situé en dehors du dépôt consulté.".into());
        }
        fs::create_dir_all(&cache_dir)
            .map_err(|e| format!("Impossible de créer le cache : {e}"))?;
        let key = format!("{:x}", Sha256::digest(git_dir.to_string_lossy().as_bytes()));
        let head = repository
            .head()
            .ok()
            .and_then(|head| head.peel_to_commit().ok());
        let head_sha = head.as_ref().map(|head| head.id().to_string());
        let head_name = repository_head_name(&repository);
        let mut references = Vec::new();
        for reference in repository.references().map_err(|e| e.to_string())? {
            let reference = reference.map_err(|e| e.to_string())?;
            // T3 snapshots are operational refs, not history roots. Filtering
            // here also keeps their updates out of the traversal fingerprint.
            let raw_name = reference.name_bytes();
            if raw_name == b"refs/stash"
                || [
                    b"refs/t3/checkpoints/".as_slice(),
                    b"refs/notes/",
                    b"refs/prefetch/",
                ]
                .iter()
                .any(|prefix| raw_name.starts_with(prefix))
            {
                continue;
            }
            if reference.symbolic_target_bytes().is_some() {
                continue;
            }
            let Ok(commit) = reference.peel_to_commit() else {
                continue;
            };
            let kind = if reference.is_branch() {
                "local"
            } else if reference.is_remote() {
                "remote"
            } else if reference.is_tag() {
                "tag"
            } else {
                "other"
            };
            let name = reference.name().map(str::to_owned).unwrap_or_else(|_| {
                let mut key = String::from("git-bytes:");
                for byte in raw_name {
                    use std::fmt::Write;
                    write!(&mut key, "{byte:02x}").unwrap();
                }
                key
            });
            references.push(Reference {
                label: String::from_utf8_lossy(reference.shorthand_bytes()).into_owned(),
                name,
                kind: kind.into(),
                sha: commit.id().to_string(),
            });
        }
        references.sort_by(|a, b| a.name.cmp(&b.name));
        // References deleted externally leave the filter instead of keeping a stale root.
        let mut solo_refs: Vec<_> = solo
            .iter()
            .filter(|name| {
                references.iter().any(|reference| {
                    &reference.name == *name
                        && matches!(reference.kind.as_str(), "local" | "remote")
                })
            })
            .cloned()
            .collect();
        solo_refs.sort();
        solo_refs.dedup();
        let mut remotes = Vec::new();
        for name in repository
            .remotes()
            .map_err(|e| e.to_string())?
            .iter()
            .filter_map(|name| name.ok().flatten())
        {
            if let Ok(remote) = repository.find_remote(name) {
                if let Ok(url) = remote.url() {
                    remotes.push(url.to_owned());
                }
            }
        }
        // Shallow boundaries affect traversal and the effective parents exposed
        // by libgit2, even when an external deepen/fetch leaves every ref intact.
        let mut shallow_paths = vec![common_dir.join("shallow")];
        if git_dir != common_dir {
            shallow_paths.push(git_dir.join("shallow"));
        }
        let mut shallow = Vec::with_capacity(shallow_paths.len());
        for path in shallow_paths {
            shallow.push(match fs::read(path) {
                Ok(shallow) => shallow,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
                Err(error) => {
                    return Err(format!(
                        "Impossible de lire les frontières shallow : {error}"
                    ))
                }
            });
        }
        let fingerprint = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&(
                    6u32,
                    &head_sha,
                    &head_name,
                    &references,
                    &remotes,
                    &shallow,
                    &solo_refs
                ))
                .map_err(|e| e.to_string())?,
            )
        );
        if let Some(session) = sessions.get(&key) {
            if session.fingerprint == fingerprint
                && !session.cancel.load(Ordering::Acquire)
                && session.error.lock().unwrap().is_none()
            {
                let reusable = (|| {
                    let mut connection = cache::connect(&session.database, true)?;
                    let snapshot = connection.transaction().map_err(|e| e.to_string())?;
                    is_current(&snapshot, session)
                })();
                if reusable == Ok(true) {
                    let mut info = session.info.clone();
                    info.index = session.progress();
                    return Ok(info);
                }
            }
            session.cancel.store(true, Ordering::Release);
        }
        let write_lock = {
            let mut locks = self.write_locks.lock().unwrap();
            locks.retain(|_, lock| lock.strong_count() > 0);
            let lock = locks.get(&key).and_then(Weak::upgrade).unwrap_or_default();
            locks.insert(key.clone(), Arc::downgrade(&lock));
            lock
        };
        let database = cache_dir.join(format!("{key}.sqlite"));
        let (indexed, complete, index_run) = {
            let _guard = write_lock.lock().unwrap();
            let mut connection = cache::connect(&database, false)?;
            cache::initialize(&connection)?;
            let complete = cache::meta(&connection, "fingerprint")?.as_deref()
                == Some(&fingerprint)
                && cache::meta(&connection, "complete")?.as_deref() == Some("1");
            let index_run = if complete {
                cache::meta(&connection, "index_run")?.unwrap_or_default()
            } else {
                new_index_run()
            };
            if !complete {
                let transaction = connection.transaction().map_err(|e| e.to_string())?;
                transaction
                    .execute_batch("DELETE FROM walk; DELETE FROM refs;")
                    .map_err(|e| e.to_string())?;
                cache::set_meta(&transaction, "fingerprint", &fingerprint)?;
                cache::set_meta(&transaction, "complete", "0")?;
                cache::set_meta(&transaction, "index_run", &index_run)?;
                for reference in &references {
                    transaction
                        .execute(
                            "INSERT INTO refs(name, label, sha) VALUES(?1, ?2, ?3)",
                            params![reference.name, reference.label, reference.sha],
                        )
                        .map_err(|e| e.to_string())?;
                }
                transaction.commit().map_err(|e| e.to_string())?;
            }
            let count = connection
                .query_row("SELECT COUNT(*) FROM walk", [], |row| {
                    cache::row_usize(row, 0)
                })
                .map_err(|e| e.to_string())?;
            (count, complete, index_run)
        };
        let info = RepositoryInfo {
            id: format!(
                "{}-{}",
                &key[..16],
                self.generation.fetch_add(1, Ordering::Relaxed)
            ),
            name: repository_display_name(&repository, &source),
            path: source.to_string_lossy().into_owned(),
            head: head_sha,
            head_name,
            references,
            solo_refs,
            remotes,
            index: IndexProgress {
                indexed,
                complete,
                error: None,
            },
        };
        let session = Arc::new(Session {
            info: info.clone(),
            fingerprint,
            index_run,
            source,
            database,
            write_lock,
            indexed: AtomicUsize::new(indexed),
            complete: AtomicBool::new(complete),
            cancel: AtomicBool::new(false),
            error: Mutex::new(None),
            opened: Instant::now(),
        });
        if !sessions.contains_key(&key) && sessions.len() >= MAX_OPEN_REPOSITORIES {
            if let Some(oldest) = sessions
                .iter()
                .min_by_key(|(_, value)| value.opened)
                .map(|(key, _)| key.clone())
            {
                if let Some(old) = sessions.remove(&oldest) {
                    old.cancel.store(true, Ordering::Release);
                }
            }
        }
        sessions.insert(key, session.clone());
        if !complete {
            self.workers.submit(session);
        }
        Ok(info)
    }

    fn session(&self, id: &str) -> Result<Arc<Session>> {
        self.sessions
            .lock()
            .unwrap()
            .values()
            .find(|session| session.info.id == id && !session.cancel.load(Ordering::Acquire))
            .cloned()
            .ok_or_else(|| "Ce dépôt a été fermé ou actualisé. Ouvrez-le à nouveau.".into())
    }

    pub fn index_status(&self, id: &str) -> Result<IndexProgress> {
        Ok(self.session(id)?.progress())
    }

    pub fn list_commits(
        &self,
        id: &str,
        offset: usize,
        limit: usize,
        query: &str,
    ) -> Result<CommitPage> {
        let session = self.session(id)?;
        let mut connection = cache::connect(&session.database, true)?;
        let connection = connection.transaction().map_err(|e| e.to_string())?;
        ensure_current(&connection, &session)?;
        let limit = limit.clamp(1, 256) as i64;
        let offset = i64::try_from(offset).map_err(|_| "Position invalide.")?;
        let query: String = query.trim().chars().take(256).collect();
        let (commits, total) = if query.is_empty() {
            let sql = format!(
                "SELECT {} FROM walk w JOIN commits c ON c.sha = w.sha
                 WHERE w.position >= ?1 ORDER BY w.position LIMIT ?2",
                cache::COLUMNS,
            );
            let mut statement = connection.prepare(&sql).map_err(|e| e.to_string())?;
            let commits = statement
                .query_map(params![offset, limit], cache::row_commit)
                .map_err(|e| e.to_string())?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| e.to_string())?;
            let total = connection
                .query_row("SELECT COUNT(*) FROM walk", [], |row| {
                    cache::row_usize(row, 0)
                })
                .map_err(|e| e.to_string())?;
            (commits, total)
        } else {
            let fts = cache::search_terms(&query);
            let sha = query
                .chars()
                .all(|character| character.is_ascii_hexdigit())
                .then(|| format!("{}*", query.to_ascii_lowercase()));
            let search_cte = search_cte(sha.is_some());
            let reference = cache::like_prefix(&query);
            let sql = format!(
                "{search_cte} SELECT {} FROM matched m
                 JOIN commits c ON c.id = m.id JOIN walk w ON w.sha = c.sha
                 ORDER BY w.position LIMIT ?4 OFFSET ?5",
                cache::COLUMNS,
            );
            let mut statement = connection.prepare(&sql).map_err(|e| e.to_string())?;
            let commits = statement
                .query_map(
                    params![fts, sha, reference, limit, offset],
                    cache::row_commit,
                )
                .map_err(|e| e.to_string())?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| e.to_string())?;
            let total = connection.query_row(
                &format!("{search_cte} SELECT COUNT(*) FROM matched m JOIN commits c ON c.id = m.id JOIN walk w ON w.sha = c.sha"),
                params![fts, sha, reference], |row| cache::row_usize(row, 0),
            ).map_err(|e| e.to_string())?;
            (commits, total)
        };
        let indexed = if query.is_empty() {
            total
        } else {
            connection
                .query_row("SELECT COUNT(*) FROM walk", [], |row| {
                    cache::row_usize(row, 0)
                })
                .map_err(|e| e.to_string())?
        };
        let error = session.error.lock().unwrap().clone();
        let complete =
            cache::meta(&connection, "complete")?.as_deref() == Some("1") || error.is_some();
        // Progress belongs to the same SQLite snapshot as the rows. Otherwise a
        // final worker commit could advertise complete=true with an older total.
        let index = IndexProgress {
            indexed,
            complete,
            error,
        };
        Ok(CommitPage {
            commits,
            total,
            offset: offset as usize,
            index,
        })
    }

    pub fn commit_position(&self, id: &str, sha: &str) -> Result<Option<usize>> {
        let session = self.session(id)?;
        let mut connection = cache::connect(&session.database, true)?;
        let connection = connection.transaction().map_err(|e| e.to_string())?;
        ensure_current(&connection, &session)?;
        connection
            .query_row("SELECT position FROM walk WHERE sha = ?1", [sha], |row| {
                cache::row_usize(row, 0)
            })
            .optional()
            .map_err(|e| e.to_string())
    }

    pub fn commit_details(&self, id: &str, sha: &str) -> Result<CommitDetails> {
        let session = self.session(id)?;
        let repository = Repository::open(&session.source).map_err(|e| e.to_string())?;
        let (commit, diff) = commit_diff(&repository, sha)?;
        let author = commit.author();
        let files = diff
            .deltas()
            .take(MAX_CHANGED_FILES)
            .enumerate()
            .map(|(index, delta)| changed_file(index, &delta))
            .collect();
        Ok(CommitDetails {
            sha: commit.id().to_string(),
            message: String::from_utf8_lossy(commit.message_bytes()).into_owned(),
            author: String::from_utf8_lossy(author.name_bytes()).into_owned(),
            email: String::from_utf8_lossy(author.email_bytes()).into_owned(),
            time: commit.time().seconds(),
            parents: commit.parent_ids().map(|id| id.to_string()).collect(),
            files,
            file_count: diff.deltas().len(),
        })
    }

    pub fn file_diff(&self, id: &str, sha: &str, file_index: usize) -> Result<FileDiff> {
        let session = self.session(id)?;
        let repository = Repository::open(&session.source).map_err(|e| e.to_string())?;
        let (_, diff) = commit_diff(&repository, sha)?;
        if file_index >= diff.deltas().len() {
            return Err("Fichier introuvable dans ce commit.".into());
        }
        render_file_diff(&diff, file_index)
    }

    pub fn list_worktrees(&self, id: &str) -> Result<WorktreeList> {
        let session = self.session(id)?;
        let repository = Repository::open(&session.source).map_err(|e| e.to_string())?;
        repository_context::worktrees(&repository)
    }

    pub fn list_stashes(&self, id: &str) -> Result<StashList> {
        let session = self.session(id)?;
        let repository = Repository::open(&session.source).map_err(|e| e.to_string())?;
        repository_context::stashes(&repository)
    }

    pub fn stash_details(&self, id: &str, sha: &str) -> Result<StashDetails> {
        let session = self.session(id)?;
        let repository = Repository::open(&session.source).map_err(|e| e.to_string())?;
        repository_context::stash_details(&repository, sha)
    }

    pub fn stash_diff(&self, id: &str, sha: &str, area: &str, file_key: &str) -> Result<FileDiff> {
        let session = self.session(id)?;
        let repository = Repository::open(&session.source).map_err(|e| e.to_string())?;
        repository_context::stash_diff(&repository, sha, area, file_key)
    }

    pub fn working_tree_status(&self, id: &str) -> Result<WorkingTreeStatus> {
        let session = self.session(id)?;
        let repository = Repository::open(&session.source).map_err(|e| e.to_string())?;
        working_tree::status(&repository)
    }

    pub fn file_content(&self, id: &str, sha: &str, file_key: &str) -> Result<FileContent> {
        let session = self.session(id)?;
        let repository = Repository::open(&session.source).map_err(|e| e.to_string())?;
        file_browser::content(&repository, sha, file_key)
    }

    pub fn file_history(
        &self,
        id: &str,
        sha: &str,
        file_key: &str,
        cursor: Option<&str>,
    ) -> Result<FileHistoryPage> {
        let session = self.session(id)?;
        let repository = Repository::open(&session.source).map_err(|e| e.to_string())?;
        file_browser::history(&repository, sha, file_key, cursor)
    }

    pub fn file_blame(&self, id: &str, sha: &str, file_key: &str) -> Result<FileBlame> {
        let session = self.session(id)?;
        let repository = Repository::open(&session.source).map_err(|e| e.to_string())?;
        file_browser::blame(&repository, sha, file_key)
    }

    pub fn file_revision_diff(&self, id: &str, sha: &str, file_key: &str) -> Result<FileDiff> {
        let session = self.session(id)?;
        let repository = Repository::open(&session.source).map_err(|e| e.to_string())?;
        file_browser::revision_diff(&repository, sha, file_key)
    }

    pub fn working_tree_diff(&self, id: &str, area: &str, file_key: &str) -> Result<FileDiff> {
        let session = self.session(id)?;
        let repository = Repository::open(&session.source).map_err(|e| e.to_string())?;
        working_tree::diff(&repository, area, file_key)
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        for session in self.sessions.lock().unwrap().values() {
            session.cancel.store(true, Ordering::Release);
        }
        self.workers.stop();
    }
}

// Resolve existing symlinks before creating anything, so a cache alias cannot
// accidentally redirect writes into the repository being inspected.
fn resolve_cache_path(path: &Path) -> Result<PathBuf> {
    let mut existing = std::path::absolute(path).map_err(|e| e.to_string())?;
    let mut missing = Vec::new();
    while !existing.exists() {
        missing.push(
            existing
                .file_name()
                .ok_or("Dossier de cache invalide.")?
                .to_owned(),
        );
        if !existing.pop() {
            return Err("Dossier de cache invalide.".into());
        }
    }
    let mut resolved = existing.canonicalize().map_err(|e| e.to_string())?;
    for component in missing.into_iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

fn new_index_run() -> String {
    static NEXT_RUN: AtomicU64 = AtomicU64::new(1);
    format!(
        "{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        NEXT_RUN.fetch_add(1, Ordering::Relaxed)
    )
}

fn ensure_current(connection: &rusqlite::Connection, session: &Session) -> Result<()> {
    if !is_current(connection, session)? {
        return Err("Ce dépôt a été actualisé. Ouvrez-le à nouveau.".into());
    }
    Ok(())
}

fn is_current(connection: &rusqlite::Connection, session: &Session) -> Result<bool> {
    Ok(!(session.cancel.load(Ordering::Acquire)
        || cache::meta(connection, "fingerprint")?.as_deref() != Some(&session.fingerprint)
        || cache::meta(connection, "index_run")?.as_deref() != Some(&session.index_run)))
}

fn open_source_repository(path: &str) -> Result<Repository> {
    let path = Path::new(path.trim());
    if !path.is_dir() {
        return Err("Choisissez un dossier contenant un dépôt Git local.".into());
    }
    Repository::open_ext(
        path,
        git2::RepositoryOpenFlags::empty(),
        &[] as &[&std::ffi::OsStr],
    )
    .map_err(|e| format!("Impossible d'ouvrir ce dépôt Git : {e}"))
}

fn repository_display_name(repository: &Repository, source: &Path) -> String {
    if !repository.is_worktree() {
        return source
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
    }
    let primary_name = Repository::open(repository.commondir())
        .ok()
        .and_then(|primary| {
            let source = primary
                .workdir()
                .unwrap_or(primary.path())
                .canonicalize()
                .ok()?;
            Some(source.file_name()?.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "Dépôt".into());
    let mut name = repository_head_name(repository);
    if repository.head_detached().unwrap_or(false) {
        if let Some(head) = repository.head().ok().and_then(|head| head.target()) {
            name = format!("{name} · {}", &head.to_string()[..7]);
        }
    }
    format!("{primary_name} · {name}")
}

fn repository_head_name(repository: &Repository) -> String {
    if repository.head_detached().unwrap_or(false) {
        return "HEAD détachée".into();
    }
    repository
        .head()
        .ok()
        .and_then(|head| head.shorthand().ok().map(str::to_owned))
        .or_else(|| {
            repository.find_reference("HEAD").ok().and_then(|head| {
                head.symbolic_target()
                    .ok()
                    .flatten()
                    .map(|name| name.trim_start_matches("refs/heads/").to_owned())
            })
        })
        .unwrap_or_else(|| "HEAD".into())
}

fn commit_diff<'a>(
    repository: &'a Repository,
    sha: &str,
) -> Result<(git2::Commit<'a>, git2::Diff<'a>)> {
    let oid = Oid::from_str(sha).map_err(|_| "Identifiant de commit invalide.")?;
    let commit = repository.find_commit(oid).map_err(|e| e.to_string())?;
    let tree = commit.tree().map_err(|e| e.to_string())?;
    let parent = if commit.parent_count() > 0 {
        Some(
            commit
                .parent(0)
                .and_then(|parent| parent.tree())
                .map_err(|e| e.to_string())?,
        )
    } else {
        None
    };
    let diff = tree_diff(repository, parent.as_ref(), &tree)?;
    Ok((commit, diff))
}

fn tree_diff<'repo>(
    repository: &'repo Repository,
    old: Option<&git2::Tree<'repo>>,
    new: &git2::Tree<'repo>,
) -> Result<git2::Diff<'repo>> {
    let mut options = DiffOptions::new();
    options.include_typechange(true).max_size(MAX_FILE_SIZE);
    let mut diff = repository
        .diff_tree_to_tree(old, Some(new), Some(&mut options))
        .map_err(|e| e.to_string())?;
    let mut find = DiffFindOptions::new();
    find.renames(true).rename_limit(200);
    diff.find_similar(Some(&mut find))
        .map_err(|e| e.to_string())?;
    Ok(diff)
}

fn index_repository(session: &Session) -> Result<()> {
    let repository = Repository::open(&session.source).map_err(|e| e.to_string())?;
    let mut walk = repository.revwalk().map_err(|e| e.to_string())?;
    walk.set_sorting(Sort::TOPOLOGICAL | Sort::TIME)
        .map_err(|e| e.to_string())?;
    for reference in &session.info.references {
        if !session.info.solo_refs.is_empty() && !session.info.solo_refs.contains(&reference.name) {
            continue;
        }
        walk.push(Oid::from_str(&reference.sha).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    }
    if let Some(head) = session
        .info
        .head
        .as_ref()
        .filter(|_| session.info.solo_refs.is_empty())
    {
        walk.push(Oid::from_str(head).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    }
    let mut layout = graph::Layout::default();
    let mut connection = cache::connect(&session.database, false)?;
    let mut position = 0;
    loop {
        if session.cancel.load(Ordering::Acquire) {
            return Ok(());
        }
        let batch: Vec<_> = walk.by_ref().take(BATCH_SIZE).collect();
        let exhausted = batch.len() < BATCH_SIZE;
        let mut values = Vec::with_capacity(batch.len());
        for oid in batch {
            if session.cancel.load(Ordering::Acquire) {
                return Ok(());
            }
            let oid = oid.map_err(|e| e.to_string())?;
            let sha = oid.to_string();
            let cached: bool = connection
                .prepare_cached("SELECT EXISTS(SELECT 1 FROM commits WHERE sha = ?1)")
                .map_err(|e| e.to_string())?
                .query_row([&sha], |row| row.get(0))
                .map_err(|e| e.to_string())?;
            let commit = repository.find_commit(oid).map_err(|e| e.to_string())?;
            let parents: Vec<_> = commit.parent_ids().map(|id| id.to_string()).collect();
            let metadata = if cached {
                None
            } else {
                let author = commit.author();
                Some(CommitMetadata {
                    summary: String::from_utf8_lossy(commit.summary_bytes().unwrap_or_default())
                        .into_owned(),
                    message: String::from_utf8_lossy(commit.message_bytes()).into_owned(),
                    author: String::from_utf8_lossy(author.name_bytes()).into_owned(),
                    email: String::from_utf8_lossy(author.email_bytes()).into_owned(),
                    time: commit.time().seconds(),
                })
            };
            let graph = layout.push(&sha, &parents);
            values.push(IndexedCommit {
                sha,
                metadata,
                parents: serde_json::to_string(&parents).map_err(|e| e.to_string())?,
                graph: serde_json::to_string(&graph).map_err(|e| e.to_string())?,
            });
        }
        {
            let _guard = session.write_lock.lock().unwrap();
            if session.cancel.load(Ordering::Acquire) {
                return Ok(());
            }
            let transaction = connection.transaction().map_err(|e| e.to_string())?;
            if !is_current(&transaction, session)? {
                session.cancel.store(true, Ordering::Release);
                return Ok(());
            }
            {
                let mut insert = transaction.prepare_cached(
                    "INSERT OR IGNORE INTO commits(sha, summary, message, author, email, time, parents)
                     VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                ).map_err(|e| e.to_string())?;
                let mut order = transaction
                    .prepare_cached("INSERT INTO walk(position, sha, graph) VALUES(?1, ?2, ?3)")
                    .map_err(|e| e.to_string())?;
                let mut parents = transaction
                    .prepare_cached("UPDATE commits SET parents = ?2 WHERE sha = ?1")
                    .map_err(|e| e.to_string())?;
                for value in values {
                    if let Some(metadata) = value.metadata {
                        insert
                            .execute(params![
                                value.sha,
                                metadata.summary,
                                metadata.message,
                                metadata.author,
                                metadata.email,
                                metadata.time,
                                value.parents
                            ])
                            .map_err(|e| e.to_string())?;
                    } else {
                        // Metadata text remains immutable, but effective parents
                        // change when a shallow boundary is added or removed.
                        parents
                            .execute(params![value.sha, value.parents])
                            .map_err(|e| e.to_string())?;
                    }
                    order
                        .execute(params![position as i64, value.sha, value.graph])
                        .map_err(|e| e.to_string())?;
                    position += 1;
                }
            }
            if exhausted {
                cache::set_meta(&transaction, "complete", "1")?;
            }
            transaction.commit().map_err(|e| e.to_string())?;
            session.indexed.store(position, Ordering::Release);
            if exhausted {
                // Closing SQLite may still checkpoint and remove WAL sidecars.
                // Publish completion only once those cache writes are finished.
                drop(connection);
                session.complete.store(true, Ordering::Release);
                return Ok(());
            }
        }
        thread::yield_now();
    }
}

fn render_file_diff(diff: &git2::Diff<'_>, file_index: usize) -> Result<FileDiff> {
    let delta = diff.get_delta(file_index).ok_or("Fichier introuvable.")?;
    if delta.old_file().size().max(delta.new_file().size()) > MAX_FILE_SIZE as u64 {
        return Ok(FileDiff {
            binary: false,
            truncated: true,
            additions: 0,
            deletions: 0,
            hunks: vec![],
        });
    }
    let Some(patch) = git2::Patch::from_diff(diff, file_index).map_err(|e| e.to_string())? else {
        return Ok(FileDiff {
            binary: true,
            truncated: false,
            additions: 0,
            deletions: 0,
            hunks: vec![],
        });
    };
    render_patch(patch)
}

fn render_patch(patch: git2::Patch<'_>) -> Result<FileDiff> {
    // libgit2 marks files above max_size as binary to skip processing. Expose
    // that size guard as a truncated diff instead of mislabelling text files.
    if patch
        .delta()
        .old_file()
        .size()
        .max(patch.delta().new_file().size())
        > MAX_FILE_SIZE as u64
    {
        return Ok(FileDiff {
            binary: false,
            truncated: true,
            additions: 0,
            deletions: 0,
            hunks: vec![],
        });
    }
    if patch.delta().flags().contains(git2::DiffFlags::BINARY) {
        return Ok(FileDiff {
            binary: true,
            truncated: false,
            additions: 0,
            deletions: 0,
            hunks: vec![],
        });
    }
    let (_, additions, deletions) = patch.line_stats().map_err(|e| e.to_string())?;
    let mut result = FileDiff {
        binary: false,
        truncated: false,
        additions,
        deletions,
        hunks: vec![],
    };
    let mut count = 0;
    let mut bytes = 0;
    'hunks: for index in 0..patch.num_hunks() {
        let (hunk, lines) = patch.hunk(index).map_err(|e| e.to_string())?;
        let mut output = DiffHunk {
            header: String::from_utf8_lossy(hunk.header()).trim_end().into(),
            lines: Vec::new(),
        };
        for line_index in 0..lines {
            let line = patch
                .line_in_hunk(index, line_index)
                .map_err(|e| e.to_string())?;
            if count >= MAX_DIFF_LINES || bytes + line.content().len() > 512 * 1024 {
                result.truncated = true;
                result.hunks.push(output);
                break 'hunks;
            }
            count += 1;
            bytes += line.content().len();
            let annotation = matches!(line.origin(), '=' | '>' | '<');
            let content = String::from_utf8_lossy(line.content());
            output.lines.push(DiffLine {
                kind: match line.origin() {
                    '+' => "addition",
                    '-' => "deletion",
                    '=' | '>' | '<' => "annotation",
                    _ => "context",
                }
                .into(),
                old_line: (!annotation).then(|| line.old_lineno()).flatten(),
                new_line: (!annotation).then(|| line.new_lineno()).flatten(),
                content: if annotation {
                    content.trim_matches(['\r', '\n'])
                } else {
                    content.trim_end_matches(['\r', '\n'])
                }
                .into(),
            });
        }
        result.hunks.push(output);
    }
    Ok(result)
}

fn changed_file(index: usize, delta: &git2::DiffDelta<'_>) -> ChangedFile {
    let path = delta
        .new_file()
        .path_bytes()
        .or_else(|| delta.old_file().path_bytes());
    ChangedFile {
        index,
        file_key: working_tree::file_key(path.unwrap_or_default()),
        path: path
            .map(|path| String::from_utf8_lossy(path).into_owned())
            .unwrap_or_else(|| "(sans nom)".into()),
        old_path: (delta.status() == git2::Delta::Renamed)
            .then(|| {
                delta
                    .old_file()
                    .path_bytes()
                    .map(|path| String::from_utf8_lossy(path).into_owned())
            })
            .flatten(),
        status: match delta.status() {
            git2::Delta::Added | git2::Delta::Untracked => "added",
            git2::Delta::Deleted => "deleted",
            git2::Delta::Modified => "modified",
            git2::Delta::Renamed => "renamed",
            git2::Delta::Copied => "copied",
            git2::Delta::Typechange => "typechange",
            git2::Delta::Unmodified => "unmodified",
            _ => "unknown",
        }
        .into(),
    }
}

#[cfg(test)]
mod tests {
    use super::{cache, search_cte};
    use rusqlite::{params, Connection};

    #[test]
    fn sha_prefix_search_uses_the_index_and_text_search_has_no_sha_scan() {
        let connection = Connection::open_in_memory().unwrap();
        cache::initialize(&connection).unwrap();
        let explain = |include_sha, sha: Option<&str>| {
            let sql = format!(
                "EXPLAIN QUERY PLAN {} SELECT id FROM matched",
                search_cte(include_sha)
            );
            connection
                .prepare(&sql)
                .unwrap()
                .query_map(params!["\"abc\"*", sha, "abc%"], |row| {
                    row.get::<_, String>(3)
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };
        let sha_plan = explain(true, Some("abc*"));
        assert!(
            sha_plan.iter().any(|step| step.contains(
                "SEARCH commits USING COVERING INDEX sqlite_autoindex_commits_1 (sha>? AND sha<?)"
            )),
            "{sha_plan:?}"
        );
        let text_plan = explain(false, None);
        assert!(
            text_plan
                .iter()
                .all(|step| !step.starts_with("SCAN commits")),
            "{text_plan:?}"
        );
    }
}
