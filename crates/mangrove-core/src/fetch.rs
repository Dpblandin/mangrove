use std::{
    io::Read,
    path::Path,
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use git2::{Reference, Repository};

#[cfg(unix)]
use std::os::unix::process::CommandExt;
#[cfg(windows)]
use std::os::windows::process::CommandExt;

use crate::{model::FetchResult, Result};

const FETCH_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_ERROR_BYTES: usize = 16 * 1024;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

pub(crate) fn fetch(source: &Path) -> Result<FetchResult> {
    let repository = Repository::open(source).map_err(|error| error.to_string())?;
    let remotes = repository.remotes().map_err(|error| error.to_string())?;
    let config = repository.config().ok();
    let mut errors = Vec::new();
    for name in remotes.iter() {
        let Some(name) = name.ok().flatten() else {
            errors.push("Le nom d’un remote n’est pas un texte UTF-8 valide.".into());
            continue;
        };
        if config
            .as_ref()
            .and_then(|config| config.get_bool(&format!("remote.{name}.skipFetchAll")).ok())
            == Some(true)
        {
            continue;
        }
        if !Reference::is_valid_name(&format!("refs/remotes/{name}/main")) {
            errors.push(format!("Nom de remote invalide : {name}"));
            continue;
        }
        if let Err(error) = fetch_remote(source, name) {
            errors.push(format!("{name} : {error}"));
        }
    }
    Ok(FetchResult {
        error: (!errors.is_empty()).then(|| errors.join("\n")),
    })
}

fn fetch_remote(source: &Path, name: &str) -> Result<()> {
    // Explicit refspecs also make mirror/custom remote configurations safe for
    // an automatic fetch: only remote-tracking branches and followed tags move.
    // Git reuses the user's credential helpers and SSH agent, without a shell.
    let hooks_path = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let mut command = Command::new("git");
    command
        .current_dir(source)
        .args(["-c", &format!("core.hooksPath={hooks_path}")])
        .args(["-c", "credential.interactive=false"])
        .args(["-c", "core.askPass="])
        .args(["-c", "protocol.ext.allow=never"])
        .args([
            "fetch",
            "--quiet",
            "--no-recurse-submodules",
            "--no-auto-maintenance",
            "--no-write-commit-graph",
            "--no-prune",
            "--no-prune-tags",
            "--refmap=",
            "--",
            name,
            &format!("+refs/heads/*:refs/remotes/{name}/*"),
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "")
        .env("SSH_ASKPASS_REQUIRE", "never")
        .env("GCM_INTERACTIVE", "never")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES");
    run_fetch(&mut command, FETCH_TIMEOUT)
}

fn run_fetch(command: &mut Command, timeout: Duration) -> Result<()> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    let mut child = command
        .spawn()
        .map_err(|error| format!("Impossible de lancer Git : {error}"))?;

    // Drain stderr while Git runs, retaining a bounded tail. No full pipe can
    // stall a fetch, and an unreachable server cannot occupy the worker forever.
    let (sender, receiver) = mpsc::sync_channel(1);
    let mut stderr = child.stderr.take().unwrap();
    thread::spawn(move || {
        let mut output = Vec::new();
        let mut buffer = [0; 4096];
        while let Ok(count) = stderr.read(&mut buffer) {
            if count == 0 {
                break;
            }
            output.extend_from_slice(&buffer[..count]);
            let excess = output.len().saturating_sub(MAX_ERROR_BYTES);
            output.drain(..excess);
        }
        let _ = sender.send(output);
    });

    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) => {
                // Git can exit before the reader drains its last diagnostic. A
                // detached helper retaining stderr must not block this handoff.
                let output = receiver
                    .recv_timeout(Duration::from_secs(1))
                    .unwrap_or_default();
                let message = String::from_utf8_lossy(&output).trim().to_owned();
                return Err(if message.is_empty() {
                    "Le fetch a échoué. Vérifiez l’accès au remote et vos identifiants Git.".into()
                } else {
                    message
                });
            }
            Ok(None) if started.elapsed() < timeout => {
                thread::sleep(Duration::from_millis(50));
            }
            result => {
                terminate_fetch(&mut child);
                return Err(match result {
                    Err(error) => format!("Impossible de suivre le fetch : {error}"),
                    _ => "Le fetch a dépassé le délai de deux minutes.".into(),
                });
            }
        }
    }
}

fn terminate_fetch(child: &mut Child) {
    // Each fetch has a private process group. Stop its SSH/credential helpers
    // too, before reaping Git and releasing the native fetch lock.
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn captures_the_last_diagnostic_after_draining_more_than_the_output_limit() {
        let error = run_fetch(
            Command::new("sh").args([
                "-c",
                r#"awk 'BEGIN { for (i=0; i<20000; i++) printf "x"; print "\nfetch diagnostic at end" }' >&2; exit 1"#,
            ]),
            FETCH_TIMEOUT,
        )
        .unwrap_err();

        assert!(error.ends_with("fetch diagnostic at end"));
        assert!(error.len() <= MAX_ERROR_BYTES);
    }

    #[test]
    fn timeout_terminates_the_fetch_and_its_helper_process() {
        let directory = tempfile::tempdir().unwrap();
        let pid_file = directory.path().join("fetch-pids");
        let started = Instant::now();
        let result = run_fetch(
            Command::new("sh")
                .env("MANGROVE_TEST_PID_FILE", &pid_file)
                .args([
                    "-c",
                    r#"sleep 30 & helper=$!; printf '%s\n%s\n' "$$" "$helper" > "$MANGROVE_TEST_PID_FILE"; wait "$helper""#,
                ]),
            Duration::from_secs(1),
        );
        let elapsed = started.elapsed();
        let pids: Vec<i32> = std::fs::read_to_string(pid_file)
            .unwrap()
            .lines()
            .map(|pid| pid.parse().unwrap())
            .collect();

        // Reclaim processes even if a regression makes an assertion fail.
        struct ProcessGroup(i32);
        impl Drop for ProcessGroup {
            fn drop(&mut self) {
                unsafe { libc::kill(-self.0, libc::SIGKILL) };
            }
        }
        let _cleanup = ProcessGroup(pids[0]);
        assert_eq!(pids.len(), 2);
        assert!(result.unwrap_err().contains("dépassé le délai"));
        assert!(elapsed < Duration::from_secs(10));

        // Orphaned helpers may briefly remain as zombies until init reaps them.
        let is_running = |pid: &i32| {
            let output = Command::new("ps")
                .args(["-p", &pid.to_string(), "-o", "stat="])
                .output()
                .unwrap();
            assert!(matches!(output.status.code(), Some(0 | 1)));
            let status = String::from_utf8(output.stdout).unwrap();
            let status = status.trim();
            !status.is_empty() && !status.starts_with(['Z', 'X'])
        };
        let deadline = Instant::now() + Duration::from_secs(2);
        while pids.iter().any(&is_running) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        for pid in pids {
            assert!(
                !is_running(&pid),
                "Fetch process {pid} survived its timeout"
            );
        }
    }
}
