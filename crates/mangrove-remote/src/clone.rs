//! Explicit Git cloning into a newly reserved local directory.
//!
//! Hosting API credentials are never used by this module. HTTPS uses only the
//! machine's configured Git credential helpers; SSH uses the profile's existing
//! identity and known hosts. Interrupted clones are left in place, so no cleanup
//! can remove files added by the user or another process.

use std::{
    fs,
    path::{Component, Path, PathBuf},
    process::Command,
    time::Duration,
};

use crate::{ssh, CancellationToken, RemoteError, Result, SshChoice};

const CLONE_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const CONFIG_TIMEOUT: Duration = Duration::from_secs(10);

enum Source<'a> {
    Https(&'a str),
    Ssh(&'a str),
    #[cfg(test)]
    File(&'a str),
}

impl<'a> Source<'a> {
    fn parse(remote: &'a str) -> Result<Self> {
        if remote.starts_with("https://") {
            parse_https_remote(remote)?;
            Ok(Self::Https(remote))
        } else {
            ssh::validate_remote(remote)?;
            Ok(Self::Ssh(remote))
        }
    }

    fn remote(&self) -> &str {
        match self {
            Self::Https(remote) | Self::Ssh(remote) => remote,
            #[cfg(test)]
            Self::File(remote) => remote,
        }
    }

    fn protocol(&self) -> &'static str {
        match self {
            Self::Https(_) => "protocol.https.allow=always",
            Self::Ssh(_) => "protocol.ssh.allow=always",
            #[cfg(test)]
            Self::File(_) => "protocol.file.allow=always",
        }
    }
}

/// Use the same HTTPS contract for advertised clone links and submitted URLs.
pub(crate) fn parse_https_remote(remote: &str) -> Result<url::Url> {
    if !remote.starts_with("https://")
        || remote.len() > 4096
        || remote
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
        || remote.contains('\\')
    {
        return Err(RemoteError::InvalidConfiguration);
    }
    let lower = remote.to_ascii_lowercase();
    if ["%00", "%0a", "%0d"]
        .iter()
        .any(|encoded| lower.contains(encoded))
    {
        return Err(RemoteError::InvalidConfiguration);
    }
    let url = url::Url::parse(remote).map_err(|_| RemoteError::InvalidConfiguration)?;
    if url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() == "/"
        || url.port() == Some(0)
    {
        return Err(RemoteError::InvalidConfiguration);
    }
    Ok(url)
}

/// Clone into a new absolute directory whose parent already exists.
///
/// Run this synchronous operation on a background executor. Existing files,
/// directories and symlinks are refused, including an empty directory. The
/// destination is reserved atomically before Git starts and is never removed by
/// Mangrove. On an error or cancellation it may contain an incomplete clone;
/// choose a different new directory when retrying. Only a successful clone
/// returns a path suitable for opening in the application.
pub fn clone_repository(
    remote: &str,
    destination: &Path,
    ssh_choice: &SshChoice,
    cancel: &CancellationToken,
) -> Result<PathBuf> {
    cancel.check()?;
    let source = Source::parse(remote)?;
    let destination = destination_path(destination)?;
    let ssh_command = if matches!(source, Source::Ssh(_)) {
        Some(ssh::ssh_command(ssh_choice)?)
    } else {
        None
    };
    let directory = tempfile::tempdir().map_err(|_| RemoteError::CloneFailed)?;
    let credentials = if matches!(source, Source::Https(_)) {
        configured_credentials(directory.path(), cancel)?
    } else {
        Vec::new()
    };
    clone_source(
        source,
        &destination,
        ssh_command.as_deref(),
        &credentials,
        cancel,
    )
}

fn destination_path(destination: &Path) -> Result<PathBuf> {
    if !destination.is_absolute()
        || destination.to_string_lossy().len() > 4096
        || destination.to_string_lossy().chars().any(char::is_control)
        || destination
            .components()
            .any(|component| component == Component::ParentDir)
    {
        return Err(RemoteError::CloneDestinationInvalid);
    }
    let normalized: PathBuf = destination.components().collect();
    let name = normalized
        .file_name()
        .ok_or(RemoteError::CloneDestinationInvalid)?;
    let parent = normalized
        .parent()
        .ok_or(RemoteError::CloneDestinationInvalid)?;
    let parent = parent
        .canonicalize()
        .map_err(|_| RemoteError::CloneDestinationInvalid)?;
    if !parent.is_dir() {
        return Err(RemoteError::CloneDestinationInvalid);
    }
    let destination = parent.join(name);
    match fs::symlink_metadata(&destination) {
        Ok(_) => Err(RemoteError::CloneDestinationExists),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(destination),
        Err(_) => Err(RemoteError::CloneDestinationInvalid),
    }
}

fn clone_source(
    source: Source<'_>,
    destination: &Path,
    ssh_command: Option<&str>,
    credentials: &[(String, String)],
    cancel: &CancellationToken,
) -> Result<PathBuf> {
    cancel.check()?;
    // Do not replace this with exists()+rename(): rename can overwrite an empty
    // directory created by another process between those two operations.
    fs::create_dir(destination).map_err(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            RemoteError::CloneDestinationExists
        } else {
            RemoteError::CloneDestinationInvalid
        }
    })?;
    let mut command = Command::new("git");
    configure_clone(&mut command, &source, destination, ssh_command, credentials);
    ssh::run_command(&mut command, CLONE_TIMEOUT, cancel).map_err(|error| match error {
        RemoteError::SshAuthentication => RemoteError::CloneAuthentication,
        RemoteError::SshUnavailable | RemoteError::Network => RemoteError::CloneFailed,
        other => other,
    })?;
    Ok(destination.to_owned())
}

fn configure_clone(
    command: &mut Command,
    source: &Source<'_>,
    destination: &Path,
    ssh_command: Option<&str>,
    credentials: &[(String, String)],
) {
    // Rust canonicalization adds a verbatim prefix on Windows that Git rejects
    // in path arguments. Use the newly reserved directory as the working
    // directory rather than changing the canonical path's meaning.
    command
        .current_dir(destination)
        .args([
            "-c",
            "core.hooksPath=",
            "-c",
            "core.askPass=",
            "-c",
            "credential.interactive=false",
            "-c",
            "http.sslVerify=true",
            "-c",
            "http.followRedirects=false",
            "-c",
            "protocol.allow=never",
            "-c",
            source.protocol(),
            "clone",
            "--quiet",
            "--no-recurse-submodules",
            "--template=",
            "--origin=origin",
            "--",
            source.remote(),
        ])
        .arg(".");
    configure_environment(command, credentials);
    if let Some(ssh) = ssh_command {
        command
            .env("GIT_SSH_COMMAND", ssh)
            .env("GIT_SSH_VARIANT", "ssh");
    }
}

fn configure_environment(command: &mut Command, credentials: &[(String, String)]) {
    ssh::configure_isolated_git_environment(command);
    command.env("GIT_CONFIG_COUNT", credentials.len().to_string());
    for (index, (key, value)) in credentials.iter().enumerate() {
        command
            .env(format!("GIT_CONFIG_KEY_{index}"), key)
            .env(format!("GIT_CONFIG_VALUE_{index}"), value);
    }
}

fn configured_credentials(
    directory: &Path,
    cancel: &CancellationToken,
) -> Result<Vec<(String, String)>> {
    let mut command = Command::new("git");
    command
        .current_dir(directory)
        .args(["config", "--null", "--list", "--includes"]);
    // Read normal system/global credential configuration, never the currently
    // open repository or inherited GIT_CONFIG_* injection. No helper runs here.
    ssh::remove_git_environment(&mut command);
    let output =
        ssh::run_command(&mut command, CONFIG_TIMEOUT, cancel).map_err(|error| match error {
            RemoteError::SshUnavailable | RemoteError::Network => RemoteError::CloneFailed,
            other => other,
        })?;
    parse_credentials(&output)
}

fn parse_credentials(output: &[u8]) -> Result<Vec<(String, String)>> {
    let mut credentials = Vec::new();
    for entry in output
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
    {
        let (key, value) = match entry.iter().position(|byte| *byte == b'\n') {
            Some(split) => (&entry[..split], Some(&entry[split + 1..])),
            None => (entry, None),
        };
        let key = std::str::from_utf8(key).map_err(|_| RemoteError::CloneFailed)?;
        let Some(suffix) = key.strip_prefix("credential.") else {
            continue;
        };
        let field = suffix.rsplit('.').next().unwrap_or_default();
        if !matches!(field, "helper" | "username" | "usehttppath") {
            continue;
        }
        if key.len() > 4096 || key.chars().any(char::is_control) || credentials.len() >= 128 {
            return Err(RemoteError::CloneFailed);
        }
        let value = match value {
            Some(value) => std::str::from_utf8(value).map_err(|_| RemoteError::CloneFailed)?,
            None if field == "usehttppath" => "true",
            None => return Err(RemoteError::CloneFailed),
        };
        if value.len() > 16 * 1024 {
            return Err(RemoteError::CloneFailed);
        }
        credentials.push((key.to_owned(), value.to_owned()));
    }
    Ok(credentials)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    fn git(directory: &Path, arguments: &[&OsStr]) -> String {
        let mut command = Command::new("git");
        command.current_dir(directory).args(arguments);
        ssh::remove_git_environment(&mut command);
        let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
        let output = command
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", null)
            .env("GIT_CONFIG_SYSTEM", null)
            .env("GIT_CONFIG_COUNT", "0")
            .output()
            .unwrap();
        assert!(output.status.success(), "fixture Git command failed");
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    #[test]
    fn rejects_unsafe_urls_before_creating_a_destination() {
        let directory = tempfile::tempdir().unwrap();
        for remote in [
            "file:///tmp/repo",
            "http://example.com/repo",
            "git://example.com/repo",
            "ext::touch secret",
            "../repo",
            "https://user:secret@example.com/repo",
            "https://user@example.com/repo",
            "https://example.com/repo?token=secret",
            "https://example.com/repo#secret",
            "https://example.com/repo\n",
            "https://example.com/repo%0Aheader",
            "https://example.com/",
            "https://example.com:0/repo",
        ] {
            let destination = directory.path().join("new-repository");
            assert_eq!(
                clone_repository(
                    remote,
                    &destination,
                    &SshChoice::Disabled,
                    &CancellationToken::new()
                ),
                Err(RemoteError::InvalidConfiguration),
                "{remote}"
            );
            assert!(!destination.exists());
        }
    }

    #[test]
    fn existing_directories_and_files_are_never_reused_or_removed() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("repo");
        fs::create_dir(&destination).unwrap();
        for populated in [false, true] {
            if populated {
                fs::write(destination.join("keep"), "user content").unwrap();
            }
            assert_eq!(
                clone_repository(
                    "https://example.com/repo.git",
                    &destination,
                    &SshChoice::Disabled,
                    &CancellationToken::new()
                ),
                Err(RemoteError::CloneDestinationExists)
            );
        }
        assert_eq!(fs::read(destination.join("keep")).unwrap(), b"user content");
        let file = directory.path().join("file");
        fs::write(&file, "keep file").unwrap();
        assert_eq!(
            clone_repository(
                "https://example.com/repo.git",
                &file,
                &SshChoice::Disabled,
                &CancellationToken::new()
            ),
            Err(RemoteError::CloneDestinationExists)
        );
        assert_eq!(fs::read(&file).unwrap(), b"keep file");
    }

    #[test]
    fn destination_created_after_validation_still_cannot_be_overwritten() {
        let directory = tempfile::tempdir().unwrap();
        let destination = destination_path(&directory.path().join("repo")).unwrap();
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("keep"), "concurrent content").unwrap();
        assert_eq!(
            clone_source(
                Source::File("file:///does-not-exist"),
                &destination,
                None,
                &[],
                &CancellationToken::new()
            ),
            Err(RemoteError::CloneDestinationExists)
        );
        assert_eq!(
            fs::read(destination.join("keep")).unwrap(),
            b"concurrent content"
        );
    }

    #[test]
    fn real_clone_contains_history_tags_origin_and_checkout_without_source_dependency() {
        let directory = tempfile::tempdir().unwrap();
        git(
            directory.path(),
            &[OsStr::new("init"), OsStr::new("--initial-branch=main")],
        );
        for (key, value) in [
            ("url.file:///missing-parent-rewrite/.insteadOf", "file:///"),
            ("core.hooksPath", "missing-parent-hooks"),
            ("filter.fixture.smudge", "missing-parent-smudge-filter"),
            ("filter.fixture.required", "true"),
            ("credential.helper", "!exit 99"),
        ] {
            git(
                directory.path(),
                &[OsStr::new("config"), OsStr::new(key), OsStr::new(value)],
            );
        }
        let parent_config = fs::read(directory.path().join(".git/config")).unwrap();
        let source = directory.path().join("source");
        fs::create_dir(&source).unwrap();
        git(
            &source,
            &[OsStr::new("init"), OsStr::new("--initial-branch=main")],
        );
        fs::write(source.join("README.md"), "fixture repository\n").unwrap();
        fs::write(source.join(".gitattributes"), "README.md filter=fixture\n").unwrap();
        git(
            &source,
            &[
                OsStr::new("add"),
                OsStr::new("README.md"),
                OsStr::new(".gitattributes"),
            ],
        );
        let commit = [
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-m",
            "fixture",
        ];
        git(&source, &commit.iter().map(OsStr::new).collect::<Vec<_>>());
        git(&source, &[OsStr::new("tag"), OsStr::new("v1")]);
        let expected_head = git(&source, &[OsStr::new("rev-parse"), OsStr::new("HEAD")]);
        let url = url::Url::from_directory_path(&source).unwrap().to_string();
        let destination = destination_path(&directory.path().join("clone with spaces")).unwrap();
        let cloned = clone_source(
            Source::File(&url),
            &destination,
            None,
            &[],
            &CancellationToken::new(),
        )
        .unwrap();
        assert_eq!(cloned, destination);
        assert_eq!(
            fs::read(directory.path().join(".git/config")).unwrap(),
            parent_config,
            "cloning must ignore and preserve the enclosing repository configuration"
        );
        assert_eq!(
            fs::read(cloned.join("README.md")).unwrap(),
            b"fixture repository\n"
        );
        assert_eq!(
            git(&cloned, &[OsStr::new("rev-parse"), OsStr::new("HEAD")]),
            expected_head
        );
        assert_eq!(
            git(&cloned, &[OsStr::new("tag"), OsStr::new("--list")]),
            "v1"
        );
        assert_eq!(
            git(
                &cloned,
                &[
                    OsStr::new("remote"),
                    OsStr::new("get-url"),
                    OsStr::new("origin")
                ]
            ),
            url
        );
        assert!(!cloned.join(".git/objects/info/alternates").exists());
        fs::remove_dir_all(source).unwrap();
        assert_eq!(
            git(&cloned, &[OsStr::new("show"), OsStr::new("HEAD:README.md")]),
            "fixture repository"
        );
    }

    #[test]
    fn failed_clone_retains_only_new_destination_and_exposes_no_remote_diagnostic() {
        let directory = tempfile::tempdir().unwrap();
        let destination = destination_path(&directory.path().join("clone")).unwrap();
        let remote =
            url::Url::from_directory_path(directory.path().join("secret-upstream-does-not-exist"))
                .unwrap()
                .to_string();
        let error = clone_source(
            Source::File(&remote),
            &destination,
            None,
            &[],
            &CancellationToken::new(),
        )
        .unwrap_err();
        assert_eq!(error, RemoteError::CloneFailed);
        assert!(!error.to_string().contains("secret-upstream"));
        assert!(destination.is_dir());
    }

    #[test]
    fn credentials_keep_helpers_and_url_context_but_ignore_rewrites_filters_and_tls_overrides() {
        let output = b"credential.helper\nosxkeychain\0credential.helper\n\0credential.https://example.com.helper\nhelper with arguments\0credential.usehttppath\ntrue\0credential.username\nfixture\0credential.interactive\ntrue\0url.file:///private/.insteadof\nhttps://example.com/\0filter.external.smudge\nsh secret\0http.sslverify\nfalse\0";
        let credentials = parse_credentials(output).unwrap();
        assert_eq!(
            credentials,
            vec![
                ("credential.helper".into(), "osxkeychain".into()),
                ("credential.helper".into(), "".into()),
                (
                    "credential.https://example.com.helper".into(),
                    "helper with arguments".into()
                ),
                ("credential.usehttppath".into(), "true".into()),
                ("credential.username".into(), "fixture".into()),
            ]
        );
    }

    #[test]
    fn already_cancelled_clone_does_not_reserve_destination() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("clone");
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert_eq!(
            clone_repository(
                "https://example.com/repo.git",
                &destination,
                &SshChoice::Disabled,
                &cancel
            ),
            Err(RemoteError::Cancelled)
        );
        assert!(!destination.exists());
    }

    #[test]
    fn real_git_uses_configured_context_helper_without_reading_other_credentials() {
        use std::{io::Write, process::Stdio};

        let directory = tempfile::tempdir().unwrap();
        let helper = "!f() { while IFS= read -r line; do case \"$line\" in path=team/repo.git) matched=1 ;; esac; done; if test \"$matched\" = 1; then printf 'username=fixture\\npassword=synthetic-fixture-secret\\n'; fi; }; f";
        let config = format!(
            "credential.https://example.invalid.helper\n{helper}\0credential.usehttppath\0"
        );
        let credentials = parse_credentials(config.as_bytes()).unwrap();
        let mut command = Command::new("git");
        command.current_dir(directory.path()).args([
            "-c",
            "credential.interactive=false",
            "credential",
            "fill",
        ]);
        configure_environment(&mut command, &credentials);
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"protocol=https\nhost=example.invalid\npath=team/repo.git\n\n")
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        let credentials = String::from_utf8(output.stdout).unwrap();
        assert!(credentials.contains("username=fixture\n"));
        assert!(credentials.contains("password=synthetic-fixture-secret\n"));
        assert!(
            fs::read_dir(directory.path()).unwrap().next().is_none(),
            "credentials must not be written to the clone directory"
        );
    }

    #[cfg(unix)]
    #[test]
    fn existing_symlink_destination_is_refused_without_touching_its_target() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("keep");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("content"), "user content").unwrap();
        let destination = directory.path().join("symlink");
        symlink(&target, &destination).unwrap();
        assert_eq!(
            clone_repository(
                "https://example.com/repo.git",
                &destination,
                &SshChoice::Disabled,
                &CancellationToken::new()
            ),
            Err(RemoteError::CloneDestinationExists)
        );
        assert_eq!(fs::read(target.join("content")).unwrap(), b"user content");
        assert!(destination
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[cfg(unix)]
    #[test]
    fn cancelling_real_git_clone_stops_ssh_and_descendants_and_preserves_partial_directory() {
        use std::{thread, time::Instant};

        let directory = tempfile::tempdir().unwrap();
        let destination = destination_path(&directory.path().join("clone")).unwrap();
        let pid_file = directory.path().join("pids");
        let script = directory.path().join("fixture-ssh");
        let quote = |path: &Path| format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"));
        fs::write(
            &script,
            format!(
                "#!/bin/sh\nsleep 30 &\nhelper=$!\nprintf '%s\\n%s\\n' \"$$\" \"$helper\" > {}\nwait \"$helper\"\n",
                quote(&pid_file)
            ),
        ).unwrap();
        let ssh = format!("sh {}", quote(&script));
        let cancel = CancellationToken::new();
        let worker_cancel = cancel.clone();
        let worker_destination = destination.clone();
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let worker = thread::spawn(move || {
            let result = clone_source(
                Source::Ssh("git@fixture.invalid:repo.git"),
                &worker_destination,
                Some(&ssh),
                &[],
                &worker_cancel,
            );
            let _ = sender.send(result);
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        let pids = loop {
            // Shell redirection creates the file before printf fills it. Wait
            // for both complete PID lines before cancelling the real SSH child.
            if let Ok(pids) = fs::read_to_string(&pid_file) {
                if pids.ends_with('\n')
                    && pids.lines().count() == 2
                    && pids.lines().all(|pid| pid.parse::<u32>().is_ok())
                {
                    break pids;
                }
            }
            if Instant::now() >= deadline {
                cancel.cancel();
                worker.join().unwrap();
                panic!("real Git must have started its SSH child before cancellation");
            }
            thread::sleep(Duration::from_millis(10));
        };
        let pids: Vec<_> = pids.lines().collect();
        assert_eq!(pids.len(), 2);
        cancel.cancel();
        assert_eq!(
            receiver.recv_timeout(Duration::from_secs(5)).unwrap(),
            Err(RemoteError::Cancelled)
        );
        worker.join().unwrap();
        assert!(destination.is_dir());
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
            thread::sleep(Duration::from_millis(10));
        }
        for pid in pids {
            assert!(
                !is_running(pid),
                "clone descendant {pid} survived cancellation"
            );
        }
    }
}
