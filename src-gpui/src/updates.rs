//! Automatic updates, shared by every profile: a check at launch and every six
//! hours, the download and installation of a newer release, then a restart
//! into it. The settings and the version line show where the update stands.
use crate::preferences::{Preferences, save_json};
use gpui_kit::{App, Context, Task};
use mangrove_update::{
    Asset, Installation, NotInstallable, Platform, Progress, Release, Result, UpdateError, Version,
};
use serde::Serialize;
use serde_json::Value;
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

/// Leaves the launch to the repository first.
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(10);
const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const PROGRESS_REFRESH: Duration = Duration::from_millis(200);

/// Stored beside the preferences, outside the profiles: the application is
/// the same whichever profile is active.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct UpdateSettings {
    pub check_automatically: bool,
    pub install_automatically: bool,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self {
            check_automatically: true,
            install_automatically: true,
        }
    }
}

impl UpdateSettings {
    fn path() -> PathBuf {
        Preferences::path().with_file_name("updates.json")
    }

    fn load_from(path: &Path) -> Self {
        let value = fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok());
        let flag = |name| {
            value
                .as_ref()
                .and_then(|value| value.get(name))
                .and_then(Value::as_bool)
        };
        let defaults = Self::default();
        Self {
            check_automatically: flag("check_automatically")
                .unwrap_or(defaults.check_automatically),
            install_automatically: flag("install_automatically")
                .unwrap_or(defaults.install_automatically),
        }
    }
}

/// The network and file work of an update, on a background thread.
pub trait UpdateService: Send + Sync + 'static {
    fn latest(&self) -> Result<Release>;
    /// Downloads, verifies and installs `asset`, reporting through `job`.
    fn install(&self, asset: &Asset, installation: &Installation, job: &Job) -> Result<()>;
}

/// The releases published on GitHub.
struct GitHub;

impl UpdateService for GitHub {
    fn latest(&self) -> Result<Release> {
        mangrove_update::latest_release(Platform::current())
    }

    fn install(&self, asset: &Asset, installation: &Installation, job: &Job) -> Result<()> {
        let download = mangrove_update::download(asset, &job.progress)?;
        job.installing.store(true, Ordering::Relaxed);
        installation.install(download.archive())
    }
}

/// A download then installation, followed by the interface.
#[derive(Default)]
pub struct Job {
    pub progress: Progress,
    installing: AtomicBool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum UpdateStatus {
    /// Not checked since the launch.
    Idle,
    Checking,
    /// At the time of the check, in hours and minutes.
    UpToDate(String),
    Available(Release),
    Downloading {
        release: Release,
        received: u64,
        total: u64,
    },
    Installing(Release),
    /// Installed: the next launch, or a restart, opens it.
    Ready(Release),
    Failed {
        error: String,
        /// The release whose installation failed, if the check succeeded.
        release: Option<Release>,
    },
}

impl UpdateStatus {
    /// The newer release this status concerns, if any.
    pub fn release(&self) -> Option<&Release> {
        match self {
            Self::Available(release)
            | Self::Downloading { release, .. }
            | Self::Installing(release)
            | Self::Ready(release)
            | Self::Failed {
                release: Some(release),
                ..
            } => Some(release),
            _ => None,
        }
    }
}

/// What the version line shows: a newer version to restart into or to look at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateBadge {
    Ready(Version),
    Available(Version),
}

pub struct Updater {
    current: Version,
    settings: UpdateSettings,
    settings_path: PathBuf,
    installation: std::result::Result<Installation, NotInstallable>,
    service: Arc<dyn UpdateService>,
    status: UpdateStatus,
    /// Checks wait for their delay; off for tests and benchmarks.
    scheduling: bool,
    check_timer: Option<Task<()>>,
    work: Option<Task<()>>,
    progress_timer: Option<Task<()>>,
    job: Option<Arc<Job>>,
    /// The last failed installation and its error: later checks keep showing
    /// it and never retry it on their own.
    failed: Option<(Version, String)>,
}

impl Updater {
    pub fn new(scheduling: bool, cx: &mut Context<Self>) -> Self {
        let installation = Installation::detect();
        if let Ok(installation) = installation.clone() {
            cx.background_executor()
                .spawn(async move { installation.remove_leftovers() })
                .detach();
        }
        Self::with_service(
            Arc::new(GitHub),
            installation,
            UpdateSettings::path(),
            current_version(),
            scheduling,
            cx,
        )
    }

    fn with_service(
        service: Arc<dyn UpdateService>,
        installation: std::result::Result<Installation, NotInstallable>,
        settings_path: PathBuf,
        current: Version,
        scheduling: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            current,
            settings: UpdateSettings::load_from(&settings_path),
            settings_path,
            installation,
            service,
            status: UpdateStatus::Idle,
            scheduling,
            check_timer: None,
            work: None,
            progress_timer: None,
            job: None,
            failed: None,
        };
        this.schedule(FIRST_CHECK_DELAY, cx);
        this
    }

    pub fn current(&self) -> Version {
        self.current
    }

    pub fn settings(&self) -> UpdateSettings {
        self.settings
    }

    pub fn status(&self) -> &UpdateStatus {
        &self.status
    }

    /// Whether this application can replace itself with `release`.
    pub fn can_install(&self, release: &Release) -> bool {
        self.installation.is_ok() && release.asset.is_some()
    }

    /// Why this application never installs releases itself, if it does not.
    pub fn not_installable(&self) -> Option<NotInstallable> {
        self.installation.as_ref().err().copied()
    }

    pub fn badge(&self) -> Option<UpdateBadge> {
        match &self.status {
            UpdateStatus::Ready(release) => Some(UpdateBadge::Ready(release.version)),
            UpdateStatus::Available(release)
            | UpdateStatus::Failed {
                release: Some(release),
                ..
            } => Some(UpdateBadge::Available(release.version)),
            _ => None,
        }
    }

    /// A check, a download or an installation is under way.
    pub fn busy(&self) -> bool {
        matches!(
            self.status,
            UpdateStatus::Checking | UpdateStatus::Downloading { .. } | UpdateStatus::Installing(_)
        )
    }

    /// The release that can be installed now, if any.
    pub fn installable(&self) -> Option<&Release> {
        match &self.status {
            UpdateStatus::Available(release)
            | UpdateStatus::Failed {
                release: Some(release),
                ..
            } => Some(release).filter(|release| self.can_install(release)),
            _ => None,
        }
    }

    pub fn check(&mut self, cx: &mut Context<Self>) {
        if self.busy() || matches!(self.status, UpdateStatus::Ready(_)) {
            return;
        }
        self.check_timer = None;
        self.status = UpdateStatus::Checking;
        let service = self.service.clone();
        self.work = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { service.latest() })
                .await;
            let _ = this.update(cx, |this, cx| this.checked(result, cx));
        }));
        cx.notify();
    }

    fn checked(&mut self, result: Result<Release>, cx: &mut Context<Self>) {
        self.work = None;
        self.status = match result {
            Ok(release) if release.version > self.current => match &self.failed {
                Some((version, error)) if *version == release.version => UpdateStatus::Failed {
                    error: error.clone(),
                    release: Some(release),
                },
                _ => UpdateStatus::Available(release),
            },
            Ok(_) => UpdateStatus::UpToDate(chrono::Local::now().format("%H:%M").to_string()),
            Err(error) => UpdateStatus::Failed {
                error: error.to_string(),
                release: None,
            },
        };
        if self.settings.install_automatically && matches!(self.status, UpdateStatus::Available(_))
        {
            self.install(cx);
        }
        self.schedule(CHECK_INTERVAL, cx);
        cx.notify();
    }

    /// Downloads and installs the available release.
    pub fn install(&mut self, cx: &mut Context<Self>) {
        let Some(release) = self.installable().cloned() else {
            return;
        };
        let (Ok(installation), Some(asset)) = (self.installation.clone(), release.asset.clone())
        else {
            return;
        };
        let job = Arc::new(Job::default());
        self.job = Some(job.clone());
        self.status = UpdateStatus::Downloading {
            release: release.clone(),
            received: 0,
            total: asset.size,
        };
        let service = self.service.clone();
        self.work = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { service.install(&asset, &installation, &job) })
                .await;
            let _ = this.update(cx, |this, cx| this.installed(release, result, cx));
        }));
        self.progress_timer = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(PROGRESS_REFRESH).await;
                if this
                    .update(cx, |this, cx| this.refresh_progress(cx))
                    .is_err()
                {
                    break;
                }
            }
        }));
        cx.notify();
    }

    fn refresh_progress(&mut self, cx: &mut Context<Self>) {
        let Some(job) = &self.job else {
            return;
        };
        let refreshed = match &self.status {
            UpdateStatus::Downloading { release, .. } if job.installing.load(Ordering::Relaxed) => {
                // Nothing reports the installation's progress.
                self.progress_timer = None;
                UpdateStatus::Installing(release.clone())
            }
            UpdateStatus::Downloading { release, total, .. } => UpdateStatus::Downloading {
                release: release.clone(),
                received: job.progress.received(),
                // The announced size, until the archive starts to arrive.
                total: match job.progress.total() {
                    0 => *total,
                    started => started,
                },
            },
            _ => return,
        };
        if refreshed != self.status {
            self.status = refreshed;
            cx.notify();
        }
    }

    fn installed(&mut self, release: Release, result: Result<()>, cx: &mut Context<Self>) {
        self.work = None;
        self.progress_timer = None;
        self.job = None;
        self.status = match result {
            Ok(()) => {
                // The installed version replaces this one at the next launch.
                self.check_timer = None;
                self.failed = None;
                UpdateStatus::Ready(release)
            }
            Err(UpdateError::Cancelled) => UpdateStatus::Available(release),
            Err(error) => {
                let error = error.to_string();
                self.failed = Some((release.version, error.clone()));
                UpdateStatus::Failed {
                    error,
                    release: Some(release),
                }
            }
        };
        cx.notify();
    }

    /// Stops a download; an installation already started goes on.
    pub fn cancel(&mut self) {
        if let Some(job) = &self.job {
            job.progress.cancel();
        }
    }

    /// Quits and opens the installed version.
    pub fn restart(&self, cx: &mut App) {
        if let (UpdateStatus::Ready(_), Ok(installation)) = (&self.status, &self.installation) {
            // The running executable may have been moved aside by the installation.
            cx.set_restart_path(installation.launch_path());
            cx.restart();
        }
    }

    /// Records the setting, then checks now when it turns on.
    pub fn set_check_automatically(
        &mut self,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> io::Result<()> {
        self.settings.check_automatically = enabled;
        let saved = save_json(&self.settings, &self.settings_path);
        if enabled && self.scheduling {
            self.check(cx);
        } else {
            self.check_timer = None;
        }
        cx.notify();
        saved
    }

    /// Records the setting, then installs an available release when it turns on.
    pub fn set_install_automatically(
        &mut self,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> io::Result<()> {
        self.settings.install_automatically = enabled;
        let saved = save_json(&self.settings, &self.settings_path);
        if enabled && matches!(self.status, UpdateStatus::Available(_)) {
            self.install(cx);
        }
        cx.notify();
        saved
    }

    /// Benchmarks measure the application alone, without any check.
    #[cfg(feature = "app-benchmark")]
    pub fn suspend(&mut self) {
        self.scheduling = false;
        self.check_timer = None;
    }

    fn schedule(&mut self, delay: Duration, cx: &mut Context<Self>) {
        self.check_timer = None;
        if !self.scheduling || !self.settings.check_automatically {
            return;
        }
        self.check_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let _ = this.update(cx, |this, cx| this.check(cx));
        }));
    }
}

fn current_version() -> Version {
    env!("CARGO_PKG_VERSION")
        .parse()
        .expect("the package version has the X.Y.Z form of releases")
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{AppContext, Entity, TestAppContext};
    use mangrove_update::Url;
    use std::sync::Mutex;

    /// Answers checks with `latest` and installations with `install`.
    struct Fake {
        latest: Mutex<Result<Release>>,
        install: Mutex<Option<Result<()>>>,
        checks: Mutex<u32>,
        installs: Mutex<u32>,
    }

    impl Fake {
        fn new(latest: Result<Release>) -> Arc<Self> {
            Arc::new(Self {
                latest: Mutex::new(latest),
                install: Mutex::new(None),
                checks: Mutex::new(0),
                installs: Mutex::new(0),
            })
        }
        fn counts(&self) -> (u32, u32) {
            (*self.checks.lock().unwrap(), *self.installs.lock().unwrap())
        }
    }

    impl UpdateService for Fake {
        fn latest(&self) -> Result<Release> {
            *self.checks.lock().unwrap() += 1;
            match &*self.latest.lock().unwrap() {
                Ok(release) => Ok(release.clone()),
                Err(_) => Err(UpdateError::Network),
            }
        }
        fn install(&self, _: &Asset, _: &Installation, job: &Job) -> Result<()> {
            *self.installs.lock().unwrap() += 1;
            if job.progress.is_cancelled() {
                return Err(UpdateError::Cancelled);
            }
            self.install.lock().unwrap().take().unwrap_or(Ok(()))
        }
    }

    fn version(text: &str) -> Version {
        text.parse().unwrap()
    }

    fn release(number: &str, signed: bool) -> Release {
        let page = format!("https://github.com/Dpblandin/mangrove/releases/tag/v{number}");
        let archive = format!("mangrove-{number}-macos-arm64.dmg");
        Release {
            version: version(number),
            page: Url::parse(&page).unwrap(),
            asset: signed.then(|| Asset {
                url: Url::parse(&format!("{page}/{archive}")).unwrap(),
                signature: Url::parse(&format!("{page}/{archive}.minisig")).unwrap(),
                name: archive,
                size: 20,
            }),
        }
    }

    struct Setup {
        settings: UpdateSettings,
        installation: std::result::Result<Installation, NotInstallable>,
        scheduling: bool,
    }

    impl Default for Setup {
        fn default() -> Self {
            Self {
                settings: UpdateSettings::default(),
                installation: Ok(Installation::MacosBundle(
                    "/Applications/Mangrove.app".into(),
                )),
                scheduling: true,
            }
        }
    }

    fn updater(
        cx: &mut TestAppContext,
        service: Arc<Fake>,
        setup: Setup,
    ) -> (Entity<Updater>, tempfile::TempDir) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("updates.json");
        save_json(&setup.settings, &path).unwrap();
        let updater = cx.new(|cx| {
            Updater::with_service(
                service,
                setup.installation,
                path,
                version("0.3.0"),
                setup.scheduling,
                cx,
            )
        });
        (updater, directory)
    }

    fn status(cx: &mut TestAppContext, updater: &Entity<Updater>) -> UpdateStatus {
        updater.read_with(cx, |updater, _| updater.status().clone())
    }

    fn wait(cx: &mut TestAppContext, delay: Duration) {
        cx.executor().advance_clock(delay);
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    fn a_newer_release_installs_after_the_first_check_then_checks_stop(cx: &mut TestAppContext) {
        let service = Fake::new(Ok(release("0.4.0", true)));
        let (updater, _directory) = updater(cx, service.clone(), Setup::default());
        assert_eq!(status(cx, &updater), UpdateStatus::Idle);
        wait(cx, FIRST_CHECK_DELAY);
        assert_eq!(
            status(cx, &updater),
            UpdateStatus::Ready(release("0.4.0", true))
        );
        assert_eq!(
            updater.read_with(cx, |updater, _| updater.badge()),
            Some(UpdateBadge::Ready(version("0.4.0")))
        );
        wait(cx, CHECK_INTERVAL * 2);
        assert_eq!(service.counts(), (1, 1));
    }

    #[gpui_kit::test]
    fn without_automatic_installation_a_release_waits_for_the_user(cx: &mut TestAppContext) {
        let service = Fake::new(Ok(release("0.4.0", true)));
        let setup = Setup {
            settings: UpdateSettings {
                check_automatically: true,
                install_automatically: false,
            },
            ..Setup::default()
        };
        let (updater, _directory) = updater(cx, service.clone(), setup);
        wait(cx, FIRST_CHECK_DELAY);
        assert_eq!(
            status(cx, &updater),
            UpdateStatus::Available(release("0.4.0", true))
        );
        assert_eq!(
            updater.read_with(cx, |updater, _| updater.badge()),
            Some(UpdateBadge::Available(version("0.4.0")))
        );
        updater.update(cx, |updater, cx| updater.install(cx));
        assert!(matches!(
            status(cx, &updater),
            UpdateStatus::Downloading { total: 20, .. }
        ));
        cx.run_until_parked();
        assert_eq!(
            status(cx, &updater),
            UpdateStatus::Ready(release("0.4.0", true))
        );
        assert_eq!(service.counts(), (1, 1));
    }

    #[gpui_kit::test]
    fn an_up_to_date_application_checks_again_six_hours_later(cx: &mut TestAppContext) {
        let service = Fake::new(Ok(release("0.3.0", true)));
        let (updater, _directory) = updater(cx, service.clone(), Setup::default());
        wait(cx, FIRST_CHECK_DELAY);
        assert!(matches!(status(cx, &updater), UpdateStatus::UpToDate(_)));
        assert_eq!(updater.read_with(cx, |updater, _| updater.badge()), None);
        wait(cx, CHECK_INTERVAL - Duration::from_secs(1));
        assert_eq!(service.counts(), (1, 0));
        wait(cx, Duration::from_secs(1));
        assert_eq!(service.counts(), (2, 0));
    }

    #[gpui_kit::test]
    fn disabled_checks_wait_for_the_user_and_turn_on_with_a_check(cx: &mut TestAppContext) {
        let service = Fake::new(Ok(release("0.3.0", true)));
        let setup = Setup {
            settings: UpdateSettings {
                check_automatically: false,
                install_automatically: true,
            },
            ..Setup::default()
        };
        let (updater, directory) = updater(cx, service.clone(), setup);
        wait(cx, CHECK_INTERVAL);
        assert_eq!(service.counts(), (0, 0));
        updater.update(cx, |updater, cx| updater.check(cx));
        cx.run_until_parked();
        assert_eq!(service.counts(), (1, 0));
        updater
            .update(cx, |updater, cx| updater.set_check_automatically(true, cx))
            .unwrap();
        cx.run_until_parked();
        assert_eq!(service.counts(), (2, 0));
        assert_eq!(
            UpdateSettings::load_from(&directory.path().join("updates.json")),
            UpdateSettings::default()
        );
        updater
            .update(cx, |updater, cx| updater.set_check_automatically(false, cx))
            .unwrap();
        wait(cx, CHECK_INTERVAL * 2);
        assert_eq!(service.counts(), (2, 0));
    }

    #[gpui_kit::test]
    fn a_local_build_or_an_unsigned_release_is_only_announced(cx: &mut TestAppContext) {
        for (signed, installation) in [
            (true, Err(NotInstallable::LocalBuild)),
            (
                false,
                Ok(Installation::MacosBundle(
                    "/Applications/Mangrove.app".into(),
                )),
            ),
        ] {
            let service = Fake::new(Ok(release("0.4.0", signed)));
            let setup = Setup {
                installation,
                ..Setup::default()
            };
            let (updater, _directory) = updater(cx, service.clone(), setup);
            wait(cx, FIRST_CHECK_DELAY);
            assert_eq!(
                status(cx, &updater),
                UpdateStatus::Available(release("0.4.0", signed))
            );
            updater.update(cx, |updater, cx| updater.install(cx));
            cx.run_until_parked();
            assert_eq!(service.counts(), (1, 0));
            assert!(!updater.read_with(cx, |updater, _| {
                updater.can_install(&release("0.4.0", signed))
            }));
        }
    }

    #[gpui_kit::test]
    fn a_failed_installation_can_be_retried_and_a_cancelled_one_waits(cx: &mut TestAppContext) {
        let service = Fake::new(Ok(release("0.4.0", true)));
        *service.install.lock().unwrap() = Some(Err(UpdateError::PermissionDenied));
        let (updater, _directory) = updater(cx, service.clone(), Setup::default());
        wait(cx, FIRST_CHECK_DELAY);
        let UpdateStatus::Failed {
            error,
            release: failed,
        } = status(cx, &updater)
        else {
            panic!("the installation fails");
        };
        assert_eq!(error, UpdateError::PermissionDenied.to_string());
        assert_eq!(failed, Some(release("0.4.0", true)));
        assert_eq!(
            updater.read_with(cx, |updater, _| updater.badge()),
            Some(UpdateBadge::Available(version("0.4.0")))
        );
        updater.update(cx, |updater, cx| {
            updater.install(cx);
            updater.cancel();
        });
        cx.run_until_parked();
        assert_eq!(
            status(cx, &updater),
            UpdateStatus::Available(release("0.4.0", true))
        );
        updater.update(cx, |updater, cx| updater.install(cx));
        cx.run_until_parked();
        assert_eq!(
            status(cx, &updater),
            UpdateStatus::Ready(release("0.4.0", true))
        );
        assert_eq!(service.counts(), (1, 3));
    }

    #[gpui_kit::test]
    fn later_checks_keep_a_failed_installation_without_retrying_it(cx: &mut TestAppContext) {
        let service = Fake::new(Ok(release("0.4.0", true)));
        *service.install.lock().unwrap() = Some(Err(UpdateError::PermissionDenied));
        let (updater, _directory) = updater(cx, service.clone(), Setup::default());
        wait(cx, FIRST_CHECK_DELAY);
        let failed = status(cx, &updater);
        assert!(matches!(
            failed,
            UpdateStatus::Failed {
                release: Some(_),
                ..
            }
        ));
        wait(cx, CHECK_INTERVAL);
        assert_eq!(service.counts(), (2, 1));
        assert_eq!(status(cx, &updater), failed);
        // A newer release is installed automatically again.
        *service.latest.lock().unwrap() = Ok(release("0.4.1", true));
        wait(cx, CHECK_INTERVAL);
        assert_eq!(service.counts(), (3, 2));
        assert_eq!(
            status(cx, &updater),
            UpdateStatus::Ready(release("0.4.1", true))
        );
    }

    #[gpui_kit::test]
    fn turning_automatic_installation_on_installs_the_available_release(cx: &mut TestAppContext) {
        let service = Fake::new(Ok(release("0.4.0", true)));
        let setup = Setup {
            settings: UpdateSettings {
                check_automatically: true,
                install_automatically: false,
            },
            ..Setup::default()
        };
        let (updater, _directory) = updater(cx, service.clone(), setup);
        wait(cx, FIRST_CHECK_DELAY);
        updater
            .update(cx, |updater, cx| {
                updater.set_install_automatically(true, cx)
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(
            status(cx, &updater),
            UpdateStatus::Ready(release("0.4.0", true))
        );
    }

    #[gpui_kit::test]
    fn a_failed_check_reports_its_error_and_checks_again_later(cx: &mut TestAppContext) {
        let service = Fake::new(Err(UpdateError::Network));
        let (updater, _directory) = updater(cx, service.clone(), Setup::default());
        wait(cx, FIRST_CHECK_DELAY);
        assert_eq!(
            status(cx, &updater),
            UpdateStatus::Failed {
                error: UpdateError::Network.to_string(),
                release: None,
            }
        );
        assert_eq!(updater.read_with(cx, |updater, _| updater.badge()), None);
        wait(cx, CHECK_INTERVAL);
        assert_eq!(service.counts(), (2, 0));
    }

    #[test]
    fn settings_default_to_automatic_updates_and_keep_valid_values() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("updates.json");
        assert_eq!(UpdateSettings::load_from(&path), UpdateSettings::default());
        fs::write(&path, "damaged").unwrap();
        assert_eq!(UpdateSettings::load_from(&path), UpdateSettings::default());
        fs::write(
            &path,
            r#"{"check_automatically": false, "install_automatically": "no"}"#,
        )
        .unwrap();
        assert_eq!(
            UpdateSettings::load_from(&path),
            UpdateSettings {
                check_automatically: false,
                install_automatically: true,
            }
        );
        assert_eq!(current_version().to_string(), env!("CARGO_PKG_VERSION"));
    }
}
