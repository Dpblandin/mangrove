//! Cost of one commit file diff read at the highest preview limit. Engine only:
//! no UI, IPC or syntax worker. Content-free output; one process per case.
//! cargo run --release --locked -p mangrove-core --example large_diff_benchmark -- \
//!   fixture DIR MIB text|long|minified|repetitive edit|rewrite|add|churn
//! cargo run --release --locked -p mangrove-core --example large_diff_benchmark -- \
//!   case REPO [SHA PATH]
use mangrove_core::{model::PreviewLimit, Workspace};
use serde_json::json;
use std::{
    env, process, thread,
    time::{Duration, Instant},
};

const LINE: usize = 80;
const LONG_LINE: usize = 64 * 1024;
const EDITS: usize = 32;
const VOCABULARY: usize = 256;
const CHURN_EVERY: usize = 10;
const SAMPLES: usize = 10;
const TIMEOUT: Duration = Duration::from_secs(120);

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn line(&mut self, length: usize, alphabet: &[u8]) -> Vec<u8> {
        (0..length)
            .map(|_| alphabet[self.next() as usize % alphabet.len()])
            .collect()
    }
}

const WORDS: &[u8] = b"abcdefghijklmnopqrstuvwxyz      ";
const BASE64: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn content(size: usize, shape: &str, seed: u64) -> Vec<Vec<u8>> {
    let mut random = Lcg(seed);
    match shape {
        "text" => (0..size / LINE)
            .map(|_| random.line(LINE - 1, WORDS))
            .collect(),
        "long" => (0..size / LONG_LINE)
            .map(|_| random.line(LONG_LINE - 1, BASE64))
            .collect(),
        "minified" => vec![random.line(size, BASE64)],
        // Generated files repeat a small set of lines, so most lines match
        // several candidates: the expensive input for Myers' algorithm.
        "repetitive" => {
            let vocabulary: Vec<_> = (0..VOCABULARY)
                .map(|_| random.line(LINE - 1, WORDS))
                .collect();
            (0..size / LINE)
                .map(|_| vocabulary[random.next() as usize % VOCABULARY].clone())
                .collect()
        }
        _ => panic!("unknown shape {shape}"),
    }
}

fn join(lines: &[Vec<u8>], shape: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    for line in lines {
        bytes.extend_from_slice(line);
        if shape != "minified" {
            bytes.push(b'\n');
        }
    }
    bytes
}

fn fixture(dir: &str, mib: usize, shape: &str, change: &str) -> Result<(), String> {
    let size = mib * 1024 * 1024;
    let old = content(size, shape, 1);
    let new = match change {
        "rewrite" => content(size, shape, 2),
        "edit" if shape == "minified" => {
            let mut line = old[0].clone();
            let mut random = Lcg(3);
            for edit in 0..EDITS {
                let at = edit * line.len() / EDITS;
                line[at] = BASE64[random.next() as usize % BASE64.len()];
            }
            vec![line]
        }
        "churn" => {
            let mut lines = old.clone();
            let mut random = Lcg(4);
            for line in lines.iter_mut().step_by(CHURN_EVERY) {
                *line = random.line(line.len(), WORDS);
            }
            lines
        }
        "edit" | "add" => {
            let mut lines = old.clone();
            let mut random = Lcg(3);
            if change == "edit" {
                for edit in 0..EDITS {
                    let at = edit * lines.len() / EDITS;
                    let length = lines[at].len();
                    lines[at] = random.line(length, WORDS);
                }
            }
            lines
        }
        _ => return Err(format!("unknown change {change}")),
    };
    let repository = git2::Repository::init(dir).map_err(|e| e.to_string())?;
    let signature = git2::Signature::new("Bench", "bench@example.invalid", &git2::Time::new(0, 0))
        .map_err(|e| e.to_string())?;
    let readme = repository.blob(b"bench\n").map_err(|e| e.to_string())?;
    let tree = |file: Option<&[u8]>| -> Result<git2::Oid, String> {
        let mut builder = repository.treebuilder(None).map_err(|e| e.to_string())?;
        builder
            .insert("README", readme, 0o100644)
            .map_err(|e| e.to_string())?;
        if let Some(bytes) = file {
            let blob = repository.blob(bytes).map_err(|e| e.to_string())?;
            builder
                .insert("file.txt", blob, 0o100644)
                .map_err(|e| e.to_string())?;
        }
        builder.write().map_err(|e| e.to_string())
    };
    let old_bytes = join(&old, shape);
    let first = tree((change != "add").then_some(old_bytes.as_slice()))?;
    let second = tree(Some(&join(&new, shape)))?;
    let first = repository.find_tree(first).map_err(|e| e.to_string())?;
    let parent = repository
        .commit(Some("HEAD"), &signature, &signature, "old", &first, &[])
        .map_err(|e| e.to_string())?;
    let parent = repository.find_commit(parent).map_err(|e| e.to_string())?;
    let second = repository.find_tree(second).map_err(|e| e.to_string())?;
    repository
        .commit(
            Some("HEAD"),
            &signature,
            &signature,
            "new",
            &second,
            &[&parent],
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(not(unix))]
fn peak_rss_mib() -> f64 {
    f64::NAN
}

#[cfg(unix)]
fn peak_rss_mib() -> f64 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    let peak = unsafe { usage.assume_init() }.ru_maxrss as f64;
    // macOS reports bytes, Linux kibibytes.
    if cfg!(target_os = "macos") {
        peak / 1024. / 1024.
    } else {
        peak / 1024.
    }
}

fn case(repository: &str, sha: Option<&str>, path: Option<&str>) -> Result<(), String> {
    thread::spawn(|| {
        thread::sleep(TIMEOUT);
        println!("{}", json!({ "timeout_s": TIMEOUT.as_secs() }));
        process::exit(2);
    });
    let sha = git2::Repository::open(repository)
        .and_then(|r| {
            Ok(r.revparse_single(sha.unwrap_or("HEAD"))?
                .peel_to_commit()?
                .id())
        })
        .map_err(|e| e.to_string())?
        .to_string();
    let cache = tempfile::tempdir().map_err(|e| e.to_string())?;
    let workspace = Workspace::new(cache.path())?;
    let info = workspace.open_repository(repository)?;
    while !workspace.index_status(&info.id)?.complete {
        thread::sleep(Duration::from_millis(5));
    }
    let details = workspace.commit_details(&info.id, &sha)?;
    let file = details
        .files
        .iter()
        .find(|file| path.is_none_or(|path| file.path == path))
        .ok_or("file not in commit")?;
    let baseline = peak_rss_mib();
    let mut times = Vec::new();
    let mut diff = None;
    for _ in 0..SAMPLES {
        let started = Instant::now();
        diff = Some(workspace.file_diff(&info.id, &sha, file.index, PreviewLimit::MAX)?);
        times.push(started.elapsed().as_secs_f64() * 1000.);
    }
    let first = times[0];
    times.sort_by(f64::total_cmp);
    let diff = diff.unwrap();
    println!(
        "{}",
        json!({
            "first_ms": first,
            "p50_ms": times[SAMPLES / 2],
            "max_ms": times[SAMPLES - 1],
            "baseline_rss_mib": baseline,
            "peak_rss_mib": peak_rss_mib(),
            "truncated": diff.truncated,
            "binary": diff.binary,
            "additions": diff.additions,
            "deletions": diff.deletions,
            "shown_lines": diff.hunks.iter().map(|h| h.lines.len()).sum::<usize>(),
        })
    );
    Ok(())
}

fn main() -> Result<(), String> {
    let args: Vec<_> = env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["fixture", dir, mib, shape, change] => {
            fixture(dir, mib.parse().map_err(|_| "MIB")?, shape, change)
        }
        ["case", repository] => case(repository, None, None),
        ["case", repository, sha, path] => case(repository, Some(sha), Some(path)),
        _ => Err("Usage: fixture DIR MIB SHAPE CHANGE | case REPO [SHA PATH]".into()),
    }
}
