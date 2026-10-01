//! Lazy repository context for the native sidebar. Each bounded core read runs
//! off the UI thread. Each list retains its physical read until completion,
//! coalesces refreshes into one pending intent, and accepts only current results.
use gpui_kit::{Context, Task};
use mangrove_core::model::{StashList, WorktreeList};
use std::sync::Arc;

use crate::sidebar::{SidebarEvent, SidebarView};

#[derive(Clone, Debug, PartialEq, Eq)]
struct ContextRequest {
    repository_id: String,
    generation: u64,
}

pub(crate) struct ContextList<T> {
    pub data: Option<Arc<T>>,
    pub loading: bool,
    pub error: Option<String>,
    task: Option<Task<()>>,
    active: Option<ContextRequest>,
    pending: Option<ContextRequest>,
}

impl<T> Default for ContextList<T> {
    fn default() -> Self {
        Self {
            data: None,
            loading: false,
            error: None,
            task: None,
            active: None,
            pending: None,
        }
    }
}

impl<T> ContextList<T> {
    fn invalidate(&mut self, clear: bool) {
        // The underlying Git read cannot be interrupted. Keep its task and lane
        // until completion rather than allowing a new generation to overlap it.
        self.pending = None;
        self.loading = false;
        self.error = None;
        if clear {
            self.data = None;
        }
    }

    fn request(&mut self, request: ContextRequest) {
        if self.active.as_ref() == Some(&request) || self.pending.as_ref() == Some(&request) {
            return;
        }
        self.pending = Some(request);
        self.loading = true;
        self.error = None;
    }

    fn acquire(&mut self) -> Option<ContextRequest> {
        if self.active.is_some() {
            return None;
        }
        let request = self.pending.take()?;
        self.active = Some(request.clone());
        Some(request)
    }

    fn is_current(&self, request: &ContextRequest, generation: u64) -> bool {
        self.active.as_ref() == Some(request)
            && self.pending.is_none()
            && request.generation == generation
    }

    fn complete(
        &mut self,
        request: &ContextRequest,
        generation: u64,
        result: Result<T, String>,
    ) -> bool {
        if self.active.as_ref() != Some(request) {
            return false;
        }
        let current = self.is_current(request, generation);
        self.active = None;
        self.task = None;
        if !current {
            return false;
        }
        self.loading = false;
        match result {
            Ok(data) => {
                self.data = Some(Arc::new(data));
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
        true
    }
}

#[derive(Default)]
pub(crate) struct RepositoryContextState {
    pub worktrees_expanded: bool,
    pub stashes_expanded: bool,
    pub worktrees: ContextList<WorktreeList>,
    pub stashes: ContextList<StashList>,
    generation: u64,
}

#[derive(Debug, PartialEq, Eq)]
enum StashListUpdate {
    Retained,
    SelectedUnavailable(String),
}

impl RepositoryContextState {
    pub fn invalidate(&mut self, clear: bool) {
        self.generation = self.generation.wrapping_add(1);
        self.worktrees.invalidate(clear);
        self.stashes.invalidate(clear);
    }

    fn accept_worktrees(
        &mut self,
        request: &ContextRequest,
        result: Result<WorktreeList, String>,
    ) -> bool {
        self.worktrees.complete(request, self.generation, result)
    }

    fn accept_stashes(
        &mut self,
        request: &ContextRequest,
        result: Result<StashList, String>,
        selected_sha: &str,
    ) -> Option<StashListUpdate> {
        let missing = self.stashes.is_current(request, self.generation)
            && !selected_sha.is_empty()
            && result.as_ref().is_ok_and(|list| {
                !list.truncated && !list.stashes.iter().any(|stash| stash.sha == selected_sha)
            });
        if !self.stashes.complete(request, self.generation, result) {
            return None;
        }
        Some(if missing {
            StashListUpdate::SelectedUnavailable(selected_sha.to_owned())
        } else {
            StashListUpdate::Retained
        })
    }
}

impl SidebarView {
    pub(crate) fn load_expanded_context(&mut self, cx: &mut Context<Self>) {
        if self.context.worktrees_expanded {
            self.load_worktrees(cx);
        }
        if self.context.stashes_expanded || !self.selected_stash.is_empty() {
            self.load_stashes(cx);
        }
    }

    pub(crate) fn toggle_worktrees(&mut self, cx: &mut Context<Self>) {
        self.context.worktrees_expanded = !self.context.worktrees_expanded;
        if self.context.worktrees_expanded {
            self.load_worktrees(cx);
        }
        self.rebuild_rows(cx);
    }

    pub(crate) fn toggle_stashes(&mut self, cx: &mut Context<Self>) {
        self.context.stashes_expanded = !self.context.stashes_expanded;
        if self.context.stashes_expanded {
            self.load_stashes(cx);
        }
        self.rebuild_rows(cx);
    }

    fn load_worktrees(&mut self, cx: &mut Context<Self>) {
        let Some(repository) = &self.repository else {
            return;
        };
        self.context.worktrees.request(ContextRequest {
            repository_id: repository.id.clone(),
            generation: self.context.generation,
        });
        self.start_worktrees(cx);
    }

    fn start_worktrees(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.context.worktrees.acquire() else {
            return;
        };
        let id = request.repository_id.clone();
        let engine = self.engine.clone();
        self.context.worktrees.task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { engine.list_worktrees(&id) })
                .await;
            let _ = this.update(cx, |view, cx| {
                let accepted = view.context.accept_worktrees(&request, result);
                view.start_worktrees(cx);
                if accepted {
                    view.rebuild_rows(cx);
                }
            });
        }));
    }

    fn load_stashes(&mut self, cx: &mut Context<Self>) {
        let Some(repository) = &self.repository else {
            return;
        };
        self.context.stashes.request(ContextRequest {
            repository_id: repository.id.clone(),
            generation: self.context.generation,
        });
        self.start_stashes(cx);
    }

    fn start_stashes(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.context.stashes.acquire() else {
            return;
        };
        let id = request.repository_id.clone();
        let engine = self.engine.clone();
        self.context.stashes.task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { engine.list_stashes(&id) })
                .await;
            let _ = this.update(cx, |view, cx| {
                let update = view
                    .context
                    .accept_stashes(&request, result, &view.selected_stash);
                // A selected stash still needs validation if its section was
                // collapsed while this read was running.
                view.start_stashes(cx);
                let Some(update) = update else {
                    return;
                };
                view.rebuild_rows(cx);
                if let StashListUpdate::SelectedUnavailable(sha) = update
                    && let Some(repository) = &view.repository
                {
                    cx.emit(SidebarEvent::StashUnavailable {
                        repository_id: repository.id.clone(),
                        sha,
                    });
                }
            });
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::{ContextList, ContextRequest, RepositoryContextState, StashListUpdate};
    use mangrove_core::model::{StashInfo, StashList, WorktreeList};

    fn begin<T>(list: &mut ContextList<T>, repository_id: &str, generation: u64) -> ContextRequest {
        list.request(ContextRequest {
            repository_id: repository_id.into(),
            generation,
        });
        list.acquire().expect("physical read should be available")
    }

    fn empty_stashes(truncated: bool) -> StashList {
        StashList {
            stashes: Vec::new(),
            truncated,
        }
    }

    fn current_stashes(
        state: &mut RepositoryContextState,
        result: Result<StashList, String>,
        selected_sha: &str,
    ) -> Option<StashListUpdate> {
        let request = begin(&mut state.stashes, "repository", state.generation);
        state.accept_stashes(&request, result, selected_sha)
    }

    #[test]
    fn blocking_read_coalesces_refreshes_and_stale_completion_keeps_latest_loading() {
        let mut list = ContextList::<u32>::default();
        let old = begin(&mut list, "repository", 0);
        for generation in 1..=3 {
            list.invalidate(false);
            list.request(ContextRequest {
                repository_id: "repository".into(),
                generation,
            });
            assert!(list.acquire().is_none(), "physical read overlapped");
        }
        assert!(!list.complete(&old, 3, Err("obsolete error".into())));
        assert!(list.loading);
        assert!(list.error.is_none());
        assert!(list.data.is_none());
        let latest = list.acquire().expect("latest refresh was lost");
        assert_eq!(latest.generation, 3);
        assert_eq!(latest.repository_id, "repository");
        assert!(list.acquire().is_none());

        // A duplicate callback cannot release the newer physical read.
        assert!(!list.complete(&old, 3, Ok(10)));
        assert!(list.acquire().is_none());
        assert!(list.loading);
        assert!(list.complete(&latest, 3, Ok(42)));
        assert!(!list.loading);
        assert_eq!(list.data.as_deref(), Some(&42));
        assert!(list.acquire().is_none());
    }

    #[test]
    fn reset_discards_previous_intents_without_cancelling_the_physical_read() {
        let mut state = RepositoryContextState::default();
        let old = begin(
            &mut state.worktrees,
            "previous-repository",
            state.generation,
        );
        state.invalidate(false);
        state.worktrees.request(ContextRequest {
            repository_id: "previous-repository".into(),
            generation: state.generation,
        });
        state.invalidate(true);
        assert!(state.worktrees.pending.is_none());
        assert!(state.worktrees.acquire().is_none());
        assert!(!state.accept_worktrees(&old, Err("previous repository error".into())));
        assert!(state.worktrees.acquire().is_none());
        assert!(!state.worktrees.loading);
        assert!(state.worktrees.error.is_none());
        assert!(state.worktrees.data.is_none());

        let current = begin(&mut state.worktrees, "new-repository", state.generation);
        assert!(state.accept_worktrees(
            &current,
            Ok(WorktreeList {
                worktrees: Vec::new(),
                truncated: false,
            }),
        ));
        assert!(!state.worktrees.loading);
        assert!(!state.worktrees.data.as_ref().unwrap().truncated);
    }

    #[test]
    fn repeated_intent_for_active_generation_does_not_schedule_another_read() {
        let mut list = ContextList::<u32>::default();
        let request = begin(&mut list, "repository", 0);
        for _ in 0..3 {
            list.request(request.clone());
            assert!(list.acquire().is_none());
        }
        assert!(list.complete(&request, 0, Ok(1)));
        assert!(list.acquire().is_none());
    }

    #[test]
    fn same_repository_refresh_retains_lists_and_switching_repository_clears_them() {
        let mut state = RepositoryContextState {
            worktrees_expanded: true,
            stashes_expanded: true,
            ..Default::default()
        };
        assert_eq!(
            current_stashes(&mut state, Ok(empty_stashes(false)), ""),
            Some(StashListUpdate::Retained)
        );
        state.invalidate(false);
        assert!(state.stashes.data.is_some());
        state.invalidate(true);
        assert!(state.stashes.data.is_none());
        assert!(state.worktrees_expanded && state.stashes_expanded);
    }

    #[test]
    fn stale_stash_failure_or_disappearance_cannot_affect_selected_stash() {
        for result in [Ok(empty_stashes(false)), Err("obsolete error".into())] {
            let mut state = RepositoryContextState::default();
            let old = begin(&mut state.stashes, "repository", state.generation);
            state.invalidate(false);
            state.stashes.request(ContextRequest {
                repository_id: "repository".into(),
                generation: state.generation,
            });
            assert!(state.stashes.acquire().is_none());
            assert_eq!(state.accept_stashes(&old, result, "selected"), None);
            assert!(state.stashes.loading);
            assert!(state.stashes.error.is_none());
            assert!(state.stashes.data.is_none());

            // Acquiring the already-requested refresh does not depend on the
            // section's expanded flag. Selected stashes still get validated.
            assert!(!state.stashes_expanded);
            let current = state
                .stashes
                .acquire()
                .expect("latest stash intent was lost");
            assert_eq!(
                state.accept_stashes(&current, Ok(empty_stashes(false)), "selected"),
                Some(StashListUpdate::SelectedUnavailable("selected".into()))
            );
            assert!(!state.stashes.loading);
        }
    }

    #[test]
    fn only_complete_successful_current_stash_list_can_confirm_selection_disappeared() {
        let mut state = RepositoryContextState::default();
        assert_eq!(
            current_stashes(&mut state, Ok(empty_stashes(true)), "selected"),
            Some(StashListUpdate::Retained)
        );
        assert_eq!(
            current_stashes(&mut state, Err("unavailable".into()), "selected"),
            Some(StashListUpdate::Retained)
        );
        assert_eq!(state.stashes.error.as_deref(), Some("unavailable"));
        assert_eq!(
            current_stashes(&mut state, Ok(empty_stashes(false)), "selected"),
            Some(StashListUpdate::SelectedUnavailable("selected".into()))
        );
        assert!(state.stashes.error.is_none());
    }

    #[test]
    fn present_stash_or_empty_selection_survives_complete_list_refresh() {
        let mut state = RepositoryContextState::default();
        assert_eq!(
            current_stashes(
                &mut state,
                Ok(StashList {
                    stashes: vec![StashInfo {
                        sha: "selected".into(),
                        index: 0,
                        message: "stash".into(),
                        author: "author".into(),
                        time: 0,
                        base_sha: "base".into(),
                    }],
                    truncated: false,
                }),
                "selected",
            ),
            Some(StashListUpdate::Retained)
        );
        assert_eq!(
            current_stashes(&mut state, Ok(empty_stashes(false)), ""),
            Some(StashListUpdate::Retained)
        );
    }
}
