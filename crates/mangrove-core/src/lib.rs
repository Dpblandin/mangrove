mod cache;
mod commit_diff;
mod fetch;
mod file_browser;
mod graph;
mod images;
pub mod model;
mod patch;
mod repository_context;
mod similarity;
#[cfg(test)]
mod test_support;
mod tracking;
mod working_tree;
mod worktree_filter;

use std::{
    collections::{HashMap, VecDeque},
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Condvar, Mutex, Weak,
    },
    thread,
    time::Instant,
};

use git2::{DiffOptions, Oid, Repository, Sort};
use model::*;
use rusqlite::{params, OptionalExtension};
use sha2::{Digest, Sha256};

pub type Result<T> = std::result::Result<T, String>;
const BATCH_SIZE: usize = 256;
/// Bounds images, rename candidates, blame and commit line totals.
const MAX_FILE_SIZE: u64 = PreviewLimit::MIN.bytes();
/// A backstop for libgit2's own size guard: each read first checks object
/// headers against its `PreviewLimit`, which is at most this size.
const MAX_PATCH_SIZE: i64 = PreviewLimit::MAX.bytes() as i64;
const MAX_CHANGED_FILES: usize = 5_000;
const MAX_OPEN_REPOSITORIES: usize = 8;
const MAX_REPOSITORY_NAMES: usize = 6;
const MAX_SEARCH_SNAPSHOT_BYTES: usize = 16 * 1024 * 1024;
const MAX_SEARCH_SNAPSHOTS: usize = 4;
/// Pairs accepted by one `Workspace::ahead_behind` call.
pub const MAX_AHEAD_BEHIND_PAIRS: usize = 10_000;

pub fn repository_names(paths: &[String]) -> Result<Vec<RepositoryName>> {
    if paths.len() > MAX_REPOSITORY_NAMES {
        return Err("La lecture des noms est limitée à six dépôts.".into());
    }
    Ok(paths
        .iter()
        .map(|path| {
            (|| -> Result<_> {
                let repository = open_source_repository(path)?;
                let source = repository
                    .workdir()
                    .unwrap_or(repository.path())
                    .canonicalize()
                    .map_err(|e| e.to_string())?;
                let primary_path = primary_repository_path(&repository);
                Ok(RepositoryName {
                    path: path.clone(),
                    name: Some(repository_display_name(
                        &repository,
                        &source,
                        primary_path.as_deref(),
                    )),
                    repository_path: Some(source.to_string_lossy().into_owned()),
                    primary_path: primary_path.map(|path| path.to_string_lossy().into_owned()),
                })
            })()
            .unwrap_or_else(|_| RepositoryName {
                path: path.clone(),
                name: None,
                repository_path: None,
                primary_path: None,
            })
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
    reads: cache::ReadPool,
    write_lock: Arc<Mutex<()>>,
    buffered: bool,
    progress: Mutex<IndexProgress>,
    searches: Mutex<SearchCache>,
    cancel: AtomicBool,
    error: Mutex<Option<String>>,
    commit_diffs: Mutex<commit_diff::CommitDiffCache>,
    working_tree: Mutex<working_tree::WorkingTreeCache>,
    counter: tracking::Counter,
    shallow: bool,
    counts_key: String,
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
        let mut progress = self.progress.lock().unwrap().clone();
        progress.error = self.error.lock().unwrap().clone();
        progress
    }
}

#[derive(Default)]
struct SearchCache {
    version: Option<(String, usize)>,
    snapshots: VecDeque<(String, Arc<Vec<usize>>)>,
    bytes: usize,
}

impl SearchCache {
    fn get(&mut self, query: &str, run: &str, indexed: usize) -> Option<Arc<Vec<usize>>> {
        if self
            .version
            .as_ref()
            .is_none_or(|version| version.0 != run || version.1 != indexed)
        {
            self.snapshots.clear();
            self.bytes = 0;
            self.version = Some((run.to_owned(), indexed));
        }
        let position = self.snapshots.iter().position(|(key, _)| key == query)?;
        let entry = self.snapshots.remove(position).unwrap();
        let snapshot = entry.1.clone();
        self.snapshots.push_back(entry);
        Some(snapshot)
    }

    fn insert(&mut self, query: String, positions: Vec<usize>) -> Arc<Vec<usize>> {
        let positions = Arc::new(positions);
        let bytes = positions.capacity() * std::mem::size_of::<usize>();
        while !self.snapshots.is_empty()
            && (self.snapshots.len() >= MAX_SEARCH_SNAPSHOTS
                || self.bytes.saturating_add(bytes) > MAX_SEARCH_SNAPSHOT_BYTES)
        {
            let (_, previous) = self.snapshots.pop_front().unwrap();
            self.bytes -= previous.capacity() * std::mem::size_of::<usize>();
        }
        self.bytes += bytes;
        self.snapshots.push_back((query, positions.clone()));
        positions
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
                    session.progress.lock().unwrap().complete = true;
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
    open_locks: Mutex<HashMap<String, Weak<Mutex<()>>>>,
    count_caches: Mutex<HashMap<String, Weak<Mutex<tracking::CountCache>>>>,
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
            open_locks: Mutex::new(HashMap::new()),
            count_caches: Mutex::new(HashMap::new()),
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
        // Serialize refreshes for this repository, while page/diff reads and
        // refreshes of other repositories can still acquire their sessions.
        let open_lock = shared(&self.open_locks, &key);
        let _open_guard = open_lock.lock().unwrap();
        let head = repository
            .head()
            .ok()
            .and_then(|head| head.peel_to_commit().ok());
        let head_sha = head.as_ref().map(|head| head.id().to_string());
        let head_name = repository_head_name(&repository);
        let mut references = Vec::new();
        for reference in repository.references().map_err(|e| e.to_string())? {
            let reference = reference.map_err(|e| e.to_string())?;
            // Only branches and tags are history roots. Internal refs stay out
            // of navigation, search and the traversal fingerprint.
            let kind = if reference.is_branch() {
                "local"
            } else if reference.is_remote() {
                "remote"
            } else if reference.is_tag() {
                "tag"
            } else {
                continue;
            };
            if reference.symbolic_target_bytes().is_some() {
                continue;
            }
            let Ok(commit) = reference.peel_to_commit() else {
                continue;
            };
            let raw_name = reference.name_bytes();
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
        let shallow = shallow_paths
            .iter()
            .map(|path| optional_file(path, "les frontières shallow"))
            .collect::<Result<Vec<_>>>()?;
        // Grafts change the parents libgit2 reads as well; it ignores
        // `refs/replace`. Linked worktrees share objects, so ahead/behind
        // counts, until shallow boundaries or grafts change them.
        let grafts = optional_file(&common_dir.join("info").join("grafts"), "les greffes")?;
        let mut counts_key = Sha256::new();
        counts_key.update(common_dir.as_os_str().as_encoded_bytes());
        for boundaries in shallow.iter().filter(|boundaries| !boundaries.is_empty()) {
            counts_key.update((boundaries.len() as u64).to_le_bytes());
            counts_key.update(boundaries);
        }
        counts_key.update(b"grafts");
        counts_key.update(&grafts);
        let counts_key = format!("{:x}", counts_key.finalize());
        let count_cache = shared(&self.count_caches, &counts_key);
        let fingerprint = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&(
                    9u32,
                    &head_sha,
                    &head_name,
                    &references,
                    &remotes,
                    &shallow,
                    &grafts,
                    &solo_refs
                ))
                .map_err(|e| e.to_string())?,
            )
        );
        let existing = self.sessions.lock().unwrap().get(&key).cloned();
        if let Some(session) = existing {
            if session.fingerprint == fingerprint
                && !session.cancel.load(Ordering::Acquire)
                && session.error.lock().unwrap().is_none()
            {
                let reusable = (|| {
                    let mut connection = session.reads.acquire()?;
                    let snapshot = connection.transaction().map_err(|e| e.to_string())?;
                    is_current(&snapshot, &session)
                })();
                if reusable == Ok(true) {
                    let mut info = session.info.clone();
                    info.index = session.progress();
                    return Ok(info);
                }
            }
            session.cancel.store(true, Ordering::Release);
        }
        let write_lock = shared(&self.write_locks, &key);
        let database = cache_dir.join(format!("{key}.sqlite"));
        let (indexed, complete, index_run, published_run, buffered) = {
            let _guard = write_lock.lock().unwrap();
            let mut connection = cache::connect(&database, false)?;
            cache::initialize(&connection)?;
            let complete = cache::meta(&connection, "fingerprint")?.as_deref()
                == Some(&fingerprint)
                && cache::meta(&connection, "complete")?.as_deref() == Some("1");
            let previous_run = cache::meta(&connection, "index_run")?.unwrap_or_default();
            let buffered = !complete && !previous_run.is_empty();
            let index_run = if complete {
                previous_run.clone()
            } else {
                new_index_run()
            };
            let published_run = if buffered {
                previous_run
            } else {
                index_run.clone()
            };
            if !complete {
                let transaction = connection.transaction().map_err(|e| e.to_string())?;
                transaction
                    .execute_batch("DELETE FROM walk_build; DELETE FROM refs_build;")
                    .map_err(|e| e.to_string())?;
                cache::set_meta(&transaction, "lease_fingerprint", &fingerprint)?;
                cache::set_meta(&transaction, "lease_run", &index_run)?;
                cache::set_meta(&transaction, "complete", "0")?;
                if !buffered {
                    cache::set_meta(&transaction, "fingerprint", &fingerprint)?;
                    cache::set_meta(&transaction, "index_run", &index_run)?;
                    cache::set_meta(&transaction, "indexed", "0")?;
                    transaction
                        .execute_batch("DELETE FROM walk; DELETE FROM refs;")
                        .map_err(|e| e.to_string())?;
                }
                let table = if buffered { "refs_build" } else { "refs" };
                let mut insert = transaction
                    .prepare_cached(&format!(
                        "INSERT INTO {table}(name, label, sha) VALUES(?1, ?2, ?3)"
                    ))
                    .map_err(|e| e.to_string())?;
                for reference in &references {
                    insert
                        .execute(params![reference.name, reference.label, reference.sha])
                        .map_err(|e| e.to_string())?;
                }
                drop(insert);
                transaction.commit().map_err(|e| e.to_string())?;
            }
            let indexed = published_count(&connection)?;
            (indexed, complete, index_run, published_run, buffered)
        };
        let info = RepositoryInfo {
            id: format!(
                "{}-{}",
                &key[..16],
                self.generation.fetch_add(1, Ordering::Relaxed)
            ),
            name: repository_display_name(
                &repository,
                &source,
                primary_repository_path(&repository).as_deref(),
            ),
            path: source.to_string_lossy().into_owned(),
            head: head_sha,
            head_name,
            references,
            solo_refs,
            remotes,
            index: IndexProgress {
                indexed,
                complete,
                index_run: published_run,
                error: None,
            },
        };
        let session = Arc::new(Session {
            info: info.clone(),
            fingerprint,
            index_run,
            source,
            reads: cache::ReadPool::new(database.clone()),
            database,
            write_lock,
            buffered,
            progress: Mutex::new(info.index.clone()),
            searches: Mutex::default(),
            cancel: AtomicBool::new(false),
            error: Mutex::new(None),
            commit_diffs: Mutex::default(),
            working_tree: Mutex::default(),
            shallow: shallow.iter().any(|boundaries| !boundaries.is_empty()),
            counts_key,
            counter: tracking::Counter::new(count_cache),
            opened: Instant::now(),
        });
        let mut sessions = self.sessions.lock().unwrap();
        let mut retired = None;
        if !sessions.contains_key(&key) && sessions.len() >= MAX_OPEN_REPOSITORIES {
            if let Some(oldest) = sessions
                .iter()
                .min_by_key(|(_, value)| value.opened)
                .map(|(key, _)| key.clone())
            {
                retired = sessions.remove(&oldest);
                if let Some(old) = &retired {
                    old.cancel.store(true, Ordering::Release);
                }
            }
        }
        let replaced = sessions.insert(key, session.clone());
        drop(sessions);
        // Pools and cached result vectors can be large. Retire them outside
        // the registry lock so unrelated session reads are never held up.
        drop(retired);
        drop(replaced);
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
        let mut connection = session.reads.acquire()?;
        let connection = connection.transaction().map_err(|e| e.to_string())?;
        ensure_current(&connection, &session)?;
        let limit = limit.clamp(1, 256);
        i64::try_from(offset).map_err(|_| "Position invalide.")?;
        let query = search_query(query);
        let index = published_progress(&connection, &session)?;
        let indexed = index.indexed;
        let (commits, total) = if query.is_empty() {
            let sql = format!(
                "SELECT {} FROM walk w JOIN commits c ON c.sha = w.sha
                 WHERE w.position >= ?1 AND w.position < ?3 ORDER BY w.position LIMIT ?2",
                cache::COLUMNS,
            );
            let mut statement = connection.prepare_cached(&sql).map_err(|e| e.to_string())?;
            let commits = statement
                .query_map(
                    params![offset as i64, limit as i64, indexed as i64],
                    cache::row_commit,
                )
                .map_err(|e| e.to_string())?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| e.to_string())?;
            (commits, indexed)
        } else {
            let mut page = Vec::with_capacity(limit);
            let results =
                search_results(&connection, &session, &query, &index, |result, position| {
                    if result >= offset && result - offset < limit {
                        page.push(position);
                    }
                })?;
            let (positions, total) = match results {
                SearchResults::Cached(snapshot) => (
                    snapshot.iter().skip(offset).take(limit).copied().collect(),
                    snapshot.len(),
                ),
                SearchResults::Scanned(total) => (page, total),
            };
            let positions = serde_json::to_string(&positions).map_err(|e| e.to_string())?;
            let sql = format!(
                "SELECT {} FROM json_each(?1) p
                 JOIN walk w ON w.position = p.value
                 JOIN commits c ON c.sha = w.sha ORDER BY w.position",
                cache::COLUMNS,
            );
            let mut statement = connection.prepare_cached(&sql).map_err(|e| e.to_string())?;
            let commits = statement
                .query_map([positions], cache::row_commit)
                .map_err(|e| e.to_string())?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| e.to_string())?;
            (commits, total)
        };
        Ok(CommitPage {
            commits,
            total,
            offset,
            index,
        })
    }

    /// Offset of a commit in the list read by `list_commits` for `query`,
    /// with the traversal run of the same snapshot. A commit outside the
    /// results has none.
    pub fn search_offset(
        &self,
        id: &str,
        query: &str,
        sha: &str,
    ) -> Result<(Option<usize>, IndexProgress)> {
        let session = self.session(id)?;
        let mut connection = session.reads.acquire()?;
        let connection = connection.transaction().map_err(|e| e.to_string())?;
        ensure_current(&connection, &session)?;
        let query = search_query(query);
        let index = published_progress(&connection, &session)?;
        let Some(target) =
            walk_position(&connection, sha)?.filter(|position| *position < index.indexed)
        else {
            return Ok((None, index));
        };
        if query.is_empty() {
            return Ok((Some(target), index));
        }
        let (mut before, mut found) = (0, false);
        let results = search_results(&connection, &session, &query, &index, |_, position| {
            before += usize::from(position < target);
            found |= position == target;
        })?;
        let offset = match results {
            SearchResults::Cached(snapshot) => snapshot.binary_search(&target).ok(),
            SearchResults::Scanned(_) => found.then_some(before),
        };
        Ok((offset, index))
    }

    pub fn commit_position(&self, id: &str, sha: &str) -> Result<Option<usize>> {
        self.commit_position_with_status(id, sha)
            .map(|(position, _)| position)
    }

    /// The position and traversal run are read from one snapshot. A buffered
    /// publication between two separate calls must never redirect a jump to
    /// the same position in a different traversal.
    pub fn commit_position_with_status(
        &self,
        id: &str,
        sha: &str,
    ) -> Result<(Option<usize>, IndexProgress)> {
        let session = self.session(id)?;
        let mut connection = session.reads.acquire()?;
        let connection = connection.transaction().map_err(|e| e.to_string())?;
        ensure_current(&connection, &session)?;
        let position = walk_position(&connection, sha)?;
        Ok((position, published_progress(&connection, &session)?))
    }

    pub fn commit_details(&self, id: &str, sha: &str) -> Result<CommitDetails> {
        self.commit_details_shared(id, sha)
            .map(|details| (*details).clone())
    }

    pub fn commit_details_shared(&self, id: &str, sha: &str) -> Result<Arc<CommitDetails>> {
        let session = self.session(id)?;
        let oid = Oid::from_str(sha).map_err(|_| "Identifiant de commit invalide.")?;
        if let Some(details) = session.commit_diffs.lock().unwrap().get_details(oid) {
            return Ok(details);
        }
        let repository = Repository::open(&session.source).map_err(|e| e.to_string())?;
        let (commit, diff, similarity_limited) = commit_diff(&repository, sha)?;
        session
            .commit_diffs
            .lock()
            .unwrap()
            .remember(commit.id(), &diff);
        let author = commit.author();
        let committer = commit.committer();
        let files = diff
            .deltas()
            .take(MAX_CHANGED_FILES)
            .enumerate()
            .map(|(index, delta)| changed_file(index, &delta))
            .collect();
        let details = Arc::new(CommitDetails {
            sha: commit.id().to_string(),
            message: String::from_utf8_lossy(commit.message_bytes()).into_owned(),
            author: String::from_utf8_lossy(author.name_bytes()).into_owned(),
            email: String::from_utf8_lossy(author.email_bytes()).into_owned(),
            author_time: author.when().seconds(),
            committer: String::from_utf8_lossy(committer.name_bytes()).into_owned(),
            committer_email: String::from_utf8_lossy(committer.email_bytes()).into_owned(),
            time: committer.when().seconds(),
            parents: commit.parent_ids().map(|id| id.to_string()).collect(),
            files,
            file_count: diff.deltas().len(),
            similarity_limited,
        });
        session
            .commit_diffs
            .lock()
            .unwrap()
            .remember_details(oid, details.clone());
        Ok(details)
    }

    pub fn file_diff(
        &self,
        id: &str,
        sha: &str,
        file_index: usize,
        limit: PreviewLimit,
    ) -> Result<FileDiff> {
        let session = self.session(id)?;
        let repository = Repository::open(&session.source).map_err(|e| e.to_string())?;
        let oid = Oid::from_str(sha).map_err(|_| "Identifiant de commit invalide.")?;
        let files = session.commit_diffs.lock().unwrap().get(oid);
        if let Some(files) = files {
            return files.file_diff(&repository, oid, file_index, limit);
        }
        let (commit, diff, _) = commit_diff(&repository, sha)?;
        session
            .commit_diffs
            .lock()
            .unwrap()
            .remember(commit.id(), &diff);
        if file_index >= diff.deltas().len() {
            return Err("Fichier introuvable dans ce commit.".into());
        }
        patch::render_bounded(&repository, &diff, file_index, limit)
    }

    /// Added and deleted lines of the whole commit, compared to its first
    /// parent like the file list. `None` when a blob is above 2 MiB or the
    /// commit has more than `max_files`; `cancelled` stops the read.
    pub fn commit_line_totals(
        &self,
        id: &str,
        sha: &str,
        max_files: usize,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Option<(usize, usize)>> {
        let session = self.session(id)?;
        let repository = Repository::open(&session.source).map_err(|e| e.to_string())?;
        let (_, diff, _) = commit_diff(&repository, sha)?;
        if diff.deltas().len() > max_files.min(MAX_CHANGED_FILES) {
            return Ok(None);
        }
        commit_diff::line_totals(&repository, &diff, cancelled)
    }

    /// Upstreams of the session's local branches, at the SHAs of its
    /// references. The configuration is read again on each call: `git branch
    /// -u` moves no ref, so it renews neither the session nor its fingerprint.
    pub fn branch_tracking(&self, id: &str) -> Result<TrackingList> {
        let session = self.session(id)?;
        let repository = Repository::open(&session.source).map_err(|e| e.to_string())?;
        tracking::branches(
            &repository,
            &session.info.references,
            session.shallow,
            &session.counts_key,
        )
    }

    /// Commits of each local SHA absent from its upstream SHA, and the reverse.
    /// Cached counts come first; the others are computed in order until
    /// `deadline`, at least one per call, or `cancelled`. `None` is a pair left
    /// for a later call, which should pass every pair still to count. Only a
    /// call stopped by its deadline keeps the repository open for the next
    /// one: an empty or cancelled call releases it. A refresh keeps the
    /// counts of unchanged pairs.
    pub fn ahead_behind(
        &self,
        id: &str,
        pairs: &[(String, String)],
        deadline: Instant,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<Option<AheadBehind>>> {
        if pairs.len() > MAX_AHEAD_BEHIND_PAIRS {
            return Err("Le comptage est limité à 10\u{202f}000 branches par lecture.".into());
        }
        let session = self.session(id)?;
        session
            .counter
            .ahead_behind(&session.source, pairs, deadline, &|| {
                cancelled() || session.cancel.load(Ordering::Acquire)
            })
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

    pub fn stash_diff(
        &self,
        id: &str,
        sha: &str,
        area: &str,
        file_key: &str,
        limit: PreviewLimit,
    ) -> Result<FileDiff> {
        let session = self.session(id)?;
        let repository = Repository::open(&session.source).map_err(|e| e.to_string())?;
        repository_context::stash_diff(&repository, sha, area, file_key, limit)
    }

    pub fn working_tree_status(&self, id: &str) -> Result<WorkingTreeStatus> {
        let session = self.session(id)?;
        let repository = Repository::open(&session.source).map_err(|e| e.to_string())?;
        let mut cache = session.working_tree.lock().unwrap();
        working_tree::status(&repository, &mut cache)
    }

    pub fn file_content(
        &self,
        id: &str,
        sha: &str,
        file_key: &str,
        limit: PreviewLimit,
    ) -> Result<FileContent> {
        let session = self.session(id)?;
        let repository = Repository::open(&session.source).map_err(|e| e.to_string())?;
        file_browser::content(&repository, sha, file_key, limit)
    }

    pub fn file_images(&self, id: &str, source: &FileImageSource) -> Result<FileImages> {
        let session = self.session(id)?;
        let repository = Repository::open(&session.source).map_err(|e| e.to_string())?;
        match source {
            FileImageSource::Commit {
                sha,
                file_key,
                comparison,
            } => file_browser::images(&repository, sha, file_key, *comparison),
            FileImageSource::WorkingTree { area, file_key } => working_tree::images(
                &repository,
                area,
                file_key,
                &mut session.working_tree.lock().unwrap(),
            ),
            FileImageSource::Stash {
                sha,
                area,
                file_key,
            } => repository_context::stash_images(&repository, sha, area, file_key),
        }
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

    pub fn file_revision_diff(
        &self,
        id: &str,
        sha: &str,
        file_key: &str,
        limit: PreviewLimit,
    ) -> Result<FileDiff> {
        let session = self.session(id)?;
        let repository = Repository::open(&session.source).map_err(|e| e.to_string())?;
        file_browser::revision_diff(&repository, sha, file_key, limit)
    }

    pub fn working_tree_diff(
        &self,
        id: &str,
        area: &str,
        file_key: &str,
        limit: PreviewLimit,
    ) -> Result<FileDiff> {
        let session = self.session(id)?;
        let repository = Repository::open(&session.source).map_err(|e| e.to_string())?;
        let mut cache = session.working_tree.lock().unwrap();
        working_tree::diff(&repository, area, file_key, limit, &mut cache)
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

/// The value of `key` while one of its users is alive.
fn shared<T: Default>(entries: &Mutex<HashMap<String, Weak<T>>>, key: &str) -> Arc<T> {
    let mut entries = entries.lock().unwrap();
    entries.retain(|_, entry| entry.strong_count() > 0);
    let entry = entries.get(key).and_then(Weak::upgrade).unwrap_or_default();
    entries.insert(key.to_owned(), Arc::downgrade(&entry));
    entry
}

/// The bytes of a file Git may omit, empty when it is absent.
fn optional_file(path: &Path, contents: &str) -> Result<Vec<u8>> {
    match fs::read(path) {
        Ok(bytes) => Ok(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(format!("Impossible de lire {contents} : {error}")),
    }
}

fn walk_position(connection: &rusqlite::Connection, sha: &str) -> Result<Option<usize>> {
    connection
        .query_row("SELECT position FROM walk WHERE sha = ?1", [sha], |row| {
            cache::row_usize(row, 0)
        })
        .optional()
        .map_err(|e| e.to_string())
}

fn search_query(query: &str) -> String {
    query.trim().chars().take(256).collect()
}

/// Walk positions of a search's results, in walk order.
enum SearchResults {
    /// The cached snapshot of every result.
    Cached(Arc<Vec<usize>>),
    /// The count of results that one scan gave to its visitor. Those that
    /// fit the snapshot budget are now cached.
    Scanned(usize),
}

/// Results of `query` in the published part of the walk. Each result of a
/// scan is given to `visit` with its offset.
fn search_results(
    connection: &rusqlite::Connection,
    session: &Session,
    query: &str,
    index: &IndexProgress,
    mut visit: impl FnMut(usize, usize),
) -> Result<SearchResults> {
    let mut cache = session.searches.lock().unwrap();
    if let Some(snapshot) = cache.get(query, &index.index_run, index.indexed) {
        return Ok(SearchResults::Cached(snapshot));
    }
    let fts = cache::search_terms(query);
    let sha = query
        .chars()
        .all(|character| character.is_ascii_hexdigit())
        .then(|| format!("{}*", query.to_ascii_lowercase()));
    let cte = search_cte(sha.is_some());
    // CROSS JOIN keeps selective matches ahead of the walk scan.
    // Only positions are materialized; graph JSON is read for the
    // requested page after the complete matching set is known.
    let sql = format!(
        "{cte} SELECT w.position FROM matched m
         CROSS JOIN commits c ON c.id = m.id
         CROSS JOIN walk w ON w.sha = c.sha
         WHERE w.position < ?4 ORDER BY w.position"
    );
    let mut statement = connection.prepare_cached(&sql).map_err(|e| e.to_string())?;
    let mut rows = statement
        .query(params![
            fts,
            sha,
            cache::like_prefix(query),
            index.indexed as i64
        ])
        .map_err(|e| e.to_string())?;
    let maximum = MAX_SEARCH_SNAPSHOT_BYTES / std::mem::size_of::<usize>();
    let mut snapshot = Vec::new();
    let mut total = 0usize;
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let position = cache::row_usize(row, 0).map_err(|e| e.to_string())?;
        if total < maximum {
            snapshot.push(position);
        }
        visit(total, position);
        total += 1;
    }
    // An exceptionally large match set stays bounded in memory. Its
    // visitor still saw every result in this one pass.
    if total <= maximum {
        cache.insert(query.to_owned(), snapshot);
    }
    Ok(SearchResults::Scanned(total))
}

fn published_count(connection: &rusqlite::Connection) -> Result<usize> {
    cache::meta(connection, "indexed")?
        .ok_or_else(|| "Le compteur d'historique est absent.".to_owned())?
        .parse()
        .map_err(|_| "Le compteur d'historique est invalide.".to_owned())
}

fn published_progress(
    connection: &rusqlite::Connection,
    session: &Session,
) -> Result<IndexProgress> {
    let error = session.error.lock().unwrap().clone();
    // All traversal fields belong to the caller's SQLite snapshot. In-memory
    // progress may already describe the run published after it began.
    Ok(IndexProgress {
        indexed: published_count(connection)?,
        complete: cache::meta(connection, "complete")?.as_deref() == Some("1") || error.is_some(),
        index_run: cache::meta(connection, "index_run")?.unwrap_or_default(),
        error,
    })
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
        || cache::meta(connection, "lease_fingerprint")?.as_deref() != Some(&session.fingerprint)
        || cache::meta(connection, "lease_run")?.as_deref() != Some(&session.index_run)))
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

fn primary_repository_path(repository: &Repository) -> Option<PathBuf> {
    if !repository.is_worktree() {
        return None;
    }
    let primary = Repository::open(repository.commondir()).ok()?;
    primary
        .workdir()
        .unwrap_or(primary.path())
        .canonicalize()
        .ok()
}

fn repository_display_name(
    repository: &Repository,
    source: &Path,
    primary_path: Option<&Path>,
) -> String {
    if !repository.is_worktree() {
        return source
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
    }
    let primary_name = primary_path
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy())
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
) -> Result<(git2::Commit<'a>, git2::Diff<'a>, Option<String>)> {
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
    let (diff, limit) = tree_diff_with_limit(repository, parent.as_ref(), &tree)?;
    Ok((commit, diff, limit))
}

fn tree_diff<'repo>(
    repository: &'repo Repository,
    old: Option<&git2::Tree<'repo>>,
    new: &git2::Tree<'repo>,
) -> Result<git2::Diff<'repo>> {
    tree_diff_with_limit(repository, old, new).map(|(diff, _)| diff)
}

fn tree_diff_with_limit<'repo>(
    repository: &'repo Repository,
    old: Option<&git2::Tree<'repo>>,
    new: &git2::Tree<'repo>,
) -> Result<(git2::Diff<'repo>, Option<String>)> {
    let mut options = DiffOptions::new();
    options
        .include_typechange(true)
        .max_size(MAX_PATCH_SIZE)
        .skip_binary_check(true);
    let mut diff = repository
        .diff_tree_to_tree(old, Some(new), Some(&mut options))
        .map_err(|e| e.to_string())?;
    let limited = similarity::find_renames(repository, &mut diff, false)?;
    Ok((diff, limited))
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
                let table = if session.buffered {
                    "walk_build"
                } else {
                    "walk"
                };
                let mut order = transaction
                    .prepare_cached(&format!(
                        "INSERT INTO {table}(position, sha, graph, parents) VALUES(?1, ?2, ?3, ?4)"
                    ))
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
                    }
                    // Effective shallow parents belong to the traversal, not
                    // immutable commit metadata shared with the previous run.
                    order
                        .execute(params![
                            position as i64,
                            value.sha,
                            value.graph,
                            value.parents
                        ])
                        .map_err(|e| e.to_string())?;
                    position += 1;
                }
            }
            if exhausted && session.buffered {
                // Schema changes are transactional in SQLite: readers see
                // either the old traversal and refs or the finished new pair.
                // Renaming avoids copying every graph row during publication.
                transaction
                    .execute_batch(
                        "DROP TABLE walk;
                     ALTER TABLE walk_build RENAME TO walk;
                     CREATE TABLE walk_build (
                         position INTEGER PRIMARY KEY, sha TEXT NOT NULL UNIQUE,
                         graph TEXT NOT NULL, parents TEXT NOT NULL
                     );
                     DROP TABLE refs;
                     ALTER TABLE refs_build RENAME TO refs;
                     CREATE TABLE refs_build (
                         name TEXT PRIMARY KEY, label TEXT NOT NULL, sha TEXT NOT NULL
                     );",
                    )
                    .map_err(|e| e.to_string())?;
                cache::set_meta(&transaction, "index_run", &session.index_run)?;
                cache::set_meta(&transaction, "fingerprint", &session.fingerprint)?;
            }
            if !session.buffered || exhausted {
                cache::set_meta(&transaction, "indexed", &position.to_string())?;
            }
            if exhausted {
                cache::set_meta(&transaction, "complete", "1")?;
            }
            transaction.commit().map_err(|e| e.to_string())?;
            if !session.buffered || exhausted {
                let mut progress = session.progress.lock().unwrap();
                progress.indexed = position;
                progress.index_run = session.index_run.clone();
            }
            if exhausted {
                // Closing SQLite may still checkpoint and remove WAL sidecars.
                // Publish completion only once those cache writes are finished.
                drop(connection);
                session.progress.lock().unwrap().complete = true;
                return Ok(());
            }
        }
        thread::yield_now();
    }
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
