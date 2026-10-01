//! Opt-in, bounded, in-application benchmark. No repository content is exported.
//!
//! The host feeds readiness from its real views and executes the returned actions
//! through the same handlers as user interactions. `tick` must run on the window's
//! next-frame callback during workloads, and on a quiet timer during idle.

use gpui_kit::profiler::{self, FrameEvent, FrameTimingCollector};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

const SCROLL_SECONDS: f32 = 3.0;
const SCROLL_SPEED: f32 = 2_400.0;
const MAX_FRAME_SAMPLES: usize = 2_000;
const BURST_SELECTIONS: usize = 60;
const BURST_INTERVAL: Duration = Duration::from_millis(33);

#[derive(Default, Serialize)]
struct Operations {
    timings_ms: BTreeMap<&'static str, Vec<f64>>,
    counts: BTreeMap<&'static str, usize>,
    counts_by_phase: BTreeMap<&'static str, BTreeMap<&'static str, usize>>,
    #[serde(skip)]
    phase: &'static str,
}

fn operations() -> &'static Mutex<Operations> {
    static OPERATIONS: OnceLock<Mutex<Operations>> = OnceLock::new();
    OPERATIONS.get_or_init(|| Mutex::new(Operations::default()))
}

pub fn record_operation(name: &'static str, duration: Duration) {
    let mut operations = operations()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let samples = operations.timings_ms.entry(name).or_default();
    if samples.len() < MAX_FRAME_SAMPLES {
        samples.push(duration.as_secs_f64() * 1_000.0);
    }
}

pub fn record_count(name: &'static str) {
    let mut operations = operations()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    *operations.counts.entry(name).or_default() += 1;
    let phase = operations.phase;
    *operations
        .counts_by_phase
        .entry(phase)
        .or_default()
        .entry(name)
        .or_default() += 1;
}

/// An opt-in benchmark cannot fall back to the user's repository or cache.
pub struct Configuration {
    pub output: PathBuf,
    pub repository: String,
    pub cache: PathBuf,
    pub syntax_files: [String; 2],
    pub heavy_file: String,
    pub scroll_files: bool,
}

impl Configuration {
    pub fn from_env() -> Result<Option<Self>, String> {
        let output = std::env::var_os("MANGROVE_BENCH_OUTPUT").map(PathBuf::from);
        let Some(mut configuration) = Self::from_paths(
            output,
            std::env::var_os("MANGROVE_BENCH_REPO").map(PathBuf::from),
            std::env::var_os("MANGROVE_BENCH_CACHE").map(PathBuf::from),
        )?
        else {
            return Ok(None);
        };
        for (slot, path) in configuration.syntax_files.iter_mut().enumerate() {
            let name = format!("MANGROVE_BENCH_SYNTAX_{slot}");
            Self::file_override(&name, path)?;
        }
        Self::file_override("MANGROVE_BENCH_HEAVY", &mut configuration.heavy_file)?;
        configuration.scroll_files = match std::env::var("MANGROVE_BENCH_SCROLL_TARGET") {
            Ok(value) if value == "unstaged-files" => true,
            Ok(value) if value == "diff" => false,
            Err(std::env::VarError::NotPresent) => false,
            _ => return Err("MANGROVE_BENCH_SCROLL_TARGET must be diff or unstaged-files".into()),
        };
        Ok(Some(configuration))
    }

    fn file_override(name: &str, path: &mut String) -> Result<(), String> {
        match std::env::var(name) {
            Ok(value) if !value.is_empty() => *path = value,
            Err(std::env::VarError::NotPresent) => {}
            _ => return Err(format!("{name} must be a nonempty UTF-8 file path")),
        }
        Ok(())
    }

    fn from_paths(
        output: Option<PathBuf>,
        repository: Option<PathBuf>,
        cache: Option<PathBuf>,
    ) -> Result<Option<Self>, String> {
        // Stray benchmark variables do not opt a normal launch into benchmarking.
        if output.is_none() {
            return Ok(None);
        }
        let output = Self::absolute_path("MANGROVE_BENCH_OUTPUT", output)?;
        let repository = Self::absolute_path("MANGROVE_BENCH_REPO", repository)?;
        let cache = Self::absolute_path("MANGROVE_BENCH_CACHE", cache)?;
        let repository = repository
            .into_os_string()
            .into_string()
            .map_err(|_| "MANGROVE_BENCH_REPO must be a UTF-8 path".to_string())?;
        Ok(Some(Self {
            output,
            repository,
            cache,
            syntax_files: [
                "composables/useDiff.ts".into(),
                "stores/repositories.ts".into(),
            ],
            heavy_file: "generated/catalog.ts".into(),
            scroll_files: false,
        }))
    }

    fn absolute_path(name: &str, path: Option<PathBuf>) -> Result<PathBuf, String> {
        let path = path.ok_or_else(|| format!("{name} is required for benchmarking"))?;
        if !path.is_absolute() {
            return Err(format!("{name} must be an explicit absolute path"));
        }
        Ok(path)
    }
}

#[derive(Default, Clone, Copy)]
pub struct PerfSnapshot {
    pub history_ready: bool,
    pub index_complete: bool,
    pub details_ready: bool,
    pub diff_ready: bool,
    pub syntax_ready: bool,
    pub coloring: bool,
    pub selected_index: usize,
    /// 0 or 1 for the two small syntax targets; `None` for another file.
    pub syntax_target: Option<usize>,
    pub heavy_diff: bool,
    pub file_count: usize,
    pub history_total: usize,
    pub diff_rows: usize,
    pub working_ready: bool,
    pub working_files: usize,
    pub working_displayed: usize,
    pub file_scroll_offset: f32,
}

#[derive(Debug, Clone, Copy)]
pub enum PerfAction {
    SelectCommit(usize),
    SelectCommitKeyboard(usize),
    OpenSyntaxFile(usize),
    OpenHeavyDiff,
    CloseDiff,
    ScrollHistory(f32),
    ScrollDiff(f32),
    SelectWorking,
    OpenWorkingFile,
    ScrollFiles(f32),
    Finish,
}

#[derive(Serialize, Default)]
struct Frames {
    interval_ms: Vec<f64>,
    draw_ms: Vec<f64>,
    submit_ms: Vec<f64>,
}

#[derive(Serialize)]
struct Report {
    schema_version: u32,
    implementation: &'static str,
    readiness_proxy: &'static str,
    frame_proxy: &'static str,
    status: &'static str,
    timings_ms: BTreeMap<String, Vec<f64>>,
    frames: BTreeMap<String, Frames>,
    operations: Operations,
    workload: Workload,
    elapsed_ms: f64,
    launch_overhead_ms: f64,
    failure: Option<String>,
}

#[derive(Serialize, Default)]
struct Workload {
    accessibility_active: bool,
    history_total: usize,
    changed_files: usize,
    heavy_diff_rows: usize,
    heavy_diff_colored: usize,
    selection_samples: usize,
    burst_selections: usize,
    syntax_samples: usize,
    scroll_duration_ms: usize,
    scroll_speed_px_s: usize,
    viewport_width: usize,
    viewport_height: usize,
    working_files: usize,
    working_displayed: usize,
    file_scroll_distance_px: f32,
    inactive_scroll_ticks: usize,
    hidden_scroll_ticks: usize,
}

#[derive(Clone, Copy, PartialEq)]
enum Stage {
    Startup,
    Index,
    Select,
    Burst,
    BurstRestore,
    Syntax,
    Heavy,
    DiffScroll,
    Working,
    WorkingDiff,
    FileScroll,
    Close,
    HistoryScroll,
    Idle,
    Done,
}

pub struct PerfSession {
    scroll_files: bool,
    syntax_files: [String; 2],
    heavy_file: String,
    output: PathBuf,
    phase_path: PathBuf,
    started: Instant,
    stage_started: Instant,
    stage: Stage,
    sample: usize,
    samples: usize,
    action_sent: bool,
    syntax_raw_ready: bool,
    burst_last_selection: Option<Instant>,
    ready_submission: Option<u64>,
    collector: FrameTimingCollector,
    submissions: u64,
    last_submission_ms: Option<f64>,
    report: Report,
}

impl PerfSession {
    pub fn new(configuration: Configuration, started: Instant) -> Result<Self, String> {
        let output = configuration.output;
        let samples = std::env::var("MANGROVE_BENCH_SAMPLES")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(30)
            .clamp(2, 100)
            / 2
            * 2;
        let launch_overhead_ms = std::env::var("MANGROVE_BENCH_LAUNCH_UNIX_MS")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .map(|launch| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs_f64()
                    * 1_000.0
                    - launch
                    - started.elapsed().as_secs_f64() * 1_000.0
            })
            .unwrap_or_default()
            .max(0.0);
        // Enable only in benchmark runs. Profiler task locations are never exported.
        profiler::set_trace_enabled(true);
        *operations()
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Operations::default();
        let phase_path = output.with_extension("phase");
        let mut session = Self {
            scroll_files: configuration.scroll_files,
            syntax_files: configuration.syntax_files,
            heavy_file: configuration.heavy_file,
            output,
            phase_path,
            started,
            stage_started: started,
            stage: Stage::Startup,
            sample: 0,
            samples,
            action_sent: false,
            syntax_raw_ready: false,
            burst_last_selection: None,
            ready_submission: None,
            collector: FrameTimingCollector::new(),
            submissions: 0,
            last_submission_ms: None,
            report: Report {
                schema_version: 1,
                implementation: "gpui",
                readiness_proxy: "state-ready observed at frame cadence then subsequent GPUI platform submission",
                frame_proxy: "GPUI platform submission intervals (not display scanout)",
                status: "ok",
                timings_ms: BTreeMap::new(),
                frames: BTreeMap::new(),
                operations: Operations::default(),
                workload: Workload {
                    scroll_duration_ms: 3_000,
                    scroll_speed_px_s: 2_400,
                    viewport_width: 1_440,
                    viewport_height: 888,
                    ..Default::default()
                },
                elapsed_ms: 0.0,
                launch_overhead_ms,
                failure: None,
            },
        };
        session.phase("startup")?;
        Ok(session)
    }

    fn phase(&mut self, name: &'static str) -> Result<(), String> {
        operations()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .phase = name;
        std::fs::write(&self.phase_path, name).map_err(|e| e.to_string())
    }

    fn transition(&mut self, stage: Stage, phase: &'static str) {
        self.stage = stage;
        self.stage_started = Instant::now();
        self.sample = 0;
        self.action_sent = false;
        self.burst_last_selection = None;
        self.ready_submission = None;
        self.last_submission_ms = None;
        if let Err(error) = self.phase(phase) {
            self.fail(error);
        }
    }

    fn timing(&mut self, name: &str, milliseconds: f64) {
        let offset = if name.starts_with("startup_") {
            self.report.launch_overhead_ms
        } else {
            0.0
        };
        self.report
            .timings_ms
            .entry(name.into())
            .or_default()
            .push(milliseconds + offset);
    }

    fn collect_frames(&mut self) {
        let phase = match self.stage {
            Stage::DiffScroll => Some("diff_scroll"),
            Stage::HistoryScroll => Some("history_scroll"),
            Stage::FileScroll => Some("file_list_scroll"),
            Stage::Select => Some("selection"),
            Stage::Burst => Some("selection_burst"),
            Stage::Syntax => Some("syntax"),
            _ => None,
        };
        for event in self.collector.collect_unseen() {
            match event {
                FrameEvent::Present(event) => {
                    self.submissions += 1;
                    if let Some(phase) = phase {
                        let now_ms = self.started.elapsed().as_secs_f64() * 1_000.0;
                        let submit_ms =
                            now_ms - event.present_end.elapsed().as_secs_f64() * 1_000.0;
                        let frames = self.report.frames.entry(phase.into()).or_default();
                        if let Some(previous) = self.last_submission_ms
                            && frames.interval_ms.len() < MAX_FRAME_SAMPLES
                        {
                            frames.interval_ms.push(submit_ms - previous);
                        }
                        self.last_submission_ms = Some(submit_ms);
                        if frames.submit_ms.len() < MAX_FRAME_SAMPLES {
                            frames
                                .submit_ms
                                .push(event.present_duration().as_secs_f64() * 1_000.0);
                        }
                    }
                }
                FrameEvent::Draw(event) => {
                    if let Some(phase) = phase {
                        let frames = self.report.frames.entry(phase.into()).or_default();
                        if frames.draw_ms.len() < MAX_FRAME_SAMPLES {
                            frames
                                .draw_ms
                                .push(event.draw_duration().as_secs_f64() * 1_000.0);
                        }
                    }
                }
            }
        }
    }

    /// Complete only after a frame containing the newly ready state was submitted.
    /// A callback at the beginning of the frame is not counted as a completed draw.
    fn submitted_ready(&mut self, ready: bool, metric: &str) -> bool {
        if !ready {
            self.ready_submission = None;
            return false;
        }
        match self.ready_submission {
            Some(submission) => return self.submissions > submission,
            None => {
                self.timing(
                    &format!("{metric}_cpu_ready"),
                    self.stage_started.elapsed().as_secs_f64() * 1_000.0,
                );
                self.ready_submission = Some(self.submissions);
            }
        }
        false
    }

    /// Quiet polling for idle/index; frame-paced while measuring visual changes.
    pub fn wake_after(&self) -> Duration {
        if self.stage == Stage::Burst && self.sample < BURST_SELECTIONS {
            burst_wait(self.burst_last_selection, Instant::now())
        } else if matches!(self.stage, Stage::Idle | Stage::Index) {
            Duration::from_millis(100)
        } else {
            Duration::ZERO
        }
    }

    /// Record the actual content viewport; caller supplies `window.viewport_size()`.
    pub fn set_viewport(&mut self, width: usize, height: usize) {
        self.report.workload.viewport_width = width;
        self.report.workload.viewport_height = height;
    }

    pub fn set_accessibility_active(&mut self, active: bool) {
        self.report.workload.accessibility_active |= active;
    }

    pub fn set_scroll_window_state(&mut self, active: bool, visible: bool) {
        if self.stage == Stage::FileScroll {
            self.report.workload.inactive_scroll_ticks += usize::from(!active);
            self.report.workload.hidden_scroll_ticks += usize::from(!visible);
        }
    }

    pub fn syntax_target(&self, path: &str) -> Option<usize> {
        self.syntax_files
            .iter()
            .position(|file| path.ends_with(file))
    }

    pub fn syntax_file(&self, slot: usize) -> &str {
        &self.syntax_files[slot]
    }

    pub fn heavy_file(&self) -> &str {
        &self.heavy_file
    }

    pub fn tick(&mut self, snapshot: PerfSnapshot) -> Option<PerfAction> {
        self.collect_frames();
        if self.started.elapsed() > Duration::from_secs(120) && self.stage != Stage::Done {
            self.fail("Timed out waiting for a real application view".into());
            return Some(PerfAction::Finish);
        }
        let stage = self.stage;
        match stage {
            Stage::Startup
                if self.submitted_ready(
                    snapshot.history_ready
                        && snapshot.details_ready
                        && self.report.workload.viewport_width == 1_440
                        && self.report.workload.viewport_height == 888,
                    "startup_history",
                ) =>
            {
                self.timing(
                    "startup_history",
                    self.started.elapsed().as_secs_f64() * 1_000.0,
                );
                self.report.workload.history_total = snapshot.history_total;
                self.transition(Stage::Index, "index");
            }
            Stage::Index if snapshot.index_complete => {
                self.report.workload.history_total = snapshot.history_total;
                if self.scroll_files {
                    self.transition(Stage::Working, "working_files");
                } else {
                    self.transition(Stage::Select, "selection");
                }
            }
            Stage::Working => {
                if !self.action_sent {
                    self.action_sent = true;
                    return Some(PerfAction::SelectWorking);
                }
                if snapshot.working_ready && snapshot.working_displayed > 0 {
                    self.report.workload.working_files = snapshot.working_files;
                    self.report.workload.working_displayed = snapshot.working_displayed;
                    self.transition(Stage::WorkingDiff, "working_diff");
                    return Some(PerfAction::OpenWorkingFile);
                }
            }
            Stage::WorkingDiff
                if self.submitted_ready(
                    snapshot.working_ready && snapshot.diff_ready && snapshot.heavy_diff,
                    "open_working_diff",
                ) =>
            {
                self.report.workload.heavy_diff_rows = snapshot.diff_rows;
                self.report.workload.heavy_diff_colored = usize::from(snapshot.syntax_ready);
                self.transition(Stage::FileScroll, "file_list_scroll");
                return Some(PerfAction::ScrollFiles(0.));
            }
            Stage::FileScroll => {
                self.report.workload.file_scroll_distance_px = snapshot.file_scroll_offset;
                let seconds = self.stage_started.elapsed().as_secs_f32();
                if seconds >= SCROLL_SECONDS {
                    self.transition(Stage::Idle, "idle");
                } else {
                    return Some(PerfAction::ScrollFiles(seconds * SCROLL_SPEED));
                }
            }
            Stage::Select => {
                let target = if self.sample.is_multiple_of(2) { 1 } else { 0 };
                if !self.action_sent {
                    self.stage_started = Instant::now();
                    self.action_sent = true;
                    return Some(PerfAction::SelectCommit(target));
                }
                if self.submitted_ready(
                    snapshot.details_ready && snapshot.selected_index == target,
                    "select_details",
                ) {
                    self.timing(
                        "select_details",
                        self.stage_started.elapsed().as_secs_f64() * 1_000.0,
                    );
                    self.sample += 1;
                    self.action_sent = false;
                    self.ready_submission = None;
                    if self.sample >= self.samples {
                        self.report.workload.selection_samples = self.sample;
                        self.report.workload.changed_files = snapshot.file_count;
                        self.transition(Stage::Burst, "selection_burst");
                    }
                }
            }
            Stage::Burst => {
                if self.sample < BURST_SELECTIONS {
                    let now = Instant::now();
                    if burst_wait(self.burst_last_selection, now).is_zero() {
                        if let Some(previous) = self.burst_last_selection {
                            self.timing(
                                "burst_inter_selection",
                                now.duration_since(previous).as_secs_f64() * 1_000.0,
                            );
                        }
                        self.burst_last_selection = Some(now);
                        let target = burst_target(self.sample, snapshot.history_total);
                        self.sample += 1;
                        if self.sample == BURST_SELECTIONS {
                            self.stage_started = now;
                        }
                        return Some(PerfAction::SelectCommitKeyboard(target));
                    }
                } else {
                    let target = burst_target(BURST_SELECTIONS - 1, snapshot.history_total);
                    if self.submitted_ready(
                        snapshot.details_ready && snapshot.selected_index == target,
                        "burst_last_details",
                    ) {
                        self.timing(
                            "burst_last_details",
                            self.stage_started.elapsed().as_secs_f64() * 1_000.0,
                        );
                        self.report.workload.burst_selections = self.sample;
                        self.transition(Stage::BurstRestore, "selection_restore");
                        return Some(PerfAction::SelectCommit(0));
                    }
                }
            }
            Stage::BurstRestore if snapshot.details_ready && snapshot.selected_index == 0 => {
                self.transition(Stage::Syntax, "syntax");
            }
            Stage::Syntax => {
                let target = self.sample % 2;
                if !self.action_sent {
                    self.stage_started = Instant::now();
                    self.action_sent = true;
                    self.syntax_raw_ready = false;
                    return Some(PerfAction::OpenSyntaxFile(target));
                }
                if !self.syntax_raw_ready
                    && snapshot.diff_ready
                    && snapshot.syntax_target == Some(target)
                {
                    self.syntax_raw_ready = true;
                    self.timing(
                        "select_raw_diff_cpu_ready",
                        self.stage_started.elapsed().as_secs_f64() * 1_000.0,
                    );
                }
                if self.submitted_ready(
                    snapshot.diff_ready
                        && snapshot.syntax_ready
                        && !snapshot.coloring
                        && snapshot.syntax_target == Some(target),
                    "select_colored_diff",
                ) {
                    self.timing(
                        "select_colored_diff",
                        self.stage_started.elapsed().as_secs_f64() * 1_000.0,
                    );
                    self.sample += 1;
                    self.action_sent = false;
                    self.ready_submission = None;
                    if self.sample >= self.samples {
                        self.report.workload.syntax_samples = self.sample;
                        self.transition(Stage::Heavy, "heavy_diff");
                    }
                }
            }
            Stage::Heavy => {
                if !self.action_sent {
                    self.action_sent = true;
                    self.syntax_raw_ready = false;
                    return Some(PerfAction::OpenHeavyDiff);
                }
                if !self.syntax_raw_ready && snapshot.diff_ready && snapshot.heavy_diff {
                    self.syntax_raw_ready = true;
                    self.timing(
                        "open_heavy_raw_cpu_ready",
                        self.stage_started.elapsed().as_secs_f64() * 1_000.0,
                    );
                }
                if self.submitted_ready(
                    snapshot.diff_ready && snapshot.heavy_diff && !snapshot.coloring,
                    "open_heavy_diff",
                ) {
                    self.timing(
                        "open_heavy_diff",
                        self.stage_started.elapsed().as_secs_f64() * 1_000.0,
                    );
                    self.report.workload.heavy_diff_rows = snapshot.diff_rows;
                    self.report.workload.heavy_diff_colored = usize::from(snapshot.syntax_ready);
                    self.transition(Stage::DiffScroll, "diff_scroll");
                }
            }
            Stage::DiffScroll => {
                let seconds = self.stage_started.elapsed().as_secs_f32();
                if seconds >= SCROLL_SECONDS {
                    self.transition(Stage::Close, "close_diff");
                } else {
                    return Some(PerfAction::ScrollDiff(seconds * SCROLL_SPEED));
                }
            }
            Stage::Close => {
                if !self.action_sent {
                    self.action_sent = true;
                    return Some(PerfAction::CloseDiff);
                }
                if snapshot.history_ready {
                    self.transition(Stage::HistoryScroll, "history_scroll");
                    return Some(PerfAction::ScrollHistory(0.0));
                }
            }
            Stage::HistoryScroll => {
                let seconds = self.stage_started.elapsed().as_secs_f32();
                if seconds >= SCROLL_SECONDS {
                    self.transition(Stage::Idle, "idle");
                } else {
                    return Some(PerfAction::ScrollHistory(seconds * SCROLL_SPEED));
                }
            }
            Stage::Idle if self.stage_started.elapsed() >= Duration::from_secs(2) => {
                self.finish();
                return Some(PerfAction::Finish);
            }
            Stage::Done => return Some(PerfAction::Finish),
            _ => {}
        }
        None
    }

    pub fn fail(&mut self, _message: String) {
        self.report.status = "failed";
        // This accepts fixed diagnostic text, never an engine error containing a path.
        self.report.failure =
            Some("The application did not complete the required visual workload".into());
        self.finish();
    }

    fn finish(&mut self) {
        self.stage = Stage::Done;
        self.report.elapsed_ms =
            self.started.elapsed().as_secs_f64() * 1_000.0 + self.report.launch_overhead_ms;
        self.report.operations = std::mem::take(
            &mut *operations()
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        );
        let temporary = self.output.with_extension("tmp");
        if let Ok(json) = serde_json::to_vec_pretty(&self.report)
            && std::fs::write(&temporary, json).is_ok()
        {
            let _ = std::fs::rename(&temporary, &self.output);
        }
        let _ = self.phase("done");
        profiler::set_trace_enabled(false);
    }
}

fn burst_target(sample: usize, total: usize) -> usize {
    (sample + 1).min(total.saturating_sub(1))
}

// Schedule from the last dispatched action. After a slow render/Git step, an
// absolute schedule would catch up by sending multiple keys less than 33 ms apart.
fn burst_wait(previous: Option<Instant>, now: Instant) -> Duration {
    previous.map_or(Duration::ZERO, |previous| {
        BURST_INTERVAL.saturating_sub(now.duration_since(previous))
    })
}

#[cfg(test)]
mod configuration_tests {
    use super::Configuration;
    use std::path::PathBuf;

    #[test]
    fn burst_pacing_waits_from_the_actual_last_key_without_catching_up() {
        use std::time::{Duration, Instant};
        let start = Instant::now();
        assert_eq!(super::burst_wait(None, start), Duration::ZERO);
        assert_eq!(
            super::burst_wait(Some(start), start + Duration::from_millis(32)),
            Duration::from_millis(1)
        );
        assert_eq!(
            super::burst_wait(Some(start), start + Duration::from_millis(33)),
            Duration::ZERO
        );
        let delayed = start + Duration::from_millis(200);
        assert_eq!(super::burst_wait(Some(start), delayed), Duration::ZERO);
        assert_eq!(
            super::burst_wait(Some(delayed), delayed),
            Duration::from_millis(33)
        );
    }

    fn absolute(name: &str) -> PathBuf {
        PathBuf::from(if cfg!(windows) { "C:\\" } else { "/" }).join(name)
    }

    fn valid_paths() -> [Option<PathBuf>; 3] {
        [
            Some(absolute("benchmark/report.json")),
            Some(absolute("benchmark/repository")),
            Some(absolute("benchmark/cache")),
        ]
    }

    fn configuration(paths: [Option<PathBuf>; 3]) -> Result<Option<Configuration>, String> {
        let [output, repository, cache] = paths;
        Configuration::from_paths(output, repository, cache)
    }

    #[test]
    fn no_output_ignores_other_benchmark_paths() {
        assert!(
            configuration([None, Some(PathBuf::from("relative-repository")), None])
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn explicit_absolute_paths_are_preserved() {
        let config = configuration(valid_paths()).unwrap().unwrap();
        assert_eq!(config.output, absolute("benchmark/report.json"));
        assert_eq!(
            config.repository,
            absolute("benchmark/repository").to_str().unwrap()
        );
        assert_eq!(config.cache, absolute("benchmark/cache"));
    }

    #[test]
    fn opt_in_requires_both_repository_and_cache() {
        for (index, name) in [(1, "MANGROVE_BENCH_REPO"), (2, "MANGROVE_BENCH_CACHE")] {
            let mut paths = valid_paths();
            paths[index] = None;
            let error = configuration(paths).err().expect("missing path accepted");
            assert!(error.contains(name), "{error}");
        }
    }

    #[test]
    fn opt_in_rejects_relative_and_empty_paths() {
        for (index, name) in [
            (0, "MANGROVE_BENCH_OUTPUT"),
            (1, "MANGROVE_BENCH_REPO"),
            (2, "MANGROVE_BENCH_CACHE"),
        ] {
            for invalid in ["relative/path", ""] {
                let mut paths = valid_paths();
                paths[index] = Some(PathBuf::from(invalid));
                let error = configuration(paths).err().expect("relative path accepted");
                assert!(error.contains(name), "{error}");
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn repository_path_must_be_representable_by_core() {
        use std::{ffi::OsString, os::unix::ffi::OsStringExt};
        let mut paths = valid_paths();
        paths[1] = Some(PathBuf::from(OsString::from_vec(
            b"/benchmark/\xff".to_vec(),
        )));
        let error = configuration(paths)
            .err()
            .expect("non-UTF-8 repository accepted");
        assert!(error.contains("MANGROVE_BENCH_REPO"));
    }
}
