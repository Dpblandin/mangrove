//! Repository opening may replace a native core session even when its UI result
//! is obsolete. Keep one physical read until completion and only the latest
//! queued intent, so a delayed Solo read cannot cancel the session shown next.
use crate::tabs::TabOpenAttempt;

pub struct OpenIntent {
    pub path: String,
    pub solo: Vec<String>,
    pub refresh: bool,
    pub generation: u64,
    pub attempt: TabOpenAttempt,
    pub navigate_to: Option<String>,
}

#[derive(Default)]
pub struct RepositoryOpenLane {
    active: Option<u64>,
    pending: Option<OpenIntent>,
}

impl RepositoryOpenLane {
    /// Returned intents may start a physical read; queued intents replace one
    /// another without dropping the task that owns the active native read.
    pub fn submit(&mut self, intent: OpenIntent) -> Option<OpenIntent> {
        if self.active.is_some() {
            self.pending = Some(intent);
            None
        } else {
            self.active = Some(intent.generation);
            Some(intent)
        }
    }

    /// Release only after the native read has really finished, including errors.
    pub fn finished(&mut self, generation: u64) -> Option<OpenIntent> {
        if self.active != Some(generation) {
            return None;
        }
        let next = self.pending.take();
        self.active = next.as_ref().map(|intent| intent.generation);
        next
    }

    /// Closing or leaving a tab cancels queued work. Its noninterruptible native
    /// read still owns the lane until `finished` receives its original ticket.
    pub fn invalidate_pending(&mut self) {
        self.pending = None;
    }
}

#[cfg(test)]
mod tests {
    use super::{OpenIntent, RepositoryOpenLane};
    use crate::tabs::Tabs;

    fn intent(tabs: &mut Tabs, generation: u64, path: &str, solo: &str) -> OpenIntent {
        OpenIntent {
            path: path.into(),
            solo: vec![solo.into()],
            refresh: false,
            generation,
            attempt: tabs.begin_open(path.into()),
            navigate_to: None,
        }
    }

    #[test]
    fn native_reads_run_a_then_c_and_only_c_can_publish_after_b_is_replaced() {
        let mut tabs = Tabs::default();
        let mut lane = RepositoryOpenLane::default();
        let mut a = intent(&mut tabs, 1, "/a", "a");
        a.navigate_to = Some("a-sha".into());
        let a = lane.submit(a).unwrap();
        assert!(lane.submit(intent(&mut tabs, 2, "/b", "b")).is_none());
        assert!(lane.submit(intent(&mut tabs, 3, "/c", "c")).is_none());

        let mut native_order = vec![a.path.clone()];
        assert!(!tabs.is_current(&a.attempt));
        let c = lane.finished(a.generation).unwrap();
        assert!(!tabs.complete_open(a.attempt, a.path, "obsolete".into()));
        native_order.push(c.path.clone());
        assert_eq!(c.path, "/c");
        assert_eq!(c.solo, ["c"]);
        assert!(c.navigate_to.is_none());
        assert!(tabs.is_current(&c.attempt));
        assert!(lane.finished(c.generation).is_none());
        assert!(tabs.complete_open(c.attempt, c.path.clone(), "C".into()));
        assert_eq!(native_order, ["/a", "/c"]);
        assert_eq!(tabs.active_tab().unwrap().path.as_deref(), Some("/c"));
        assert_eq!(tabs.active_tab().unwrap().name, "C");
    }

    #[test]
    fn latest_solo_and_refresh_flags_belong_to_the_queued_attempt() {
        let mut tabs = Tabs::default();
        let mut lane = RepositoryOpenLane::default();
        let active = lane
            .submit(intent(&mut tabs, 1, "/repo", "original"))
            .unwrap();
        let mut intermediate = intent(&mut tabs, 2, "/repo", "intermediate");
        intermediate.refresh = true;
        intermediate.navigate_to = Some("obsolete-solo-sha".into());
        assert!(lane.submit(intermediate).is_none());
        let mut latest = intent(&mut tabs, 3, "/repo", "latest");
        latest.refresh = true;
        assert!(lane.submit(latest).is_none());
        let latest = lane.finished(active.generation).unwrap();
        assert_eq!(latest.solo, ["latest"]);
        assert!(latest.refresh);
        assert!(latest.navigate_to.is_none());
        assert_eq!(latest.generation, 3);
        assert!(tabs.is_current(&latest.attempt));
        assert!(!tabs.is_current(&active.attempt));
    }

    #[test]
    fn close_and_new_tab_cancel_pending_but_keep_the_native_read_until_completion() {
        let mut tabs = Tabs::default();
        let mut lane = RepositoryOpenLane::default();
        let active = lane.submit(intent(&mut tabs, 1, "/a", "a")).unwrap();
        assert!(lane.submit(intent(&mut tabs, 2, "/b", "b")).is_none());
        tabs.close(tabs.active.unwrap());
        lane.invalidate_pending();
        tabs.create();
        assert!(!tabs.is_current(&active.attempt));
        assert!(lane.submit(intent(&mut tabs, 4, "/d", "d")).is_none());
        let next = lane.finished(active.generation).unwrap();
        assert_eq!(next.path, "/d");
        assert!(tabs.is_current(&next.attempt));
        assert!(lane.finished(next.generation).is_none());
    }

    #[test]
    fn clearing_without_replacement_cannot_release_a_running_read() {
        let mut tabs = Tabs::default();
        let mut lane = RepositoryOpenLane::default();
        let active = lane.submit(intent(&mut tabs, 1, "/a", "a")).unwrap();
        assert!(lane.submit(intent(&mut tabs, 2, "/b", "b")).is_none());
        lane.invalidate_pending();
        tabs.create();
        assert!(lane.finished(active.generation).is_none());
        assert!(tabs.active_tab().unwrap().path.is_none());
        assert!(lane.submit(intent(&mut tabs, 4, "/d", "d")).is_some());
    }

    #[test]
    fn a_late_completion_cannot_release_a_different_physical_read() {
        let mut tabs = Tabs::default();
        let mut lane = RepositoryOpenLane::default();
        let a = lane.submit(intent(&mut tabs, 1, "/a", "a")).unwrap();
        assert!(lane.submit(intent(&mut tabs, 2, "/b", "b")).is_none());
        let b = lane.finished(a.generation).unwrap();
        assert!(lane.submit(intent(&mut tabs, 3, "/c", "c")).is_none());
        assert!(lane.finished(a.generation).is_none());
        let c = lane.finished(b.generation).unwrap();
        assert_eq!(c.path, "/c");
    }
}
