use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reference {
    pub name: String,
    pub label: String,
    pub kind: String,
    pub sha: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryInfo {
    pub id: String,
    pub name: String,
    pub path: String,
    pub head: Option<String>,
    pub head_name: String,
    pub references: Vec<Reference>,
    pub solo_refs: Vec<String>,
    pub remotes: Vec<String>,
    pub index: IndexProgress,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryName {
    pub path: String,
    pub name: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexProgress {
    pub indexed: usize,
    pub complete: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphEdge {
    pub from: usize,
    pub to: usize,
    pub color: usize,
    pub kind: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphRow {
    pub lane: usize,
    pub color: usize,
    pub width: usize,
    pub edges: Vec<GraphEdge>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitRow {
    pub sha: String,
    pub summary: String,
    pub author: String,
    pub email: String,
    pub time: i64,
    pub parents: Vec<String>,
    pub position: usize,
    pub graph: GraphRow,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitPage {
    pub commits: Vec<CommitRow>,
    pub total: usize,
    pub offset: usize,
    pub index: IndexProgress,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangedFile {
    pub index: usize,
    pub file_key: String,
    pub path: String,
    pub old_path: Option<String>,
    pub status: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkingTreeFile {
    /// Lossless identity of the Git path; `file.path` is display text only.
    pub key: String,
    #[serde(flatten)]
    pub file: ChangedFile,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkingTreeStatus {
    pub available: bool,
    pub staged: Vec<WorkingTreeFile>,
    pub unstaged: Vec<WorkingTreeFile>,
    pub staged_count: usize,
    pub unstaged_count: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitDetails {
    pub sha: String,
    pub message: String,
    pub author: String,
    pub email: String,
    pub time: i64,
    pub parents: Vec<String>,
    pub files: Vec<ChangedFile>,
    pub file_count: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffLine {
    pub kind: String,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    pub content: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffHunk {
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDiff {
    pub binary: bool,
    pub truncated: bool,
    pub additions: usize,
    pub deletions: usize,
    pub hunks: Vec<DiffHunk>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileContent {
    pub sha: String,
    pub path: String,
    pub file_key: String,
    pub kind: String,
    pub size: usize,
    pub lines: Vec<String>,
    pub truncated: bool,
    pub ends_with_newline: bool,
    pub parent_sha: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileHistoryEntry {
    pub sha: String,
    pub path: String,
    pub file_key: String,
    pub old_path: Option<String>,
    pub status: String,
    pub summary: String,
    pub author: String,
    pub time: i64,
    pub merge: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileHistoryPage {
    pub entries: Vec<FileHistoryEntry>,
    pub next_cursor: Option<String>,
    pub scanned: usize,
    pub limitation: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlameGroup {
    pub start_line: usize,
    pub lines: usize,
    pub sha: String,
    pub author: String,
    pub time: i64,
    pub boundary: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileBlame {
    pub groups: Vec<BlameGroup>,
    pub limitation: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchResult {
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeInfo {
    pub name: String,
    pub path: String,
    pub primary: bool,
    pub current: bool,
    pub available: bool,
    pub head: Option<String>,
    pub head_name: String,
    pub detached: bool,
    pub locked: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeList {
    pub worktrees: Vec<WorktreeInfo>,
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StashInfo {
    pub sha: String,
    pub index: usize,
    pub message: String,
    pub author: String,
    pub time: i64,
    pub base_sha: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StashList {
    pub stashes: Vec<StashInfo>,
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StashGroup {
    pub area: String,
    pub comparison: String,
    pub old_sha: Option<String>,
    pub new_sha: String,
    pub files: Vec<ChangedFile>,
    pub file_count: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StashDetails {
    pub sha: String,
    pub message: String,
    pub author: String,
    pub time: i64,
    pub base_sha: String,
    pub groups: Vec<StashGroup>,
}
