//! Explicit, read-only SSH discovery. No repository, object store or Git config is written.

use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

use crate::{CancellationToken, RemoteError, Result};

#[cfg(unix)]
use std::os::unix::{fs::FileTypeExt, process::CommandExt};
#[cfg(windows)]
use std::os::windows::process::CommandExt;

const TIMEOUT: Duration = Duration::from_secs(30);
const MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
const MAX_DIAGNOSTIC_BYTES: usize = 16 * 1024;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

/// SSH uses an existing local identity independently of a hosting provider's API token.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SshChoice {
    Disabled,
    #[default]
    Agent,
    KeyFile {
        path: PathBuf,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteReference {
    /// Full branch/tag reference, including `^{}` for an annotated tag's peeled target.
    pub name: String,
    /// SHA-1 or SHA-256 object identifier advertised by the server; no object is fetched.
    pub object_id: String,
}

/// List advertised heads and tags after an explicit user action.
pub fn read_references(
    remote: &str,
    choice: &SshChoice,
    cancel: &CancellationToken,
) -> Result<Vec<RemoteReference>> {
    cancel.check()?;
    validate_remote(remote)?;
    let ssh = ssh_command(choice)?;
    // A fresh directory prevents a caller's local config (URL rewrites, helpers,
    // includes, hooks) from changing this request. It never contains Git objects.
    let directory = tempfile::tempdir().map_err(|_| RemoteError::Storage)?;
    let mut command = Command::new("git");
    configure_git(&mut command, directory.path(), remote, &ssh);
    parse_references(&run_command(&mut command, TIMEOUT, cancel)?)
}

fn configure_git(command: &mut Command, directory: &Path, remote: &str, ssh: &str) {
    command.current_dir(directory).args([
        "-c",
        "core.hooksPath=",
        "-c",
        "credential.helper=",
        "-c",
        "credential.interactive=false",
        "-c",
        "core.askPass=",
        "-c",
        "protocol.allow=never",
        "-c",
        "protocol.ssh.allow=always",
        "ls-remote",
        "--heads",
        "--tags",
        "--",
        remote,
    ]);
    configure_isolated_git_environment(command);
    command
        .env("GIT_SSH_COMMAND", ssh)
        .env("GIT_SSH_VARIANT", "ssh");
}

/// Remove inherited config injection, repository/object paths, traces and SSH overrides.
pub(crate) fn remove_git_environment(command: &mut Command) {
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("GIT_") {
            command.env_remove(name);
        }
    }
}

/// Isolate explicit Git operations before callers add validated credentials or SSH choices.
pub(crate) fn configure_isolated_git_environment(command: &mut Command) {
    remove_git_environment(command);
    let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_SYSTEM", null)
        .env("GIT_CONFIG_GLOBAL", null)
        .env("GIT_CONFIG_COUNT", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "")
        .env("SSH_ASKPASS_REQUIRE", "never")
        .env_remove("SSH_ASKPASS")
        .env("GCM_INTERACTIVE", "never");
}

pub(crate) fn ssh_command(choice: &SshChoice) -> Result<String> {
    let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
    // Git invokes GIT_SSH_COMMAND through its shell. Only the identity path is
    // variable, and it is passed as a single POSIX-shell-quoted argument.
    let mut command = format!(
        "ssh -F {null} -o BatchMode=yes -o StrictHostKeyChecking=yes \
         -o UpdateHostKeys=no -o PasswordAuthentication=no \
         -o KbdInteractiveAuthentication=no -o NumberOfPasswordPrompts=0 \
         -o PermitLocalCommand=no -o ControlMaster=no -o ControlPersist=no \
         -o ControlPath=none -o ProxyCommand=none -o ProxyJump=none \
         -o ConnectTimeout=10 -o ConnectionAttempts=1 -o LogLevel=ERROR"
    );
    match choice {
        SshChoice::Disabled => return Err(RemoteError::SshUnavailable),
        SshChoice::Agent => {
            // Unix agents advertise a socket. Windows OpenSSH discovers its
            // native agent service through a named pipe without SSH_AUTH_SOCK.
            #[cfg(unix)]
            validate_agent_socket(std::env::var_os("SSH_AUTH_SOCK").as_deref())?;
            command.push_str(" -o IdentityFile=none -o IdentitiesOnly=no");
        }
        SshChoice::KeyFile { path } => {
            let text = path.to_str().ok_or(RemoteError::InvalidConfiguration)?;
            if !path.is_absolute() || text.chars().any(char::is_control) || !path.is_file() {
                return Err(RemoteError::InvalidConfiguration);
            }
            command.push_str(" -o IdentityAgent=none -o IdentitiesOnly=yes -i ");
            command.push_str(&shell_quote(text));
        }
    }
    Ok(command)
}

#[cfg(unix)]
fn validate_agent_socket(socket: Option<&std::ffi::OsStr>) -> Result<()> {
    if socket
        .filter(|value| !value.is_empty())
        .is_some_and(|socket| {
            std::fs::metadata(socket)
                .map(|metadata| metadata.file_type().is_socket())
                .unwrap_or(false)
        })
    {
        Ok(())
    } else {
        Err(RemoteError::SshUnavailable)
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Reject options, local transports, URL credentials, and shell metacharacters.
/// Only literal SSH URLs and SCP-style repository paths are accepted.
pub(crate) fn validate_remote(remote: &str) -> Result<()> {
    if remote.is_empty()
        || remote.len() > 4096
        || !remote.is_ascii()
        || (remote.contains("://") && !remote.starts_with("ssh://"))
        || remote
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
    {
        return Err(RemoteError::InvalidConfiguration);
    }
    let (authority, path, allow_port) = if let Some(value) = remote.strip_prefix("ssh://") {
        let split = value.find('/').ok_or(RemoteError::InvalidConfiguration)?;
        (&value[..split], &value[split..], true)
    } else {
        let split = if let Some(end) = remote.find(']') {
            if remote.as_bytes().get(end + 1) != Some(&b':') {
                return Err(RemoteError::InvalidConfiguration);
            }
            end + 1
        } else {
            remote.find(':').ok_or(RemoteError::InvalidConfiguration)?
        };
        (&remote[..split], &remote[split + 1..], false)
    };
    if path.is_empty()
        || path == "/"
        || path.starts_with('-')
        || !path
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-._/~".contains(&byte))
    {
        return Err(RemoteError::InvalidConfiguration);
    }
    let host = if let Some((user, host)) = authority.split_once('@') {
        if !valid_login(user) {
            return Err(RemoteError::InvalidConfiguration);
        }
        host
    } else {
        authority
    };
    let (host, port) = if host.starts_with('[') {
        let end = host.find(']').ok_or(RemoteError::InvalidConfiguration)?;
        let address = &host[1..end];
        if address.parse::<std::net::Ipv6Addr>().is_err() {
            return Err(RemoteError::InvalidConfiguration);
        }
        let suffix = &host[end + 1..];
        let port = if suffix.is_empty() {
            None
        } else {
            Some(
                suffix
                    .strip_prefix(':')
                    .ok_or(RemoteError::InvalidConfiguration)?,
            )
        };
        (&host[..end + 1], port)
    } else if allow_port {
        host.split_once(':')
            .map(|(host, port)| (host, Some(port)))
            .unwrap_or((host, None))
    } else {
        (host, None)
    };
    if !host.starts_with('[')
        && (host.is_empty()
            || host.starts_with(['-', '.'])
            || !host
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-.".contains(&byte)))
    {
        return Err(RemoteError::InvalidConfiguration);
    }
    if let Some(port) = port {
        if !allow_port
            || !port.bytes().all(|byte| byte.is_ascii_digit())
            || !matches!(port.parse::<u16>(), Ok(1..=u16::MAX))
        {
            return Err(RemoteError::InvalidConfiguration);
        }
    }
    Ok(())
}

fn valid_login(login: &str) -> bool {
    !login.is_empty()
        && !login.starts_with('-')
        && login
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-._".contains(&byte))
}

fn parse_references(output: &[u8]) -> Result<Vec<RemoteReference>> {
    let text = std::str::from_utf8(output).map_err(|_| RemoteError::InvalidResponse)?;
    text.lines()
        .map(|line| {
            let (object_id, name) = line.split_once('\t').ok_or(RemoteError::InvalidResponse)?;
            if !matches!(object_id.len(), 40 | 64)
                || !object_id.bytes().all(|byte| byte.is_ascii_hexdigit())
                || !valid_reference(name)
            {
                return Err(RemoteError::InvalidResponse);
            }
            Ok(RemoteReference {
                name: name.to_owned(),
                object_id: object_id.to_owned(),
            })
        })
        .collect()
}

fn valid_reference(name: &str) -> bool {
    let name = if name.starts_with("refs/tags/") {
        name.strip_suffix("^{}").unwrap_or(name)
    } else {
        name
    };
    let suffix = name
        .strip_prefix("refs/heads/")
        .or_else(|| name.strip_prefix("refs/tags/"));
    let Some(suffix) = suffix else { return false };
    !suffix.is_empty()
        && !name.contains("..")
        && !name.contains("@{")
        && !name
            .bytes()
            .any(|byte| byte <= b' ' || byte == 0x7f || b"~^:?*[\\".contains(&byte))
        && name.split('/').all(|part| {
            !part.is_empty()
                && !part.starts_with('.')
                && !part.ends_with('.')
                && !part.ends_with(".lock")
        })
}

struct CapturedOutput {
    bytes: Vec<u8>,
    failed: bool,
}

fn drain(
    mut input: impl Read + Send + 'static,
    limit: usize,
    overflow: Option<Arc<AtomicBool>>,
) -> mpsc::Receiver<CapturedOutput> {
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut buffer = [0; 8192];
        let mut failed = false;
        loop {
            match input.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    let available = limit.saturating_sub(bytes.len());
                    bytes.extend_from_slice(&buffer[..count.min(available)]);
                    if count > available {
                        if let Some(overflow) = &overflow {
                            overflow.store(true, Ordering::Release);
                        }
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    failed = true;
                    break;
                }
            }
        }
        let _ = sender.send(CapturedOutput { bytes, failed });
    });
    receiver
}

pub(crate) fn run_command(
    command: &mut Command,
    timeout: Duration,
    cancel: &CancellationToken,
) -> Result<Vec<u8>> {
    cancel.check()?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    let child = command.spawn().map_err(|_| RemoteError::SshUnavailable)?;
    let mut process = Process(child);
    let overflow = Arc::new(AtomicBool::new(false));
    let stdout = drain(
        process.0.stdout.take().unwrap(),
        MAX_OUTPUT_BYTES,
        Some(overflow.clone()),
    );
    let stderr = drain(process.0.stderr.take().unwrap(), MAX_DIAGNOSTIC_BYTES, None);
    let mut output = None;
    let mut diagnostic = None;
    let mut status: Option<ExitStatus> = None;
    let started = Instant::now();
    loop {
        cancel.check()?;
        if overflow.load(Ordering::Acquire) {
            return Err(RemoteError::ResponseTooLarge);
        }
        if started.elapsed() >= timeout {
            return Err(RemoteError::Timeout);
        }
        if status.is_none() {
            status = process.0.try_wait().map_err(|_| RemoteError::Network)?;
        }
        if output.is_none() {
            output = stdout.try_recv().ok();
        }
        if diagnostic.is_none() {
            diagnostic = stderr.try_recv().ok();
        }
        if let (Some(status), Some(captured), Some(diagnostic)) = (&status, &output, &diagnostic) {
            if overflow.load(Ordering::Acquire) {
                return Err(RemoteError::ResponseTooLarge);
            }
            if captured.failed || diagnostic.failed {
                return Err(RemoteError::Network);
            }
            if status.success() {
                return Ok(output.take().unwrap().bytes);
            }
            // Diagnostics are used only to select a fixed message. Never return
            // server output, a private key path, URL, or an inherited credential.
            let text = String::from_utf8_lossy(&diagnostic.bytes).to_ascii_lowercase();
            return Err(
                if text.contains("host key verification failed")
                    || text.contains("remote host identification has changed")
                {
                    RemoteError::SshHostKey
                } else if text.contains("permission denied")
                    || text.contains("authentication failed")
                {
                    RemoteError::SshAuthentication
                } else {
                    RemoteError::Network
                },
            );
        }
        thread::sleep(Duration::from_millis(10));
    }
}

struct Process(Child);

impl Drop for Process {
    fn drop(&mut self) {
        // Terminate the private group as well as Git itself: cancellation also
        // stops SSH and a descendant retaining one of the output pipes.
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.0.id() as i32), libc::SIGKILL);
        }
        #[cfg(windows)]
        {
            let _ = Command::new("taskkill")
                .args(["/PID", &self.0.id().to_string(), "/T", "/F"])
                .creation_flags(CREATE_NO_WINDOW)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::BTreeMap, ffi::OsStr, fs};

    #[test]
    fn accepts_literal_ssh_endpoints_and_rejects_other_transports_and_options() {
        for remote in [
            "git@example.com:team/repository.git",
            "example.com:/srv/git/repository.git",
            "git@example.com:~/repository.git",
            "ssh://git@example.com:2222/team/repository.git",
            "ssh://example.com/team/repository.git",
            "git@[::1]:repository.git",
            "ssh://git@[2001:db8::1]:22/repository.git",
        ] {
            assert!(validate_remote(remote).is_ok(), "{remote}");
        }
        for remote in [
            "",
            "origin",
            "../repository",
            "/tmp/repository",
            "file:///tmp/repository",
            "https://example.com/repository",
            "ext::touch /tmp/x",
            "-oProxyCommand=touch:repo",
            "git@-example.com:repo",
            "ssh://-example.com/repo",
            "git@host:-option",
            "git@host:repo;touch",
            "git@host:repo$(touch)",
            "git@host:repo`touch`",
            "git@host:repo\n",
            "ssh://user:secret@host/repo",
            "ssh://host/repo?secret=token",
            "ssh://host/repo#fragment",
            "ssh://host/%2e%2e/repo",
            "ssh://host:0/repo",
            "ssh://host:65536/repo",
            "ssh://host:abc/repo",
            "ssh://host",
            "ssh://host/",
            "git@[not-ipv6]:repo",
            "git@[::1]:22:repo",
            "git@host:repo name",
        ] {
            assert!(validate_remote(remote).is_err(), "{remote}");
        }
    }

    #[test]
    fn parses_branches_lightweight_and_peeled_tags_but_rejects_malformed_data() {
        let sha1 = "a".repeat(40);
        let sha256 = "b".repeat(64);
        let refs = parse_references(
            format!("{sha1}\trefs/heads/main\n{sha256}\trefs/tags/v1\n{sha1}\trefs/tags/v1^{{}}\n")
                .as_bytes(),
        )
        .unwrap();
        assert_eq!(refs.len(), 3);
        assert_eq!(refs[0].name, "refs/heads/main");
        assert_eq!(refs[1].object_id, sha256);
        assert_eq!(refs[2].name, "refs/tags/v1^{}");
        assert!(parse_references(b"").unwrap().is_empty());
        for name in [
            "HEAD",
            "refs/heads/",
            "refs/heads/a..b",
            "refs/tags/a.lock",
            "refs/heads/a^{}",
            "refs/heads/a//b",
            "refs/tags/a\tsecret",
        ] {
            assert!(parse_references(format!("{sha1}\t{name}\n").as_bytes()).is_err());
        }
        assert!(parse_references(b"not-a-hash\trefs/heads/main\n").is_err());
        assert!(parse_references(b"\xff").is_err());
    }

    #[test]
    fn git_command_uses_only_advertised_refs_and_isolated_configuration() {
        let directory = tempfile::tempdir().unwrap();
        let mut command = Command::new("git");
        configure_git(
            &mut command,
            directory.path(),
            "git@host:team/repo",
            "ssh options",
        );
        let arguments: Vec<_> = command
            .get_args()
            .map(|value| value.to_string_lossy())
            .collect();
        assert_eq!(
            &arguments[arguments.len() - 5..],
            &["ls-remote", "--heads", "--tags", "--", "git@host:team/repo"]
        );
        assert_eq!(command.get_current_dir(), Some(directory.path()));
        let environment: BTreeMap<_, _> = command.get_envs().collect();
        assert_eq!(
            environment.get(OsStr::new("GIT_CONFIG_NOSYSTEM")),
            Some(&Some(OsStr::new("1")))
        );
        assert_eq!(
            environment.get(OsStr::new("GIT_CONFIG_COUNT")),
            Some(&Some(OsStr::new("0")))
        );
        assert_eq!(
            environment.get(OsStr::new("GIT_SSH_COMMAND")),
            Some(&Some(OsStr::new("ssh options")))
        );
        assert!(fs::read_dir(directory.path()).unwrap().next().is_none());
    }

    #[test]
    fn selected_key_must_be_an_existing_absolute_file_and_uses_batch_mode() {
        assert!(ssh_command(&SshChoice::Disabled).is_err());
        assert!(ssh_command(&SshChoice::KeyFile {
            path: PathBuf::from("relative-key")
        })
        .is_err());
        let directory = tempfile::tempdir().unwrap();
        assert!(ssh_command(&SshChoice::KeyFile {
            path: directory.path().to_owned()
        })
        .is_err());
        let key = directory.path().join("key's $(touch unwanted)");
        fs::write(&key, "fixture identity content").unwrap();
        let command = ssh_command(&SshChoice::KeyFile { path: key.clone() }).unwrap();
        assert!(command.contains("BatchMode=yes"));
        assert!(command.contains("StrictHostKeyChecking=yes"));
        assert!(command.contains("UpdateHostKeys=no"));
        assert!(command.contains("IdentityAgent=none"));
        assert!(command.ends_with(&shell_quote(key.to_str().unwrap())));
        assert_eq!(fs::read(&key).unwrap(), b"fixture identity content");
    }

    #[cfg(unix)]
    #[test]
    fn unix_agent_requires_an_existing_socket_without_touching_the_real_agent() {
        use std::os::unix::net::UnixListener;

        let directory = tempfile::tempdir().unwrap();
        let socket_path = directory.path().join("fixture-agent.sock");
        let _listener = UnixListener::bind(&socket_path).unwrap();
        assert!(validate_agent_socket(Some(socket_path.as_os_str())).is_ok());
        assert_eq!(
            validate_agent_socket(None),
            Err(RemoteError::SshUnavailable)
        );
        assert_eq!(
            validate_agent_socket(Some(OsStr::new(""))),
            Err(RemoteError::SshUnavailable)
        );
        let ordinary_file = directory.path().join("ordinary-file");
        fs::write(&ordinary_file, "fixture").unwrap();
        assert_eq!(
            validate_agent_socket(Some(ordinary_file.as_os_str())),
            Err(RemoteError::SshUnavailable)
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_agent_defers_to_the_native_openssh_service() {
        let command = ssh_command(&SshChoice::Agent).unwrap();
        assert!(command.contains("IdentityFile=none"));
        assert!(command.contains("IdentitiesOnly=no"));
        assert!(command.contains("BatchMode=yes"));
        assert!(!command.contains("IdentityAgent=none"));
    }

    #[cfg(unix)]
    #[test]
    fn identity_path_quoting_preserves_shell_metacharacters_as_one_argument() {
        let path = "/tmp/key's $(printf secret);`printf secret`";
        let output = Command::new("sh")
            .args([
                "-c",
                &format!("set -- {}; printf '%s' \"$1\"", shell_quote(path)),
            ])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, path.as_bytes());
    }

    #[cfg(unix)]
    #[test]
    fn drains_both_pipes_and_never_exposes_server_diagnostics() {
        let output = run_command(
            Command::new("sh").args(["-c", "head -c 100000 /dev/zero >&2; printf result"]),
            Duration::from_secs(5),
            &CancellationToken::new(),
        )
        .unwrap();
        assert_eq!(output, b"result");
        let error = run_command(
            Command::new("sh").args(["-c", "printf 'Permission denied (secret key /private/identity and token=secret)' >&2; exit 1"]),
            Duration::from_secs(5), &CancellationToken::new(),
        ).unwrap_err();
        assert!(matches!(error, RemoteError::SshAuthentication));
        assert!(!error.to_string().contains("secret"));
        assert!(!error.to_string().contains("/private"));
    }

    #[cfg(unix)]
    #[test]
    fn bounds_output_without_a_full_pipe_deadlocking_git() {
        let error = run_command(
            Command::new("sh").args(["-c", "head -c 5000000 /dev/zero"]),
            Duration::from_secs(5),
            &CancellationToken::new(),
        )
        .unwrap_err();
        assert!(matches!(error, RemoteError::ResponseTooLarge));
    }

    #[cfg(unix)]
    fn assert_processes_stop(cancel: bool) {
        let directory = tempfile::tempdir().unwrap();
        let pid_file = directory.path().join("pids");
        let cancellation = CancellationToken::new();
        let cancel_thread = cancel.then(|| {
            let cancellation = cancellation.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(150));
                cancellation.cancel();
            })
        });
        let started = Instant::now();
        let result = run_command(
            Command::new("sh").env("MANGROVE_TEST_PID_FILE", &pid_file).args([
                "-c", "sleep 30 & helper=$!; printf '%s\\n%s\\n' \"$$\" \"$helper\" > \"$MANGROVE_TEST_PID_FILE\"; wait \"$helper\"",
            ]),
            Duration::from_millis(500), &cancellation,
        );
        if let Some(cancel_thread) = cancel_thread {
            cancel_thread.join().unwrap();
        }
        assert!(started.elapsed() < Duration::from_secs(5));
        if cancel {
            assert!(matches!(result, Err(RemoteError::Cancelled)));
        } else {
            assert!(matches!(result, Err(RemoteError::Timeout)));
        }
        let pids: Vec<_> = fs::read_to_string(pid_file)
            .unwrap()
            .lines()
            .map(|pid| pid.to_owned())
            .collect();
        assert_eq!(pids.len(), 2);
        let is_running = |pid: &str| {
            let output = Command::new("ps")
                .args(["-p", pid, "-o", "stat="])
                .output()
                .unwrap();
            let state = String::from_utf8(output.stdout).unwrap();
            !state.trim().is_empty() && !state.trim().starts_with(['Z', 'X'])
        };
        let deadline = Instant::now() + Duration::from_secs(2);
        while pids.iter().any(|pid| is_running(pid)) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        for pid in pids {
            assert!(!is_running(&pid), "SSH process {pid} survived");
        }
    }

    #[cfg(unix)]
    #[test]
    fn timeout_terminates_git_and_its_descendant() {
        assert_processes_stop(false);
    }

    #[cfg(unix)]
    #[test]
    fn cancellation_terminates_git_and_its_descendant() {
        assert_processes_stop(true);
    }

    fn snapshot(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
        fn visit(root: &Path, path: &Path, contents: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
            for entry in fs::read_dir(path).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                if entry.file_type().unwrap().is_dir() {
                    contents.insert(path.strip_prefix(root).unwrap().to_owned(), None);
                    visit(root, &path, contents);
                } else {
                    contents.insert(
                        path.strip_prefix(root).unwrap().to_owned(),
                        Some(fs::read(path).unwrap()),
                    );
                }
            }
        }
        let mut contents = BTreeMap::new();
        visit(root, root, &mut contents);
        contents
    }

    fn make_bare(directory: &Path) -> (PathBuf, String) {
        let bare = directory.join("source.git");
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .current_dir(directory)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env(
                    "GIT_CONFIG_GLOBAL",
                    if cfg!(windows) { "NUL" } else { "/dev/null" },
                )
                .env("GIT_AUTHOR_NAME", "Fixture")
                .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
                .env("GIT_COMMITTER_NAME", "Fixture")
                .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
                .args(args)
                .stdin(Stdio::null())
                .output()
                .unwrap();
            assert!(output.status.success());
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        };
        git(&["init", "--bare", bare.to_str().unwrap()]);
        let tree = git(&["--git-dir", bare.to_str().unwrap(), "mktree"]);
        let commit = git(&[
            "--git-dir",
            bare.to_str().unwrap(),
            "commit-tree",
            &tree,
            "-m",
            "fixture",
        ]);
        git(&[
            "--git-dir",
            bare.to_str().unwrap(),
            "update-ref",
            "refs/heads/main",
            &commit,
        ]);
        git(&[
            "--git-dir",
            bare.to_str().unwrap(),
            "update-ref",
            "refs/tags/v1",
            &commit,
        ]);
        (bare, commit)
    }

    #[test]
    fn real_git_advertisement_reads_a_synthetic_bare_repository_without_writes() {
        let directory = tempfile::tempdir().unwrap();
        let (bare, commit) = make_bare(directory.path());
        let before = snapshot(&bare);
        let output = run_command(
            Command::new("git").current_dir(directory.path()).args([
                "ls-remote",
                "--heads",
                "--tags",
                "--",
                bare.to_str().unwrap(),
            ]),
            Duration::from_secs(5),
            &CancellationToken::new(),
        )
        .unwrap();
        let references = parse_references(&output).unwrap();
        assert_eq!(references.len(), 2);
        assert!(references
            .iter()
            .all(|reference| reference.object_id == commit));
        assert_eq!(snapshot(&bare), before);
    }

    #[cfg(unix)]
    #[test]
    fn localhost_ssh_fixture_preserves_identity_argument_and_leaves_bare_source_unchanged() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let (bare, commit) = make_bare(directory.path());
        let binaries = directory.path().join("bin");
        fs::create_dir(&binaries).unwrap();
        let fake_ssh = binaries.join("ssh");
        fs::write(&fake_ssh,
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$MANGROVE_TEST_SSH_ARGUMENTS\"\nexec git upload-pack \"$MANGROVE_TEST_REMOTE\"\n",
        ).unwrap();
        fs::set_permissions(&fake_ssh, fs::Permissions::from_mode(0o700)).unwrap();
        let key = directory.path().join("key's $(touch unwanted)");
        fs::write(&key, "fixture identity only").unwrap();
        let arguments_file = directory.path().join("ssh-arguments");
        let work = directory.path().join("empty-work");
        fs::create_dir(&work).unwrap();
        let remote = "git@localhost:fixture.git";
        validate_remote(remote).unwrap();
        let ssh = ssh_command(&SshChoice::KeyFile { path: key.clone() }).unwrap();
        let mut command = Command::new("git");
        configure_git(&mut command, &work, remote, &ssh);
        let mut paths = vec![binaries];
        paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
        command
            .env("PATH", std::env::join_paths(paths).unwrap())
            .env("MANGROVE_TEST_REMOTE", &bare)
            .env("MANGROVE_TEST_SSH_ARGUMENTS", &arguments_file);
        let before = snapshot(&bare);
        let refs = parse_references(
            &run_command(
                &mut command,
                Duration::from_secs(5),
                &CancellationToken::new(),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(refs.len(), 2);
        assert!(refs.iter().all(|reference| reference.object_id == commit));
        let arguments = fs::read_to_string(arguments_file).unwrap();
        let arguments: Vec<_> = arguments.lines().collect();
        assert!(arguments
            .windows(2)
            .any(|pair| pair == ["-i", key.to_str().unwrap()]));
        assert!(arguments.contains(&"BatchMode=yes"));
        assert!(arguments.contains(&"StrictHostKeyChecking=yes"));
        assert!(arguments.contains(&"UpdateHostKeys=no"));
        assert!(arguments.contains(&"git@localhost"));
        assert_eq!(snapshot(&bare), before);
        assert!(fs::read_dir(&work).unwrap().next().is_none());
        assert!(!directory.path().join("unwanted").exists());
    }
}
