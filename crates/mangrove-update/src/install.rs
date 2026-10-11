//! Puts a verified archive in place of the running application. The new files
//! are prepared beside the installation, on the same volume, then renamed into
//! place: a failure leaves the current application as it was. What they
//! replace stays aside until a later launch, since the running process may
//! still read it.
use crate::{Result, UpdateError};
use std::{
    fmt, fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

/// Hidden folders beside the installation, named after the process: one
/// prepares an installation, the other keeps what an installation replaced.
const STAGING_PREFIX: &str = ".mangrove-update-";
const PREVIOUS_PREFIX: &str = ".mangrove-previous-";
/// No installation takes that long: its staging folder was abandoned.
const ABANDONED_AFTER: Duration = Duration::from_secs(60 * 60);
const BUNDLE_EXECUTABLE: &str = "Contents/MacOS/mangrove-gpui";
const LINUX_EXECUTABLE: &str = "bin/mangrove-gpui";
/// Installed with the Linux binary, it tells a release prefix from a build.
const LINUX_DESKTOP_ENTRY: &str = "share/applications/dev.mangrove.gpui.desktop";
const WINDOWS_EXECUTABLE: &str = "Mangrove.exe";

/// Where the running application was installed from a release archive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Installation {
    /// `Mangrove.app`, replaced as a whole.
    MacosBundle(PathBuf),
    /// The prefix where the archive's `bin/` and `share/` were extracted.
    LinuxPrefix(PathBuf),
    /// The folder of `Mangrove.exe` and its licenses.
    WindowsFolder(PathBuf),
}

/// Why the running application cannot replace itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotInstallable {
    /// Built from the sources rather than installed from a release.
    LocalBuild,
    /// macOS runs a downloaded application from a read-only copy until it
    /// is moved elsewhere.
    Translocated,
}

impl fmt::Display for NotInstallable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::LocalBuild => {
                "Ce build n’a pas été installé depuis une release : il ne se remplace pas lui-même."
            }
            Self::Translocated => {
                "macOS exécute Mangrove depuis une copie temporaire : déplacez l’application dans Applications pour qu’elle se mette à jour."
            }
        })
    }
}

impl Installation {
    /// The installation of the running executable.
    pub fn detect() -> std::result::Result<Self, NotInstallable> {
        let executable = std::env::current_exe().map_err(|_| NotInstallable::LocalBuild)?;
        if cfg!(target_os = "macos") {
            detect_bundle(&executable)
        } else if cfg!(target_os = "linux") {
            detect_prefix(&executable)
        } else if cfg!(windows) {
            detect_folder(&executable)
        } else {
            Err(NotInstallable::LocalBuild)
        }
    }

    /// What starts the installed application.
    pub fn launch_path(&self) -> PathBuf {
        match self {
            Self::MacosBundle(bundle) => bundle.clone(),
            Self::LinuxPrefix(prefix) => prefix.join(LINUX_EXECUTABLE),
            Self::WindowsFolder(folder) => folder.join(WINDOWS_EXECUTABLE),
        }
    }

    /// Replaces the installation with the application of `archive`, a
    /// release archive of this platform whose signature was checked.
    pub fn install(&self, archive: &Path) -> Result<()> {
        let staging = Staging::create(self.parent())?;
        let extracted = staging.path.join("new");
        let previous = staging.path.join("previous");
        match self {
            Self::MacosBundle(bundle) => {
                let app = copy_from_disk_image(archive, &extracted)?;
                swap(bundle, &app, &previous)?;
            }
            Self::LinuxPrefix(prefix) => {
                fs::create_dir(&extracted)?;
                run(
                    Command::new("tar")
                        .arg("-xzf")
                        .arg(archive)
                        .arg("-C")
                        .arg(&extracted),
                    "tar",
                )?;
                replace_files(
                    &extracted.join("mangrove"),
                    prefix,
                    Path::new(LINUX_EXECUTABLE),
                    &previous,
                )?;
            }
            Self::WindowsFolder(folder) => {
                fs::create_dir(&extracted)?;
                run(
                    Command::new(windows_tar())
                        .arg("-xf")
                        .arg(archive)
                        .arg("-C")
                        .arg(&extracted),
                    "tar",
                )?;
                replace_files(
                    &extracted.join("Mangrove"),
                    folder,
                    Path::new(WINDOWS_EXECUTABLE),
                    &previous,
                )?;
            }
        }
        staging.keep();
        Ok(())
    }

    /// Removes what earlier installations replaced, once their process has
    /// quit, and abandoned staging folders. Another process may be installing.
    pub fn remove_leftovers(&self) {
        remove_leftovers(self.parent(), ABANDONED_AFTER);
    }

    /// The folder that receives the staging area.
    fn parent(&self) -> &Path {
        match self {
            Self::MacosBundle(bundle) => bundle.parent().unwrap_or(bundle),
            Self::LinuxPrefix(folder) | Self::WindowsFolder(folder) => folder,
        }
    }
}

fn detect_bundle(executable: &Path) -> std::result::Result<Installation, NotInstallable> {
    let bundle = executable
        .parent()
        .filter(|folder| folder.ends_with("MacOS"))
        .and_then(Path::parent)
        .filter(|folder| folder.ends_with("Contents"))
        .and_then(Path::parent)
        .filter(|bundle| {
            bundle
                .extension()
                .is_some_and(|extension| extension == "app")
        })
        .ok_or(NotInstallable::LocalBuild)?;
    if bundle
        .components()
        .any(|component| component.as_os_str() == "AppTranslocation")
    {
        return Err(NotInstallable::Translocated);
    }
    Ok(Installation::MacosBundle(bundle.to_path_buf()))
}

fn detect_prefix(executable: &Path) -> std::result::Result<Installation, NotInstallable> {
    executable
        .parent()
        .filter(|bin| bin.ends_with("bin"))
        .and_then(Path::parent)
        .filter(|prefix| {
            prefix.join(LINUX_EXECUTABLE) == executable
                && prefix.join(LINUX_DESKTOP_ENTRY).is_file()
        })
        .map(|prefix| Installation::LinuxPrefix(prefix.to_path_buf()))
        .ok_or(NotInstallable::LocalBuild)
}

fn detect_folder(executable: &Path) -> std::result::Result<Installation, NotInstallable> {
    // Cargo builds `mangrove-gpui.exe`; the release archive renames it.
    executable
        .file_name()
        .filter(|name| name.eq_ignore_ascii_case(WINDOWS_EXECUTABLE))
        .and(executable.parent())
        .map(|folder| Installation::WindowsFolder(folder.to_path_buf()))
        .ok_or(NotInstallable::LocalBuild)
}

fn remove_leftovers(folder: &Path, abandoned_after: Duration) {
    let Ok(entries) = fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let remove = if let Some(process) = name.strip_prefix(PREVIOUS_PREFIX) {
            process.parse().is_ok_and(|process| !is_running(process))
        } else if name.starts_with(STAGING_PREFIX) {
            entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .ok()
                .and_then(|modified| modified.elapsed().ok())
                .is_some_and(|age| age >= abandoned_after)
        } else {
            false
        };
        if remove {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

/// Whether the process `id` still runs.
#[cfg(unix)]
fn is_running(id: u32) -> bool {
    let Ok(id) = libc::pid_t::try_from(id) else {
        return false;
    };
    // Signal 0 only checks that the process exists; 0 and negative
    // identifiers would address process groups.
    id > 0
        && (unsafe { libc::kill(id, 0) } == 0
            || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM))
}

/// Windows refuses to delete the files a running process was started from,
/// which keeps them until it quits.
#[cfg(not(unix))]
fn is_running(id: u32) -> bool {
    id == std::process::id()
}

/// A hidden folder beside the installation, removed with what it holds
/// unless the installation succeeds.
struct Staging {
    path: PathBuf,
    kept: bool,
}

impl Staging {
    fn create(parent: &Path) -> Result<Self> {
        let path = parent.join(format!("{STAGING_PREFIX}{}", std::process::id()));
        // A failed attempt of this process may have left it.
        let _ = fs::remove_dir_all(&path);
        fs::create_dir(&path)?;
        Ok(Self { path, kept: false })
    }

    /// Keeps what the installation replaced for a later launch to remove.
    fn keep(mut self) {
        let kept = self
            .path
            .with_file_name(format!("{PREVIOUS_PREFIX}{}", std::process::id()));
        self.kept = fs::rename(&self.path, kept).is_ok();
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        if !self.kept {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

/// Copies `Mangrove.app` out of the release disk image into `destination`.
fn copy_from_disk_image(image: &Path, destination: &Path) -> Result<PathBuf> {
    let volume = tempfile::Builder::new()
        .prefix("mangrove-volume-")
        .tempdir()?;
    run(
        Command::new("/usr/bin/hdiutil")
            .args([
                "attach",
                "-nobrowse",
                "-readonly",
                "-noautoopen",
                "-noverify",
                "-mountpoint",
            ])
            .arg(volume.path())
            .arg(image),
        "hdiutil",
    )?;
    let source = volume.path().join("Mangrove.app");
    let app = destination.join("Mangrove.app");
    let copied = if source.join(BUNDLE_EXECUTABLE).is_file() {
        run(
            Command::new("/usr/bin/ditto").arg(&source).arg(&app),
            "ditto",
        )
    } else {
        Err(UpdateError::InvalidArchive)
    };
    let detach = |force: bool| {
        let mut command = Command::new("/usr/bin/hdiutil");
        command.arg("detach").arg(volume.path());
        if force {
            command.arg("-force");
        }
        run(&mut command, "hdiutil")
    };
    if detach(false).is_err() && detach(true).is_err() {
        // Still mounted: removing the mount point would reach into the volume.
        let _ = volume.keep();
    }
    copied.map(|()| app)
}

/// Moves every file of `source` to the same place under `target`, the
/// executable last so that it never starts beside older files. Each file
/// replaced first moves to the same place under `previous`: Windows cannot
/// overwrite a running executable but can rename it, and a failure puts
/// every file back.
fn replace_files(source: &Path, target: &Path, executable: &Path, previous: &Path) -> Result<()> {
    if !source.join(executable).is_file() {
        return Err(UpdateError::InvalidArchive);
    }
    let mut files = Vec::new();
    list_files(source, Path::new(""), &mut files)?;
    files.sort();
    files.sort_by_key(|file| file == executable);
    let mut moved = Vec::new();
    for file in &files {
        match replace_file(source, target, previous, file) {
            Ok(replaced) => moved.push((file, replaced)),
            Err(error) => {
                for (file, replaced) in moved.into_iter().rev() {
                    let destination = target.join(file);
                    let _ = if replaced {
                        fs::rename(previous.join(file), destination)
                    } else {
                        fs::remove_file(destination)
                    };
                }
                return Err(error);
            }
        }
    }
    Ok(())
}

/// Moves `file` from `source` to `target`; returns whether it replaced a
/// file, which then moved to `previous`.
fn replace_file(source: &Path, target: &Path, previous: &Path, file: &Path) -> Result<bool> {
    let (destination, aside) = (target.join(file), previous.join(file));
    for folder in [destination.parent(), aside.parent()].into_iter().flatten() {
        fs::create_dir_all(folder)?;
    }
    let replaced = destination.symlink_metadata().is_ok();
    if replaced {
        swap(&destination, &source.join(file), &aside)?;
    } else {
        fs::rename(source.join(file), destination)?;
    }
    Ok(replaced)
}

fn list_files(root: &Path, relative: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(root.join(relative))? {
        let entry = entry?;
        let path = relative.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            list_files(root, &path, files)?;
        } else {
            files.push(path);
        }
    }
    Ok(())
}

/// Moves `current` to `previous`, then `replacement` to `current`; puts
/// `current` back when the second move fails.
fn swap(current: &Path, replacement: &Path, previous: &Path) -> Result<()> {
    fs::rename(current, previous)?;
    if let Err(error) = fs::rename(replacement, current) {
        let _ = fs::rename(previous, current);
        return Err(error.into());
    }
    Ok(())
}

/// The `tar` of Windows 10 and later, which reads zip archives. The one Git
/// for Windows may put first in the `PATH` does not.
fn windows_tar() -> PathBuf {
    std::env::var_os("SystemRoot")
        .map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from)
        .join("System32")
        .join("tar.exe")
}

fn run(command: &mut Command, tool: &'static str) -> Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // A GUI application would otherwise flash a console window.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let output = command
        .stdin(Stdio::null())
        .output()
        .map_err(|_| UpdateError::Tool(tool, String::new()))?;
    if output.status.success() {
        return Ok(());
    }
    let error = String::from_utf8_lossy(&output.stderr);
    let line = error
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .unwrap_or_default();
    Err(UpdateError::Tool(tool, line.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path).unwrap()
    }

    #[test]
    fn bundles_outside_translocation_are_installable() {
        let bundle = Path::new("/Applications/Mangrove.app");
        assert_eq!(
            detect_bundle(&bundle.join(BUNDLE_EXECUTABLE)),
            Ok(Installation::MacosBundle(bundle.into()))
        );
        assert_eq!(
            detect_bundle(Path::new(
                "/private/var/folders/x/AppTranslocation/1/d/Mangrove.app/Contents/MacOS/mangrove-gpui"
            )),
            Err(NotInstallable::Translocated)
        );
        for executable in [
            "/repo/src-gpui/target/release/mangrove-gpui",
            "/Applications/Mangrove/Contents/MacOS/mangrove-gpui",
            "/Applications/Mangrove.app/Contents/Resources/mangrove-gpui",
        ] {
            assert_eq!(
                detect_bundle(Path::new(executable)),
                Err(NotInstallable::LocalBuild),
                "{executable}"
            );
        }
    }

    #[test]
    fn a_linux_prefix_needs_its_desktop_entry() {
        let directory = tempfile::tempdir().unwrap();
        let prefix = directory.path().join("local");
        let executable = prefix.join(LINUX_EXECUTABLE);
        write(&executable, "binary");
        assert_eq!(detect_prefix(&executable), Err(NotInstallable::LocalBuild));
        write(&prefix.join(LINUX_DESKTOP_ENTRY), "[Desktop Entry]");
        assert_eq!(
            detect_prefix(&executable),
            Ok(Installation::LinuxPrefix(prefix.clone()))
        );
        assert_eq!(
            Installation::LinuxPrefix(prefix.clone()).launch_path(),
            executable
        );
        let renamed = prefix.join("bin/mangrove");
        write(&renamed, "binary");
        assert_eq!(detect_prefix(&renamed), Err(NotInstallable::LocalBuild));
    }

    #[test]
    fn only_the_released_windows_executable_is_installable() {
        let folder = Path::new("Mangrove");
        assert_eq!(
            detect_folder(&folder.join("Mangrove.exe")),
            Ok(Installation::WindowsFolder(folder.into()))
        );
        assert_eq!(
            detect_folder(&folder.join("mangrove.EXE")),
            Ok(Installation::WindowsFolder(folder.into()))
        );
        assert_eq!(
            detect_folder(Path::new("target/release/mangrove-gpui.exe")),
            Err(NotInstallable::LocalBuild)
        );
    }

    #[test]
    fn files_replace_their_counterparts_and_leave_other_files_alone() {
        let directory = tempfile::tempdir().unwrap();
        let (source, target) = (directory.path().join("new"), directory.path().join("app"));
        let previous = directory.path().join("previous");
        write(&source.join("bin/mangrove-gpui"), "new binary");
        write(&source.join("share/doc/mangrove/LICENSE.md"), "new license");
        write(
            &source.join("share/icons/hicolor/32x32/apps/icon.png"),
            "icon",
        );
        write(&target.join("bin/mangrove-gpui"), "old binary");
        write(&target.join("bin/other-tool"), "untouched");
        write(&target.join("share/doc/mangrove/LICENSE.md"), "old license");
        replace_files(&source, &target, Path::new("bin/mangrove-gpui"), &previous).unwrap();
        assert_eq!(read(&target.join("bin/mangrove-gpui")), "new binary");
        assert_eq!(read(&target.join("bin/other-tool")), "untouched");
        assert_eq!(
            read(&target.join("share/doc/mangrove/LICENSE.md")),
            "new license"
        );
        assert_eq!(
            read(&target.join("share/icons/hicolor/32x32/apps/icon.png")),
            "icon"
        );
        assert_eq!(read(&previous.join("bin/mangrove-gpui")), "old binary");
        assert_eq!(
            read(&previous.join("share/doc/mangrove/LICENSE.md")),
            "old license"
        );
    }

    #[test]
    fn an_archive_without_the_executable_changes_nothing() {
        let directory = tempfile::tempdir().unwrap();
        let (source, target) = (directory.path().join("new"), directory.path().join("app"));
        write(&source.join("LICENSE.md"), "new license");
        write(&target.join("LICENSE.md"), "old license");
        assert!(matches!(
            replace_files(
                &source,
                &target,
                Path::new("Mangrove.exe"),
                &directory.path().join("previous"),
            ),
            Err(UpdateError::InvalidArchive)
        ));
        assert_eq!(read(&target.join("LICENSE.md")), "old license");
    }

    #[test]
    fn a_failed_swap_restores_the_current_application() {
        let directory = tempfile::tempdir().unwrap();
        let current = directory.path().join("Mangrove.app");
        write(&current.join(BUNDLE_EXECUTABLE), "old");
        assert!(swap(
            &current,
            &directory.path().join("missing.app"),
            &directory.path().join("previous"),
        )
        .is_err());
        assert_eq!(read(&current.join(BUNDLE_EXECUTABLE)), "old");
    }

    #[test]
    fn a_failure_puts_every_replaced_file_back() {
        let directory = tempfile::tempdir().unwrap();
        let (source, target) = (directory.path().join("new"), directory.path().join("app"));
        write(&source.join("a.txt"), "new a");
        write(&source.join("bin/mangrove-gpui"), "new binary");
        write(&source.join("z/icon.png"), "icon");
        write(&target.join("a.txt"), "old a");
        write(&target.join("bin/mangrove-gpui"), "old binary");
        // A file where the archive needs a folder stops the replacement.
        write(&target.join("z"), "in the way");
        assert!(replace_files(
            &source,
            &target,
            Path::new("bin/mangrove-gpui"),
            &directory.path().join("previous"),
        )
        .is_err());
        assert_eq!(read(&target.join("a.txt")), "old a");
        assert_eq!(read(&target.join("bin/mangrove-gpui")), "old binary");
        assert_eq!(read(&target.join("z")), "in the way");
    }

    #[test]
    fn leftovers_wait_for_their_process_and_staging_for_its_abandon() {
        let directory = tempfile::tempdir().unwrap();
        let folder = directory.path();
        let own = std::process::id();
        // Above any process identifier in use.
        let ended = 0x7fff_fff0;
        for name in [
            format!("{PREVIOUS_PREFIX}{ended}"),
            format!("{PREVIOUS_PREFIX}{own}"),
            format!("{STAGING_PREFIX}{ended}"),
        ] {
            write(&folder.join(name).join("Mangrove.exe"), "old");
        }
        write(&folder.join("Mangrove.exe"), "current");
        remove_leftovers(folder, ABANDONED_AFTER);
        assert!(!folder.join(format!("{PREVIOUS_PREFIX}{ended}")).exists());
        assert!(folder.join(format!("{PREVIOUS_PREFIX}{own}")).exists());
        assert!(folder.join(format!("{STAGING_PREFIX}{ended}")).exists());
        remove_leftovers(folder, Duration::ZERO);
        assert!(!folder.join(format!("{STAGING_PREFIX}{ended}")).exists());
        assert_eq!(read(&folder.join("Mangrove.exe")), "current");
    }

    /// Builds a release archive of `layout` with the tools of the platform,
    /// as the release workflow does, then installs it over `old_files`.
    /// Returns the temporary folder; the installation is in `installed`.
    fn install_release_archive(
        layout: &[(&str, &str)],
        archive: impl FnOnce(&Path, &Path) -> Command,
        archive_name: &str,
        installation: impl FnOnce(&Path) -> Installation,
        old_files: &[(&str, &str)],
    ) -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        let stage = directory.path().join("stage");
        for (path, text) in layout {
            write(&stage.join(path), text);
        }
        let path = directory.path().join(archive_name);
        assert!(archive(&stage, &path).status().unwrap().success());
        let installed = directory.path().join("installed");
        for (path, text) in old_files {
            write(&installed.join(path), text);
        }
        let installation = installation(&installed);
        installation.install(&path).unwrap();
        let process = std::process::id();
        assert!(!installed
            .join(format!("{STAGING_PREFIX}{process}"))
            .exists());
        assert!(
            installed
                .join(format!("{PREVIOUS_PREFIX}{process}/previous"))
                .exists(),
            "what was replaced waits for a later launch"
        );
        directory
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_disk_image_replaces_the_application_bundle() {
        let directory = install_release_archive(
            &[
                ("Mangrove.app/Contents/MacOS/mangrove-gpui", "new binary"),
                ("Mangrove.app/Contents/Info.plist", "new plist"),
                ("LICENSE.md", "license"),
            ],
            |stage, path| {
                let mut command = Command::new("/usr/bin/hdiutil");
                command
                    .args([
                        "create", "-quiet", "-volname", "Mangrove", "-format", "UDZO",
                    ])
                    .arg("-srcfolder")
                    .arg(stage)
                    .arg(path);
                command
            },
            "mangrove-0.4.0-macos-arm64.dmg",
            |parent| Installation::MacosBundle(parent.join("Mangrove.app")),
            &[
                ("Mangrove.app/Contents/MacOS/mangrove-gpui", "old binary"),
                ("Mangrove.app/Contents/Resources/removed.icns", "old"),
            ],
        );
        let installed = directory.path().join("installed");
        let bundle = installed.join("Mangrove.app");
        assert_eq!(read(&bundle.join(BUNDLE_EXECUTABLE)), "new binary");
        assert_eq!(read(&bundle.join("Contents/Info.plist")), "new plist");
        assert!(!bundle.join("Contents/Resources/removed.icns").exists());
        assert!(!installed.join("LICENSE.md").exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_tarball_replaces_the_files_of_its_prefix() {
        let directory = install_release_archive(
            &[
                ("mangrove/bin/mangrove-gpui", "new binary"),
                (
                    "mangrove/share/applications/dev.mangrove.gpui.desktop",
                    "new entry",
                ),
            ],
            |stage, path| {
                let mut command = Command::new("tar");
                command
                    .arg("-czf")
                    .arg(path)
                    .arg("-C")
                    .arg(stage)
                    .arg("mangrove");
                command
            },
            "mangrove-0.4.0-linux-x86_64.tar.gz",
            |parent| Installation::LinuxPrefix(parent.to_path_buf()),
            &[
                ("bin/mangrove-gpui", "old binary"),
                ("bin/other-tool", "untouched"),
                ("share/applications/dev.mangrove.gpui.desktop", "old entry"),
            ],
        );
        let installed = directory.path().join("installed");
        assert_eq!(read(&installed.join("bin/mangrove-gpui")), "new binary");
        assert_eq!(read(&installed.join("bin/other-tool")), "untouched");
        assert_eq!(
            read(&installed.join("share/applications/dev.mangrove.gpui.desktop")),
            "new entry"
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_zip_archive_replaces_the_executable_and_its_licenses() {
        let directory = install_release_archive(
            &[
                ("Mangrove/Mangrove.exe", "new binary"),
                ("Mangrove/LICENSE.md", "new license"),
            ],
            |stage, path| {
                // bsdtar picks the zip format from the extension, like
                // Compress-Archive in the release workflow.
                let mut command = Command::new(windows_tar());
                command
                    .arg("-a")
                    .arg("-cf")
                    .arg(path)
                    .arg("-C")
                    .arg(stage)
                    .arg("Mangrove");
                command
            },
            "mangrove-0.4.0-windows-x86_64.zip",
            |parent| Installation::WindowsFolder(parent.to_path_buf()),
            &[
                ("Mangrove.exe", "old binary"),
                ("LICENSE.md", "old license"),
                ("notes.txt", "untouched"),
            ],
        );
        let installed = directory.path().join("installed");
        assert_eq!(read(&installed.join("Mangrove.exe")), "new binary");
        assert_eq!(read(&installed.join("LICENSE.md")), "new license");
        assert_eq!(read(&installed.join("notes.txt")), "untouched");
    }
}
