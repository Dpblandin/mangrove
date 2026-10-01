//! Tabs retain identities and locations; repository data belongs to one active workspace.
//! An opening attempt owns the target's rollback snapshot and cannot complete
//! after that target has been closed, replaced, or left by a newer navigation.
use crate::chrome::ChromeTab;

pub struct TabOpenAttempt {
    id: u64,
    revision: u64,
    previous: Option<ChromeTab>,
}

#[derive(Default)]
pub struct Tabs {
    pub entries: Vec<ChromeTab>,
    pub active: Option<u64>,
    next_id: u64,
    revision: u64,
}

impl Tabs {
    pub fn active_tab(&self) -> Option<&ChromeTab> {
        self.entries.iter().find(|tab| Some(tab.id) == self.active)
    }
    pub fn create(&mut self) -> u64 {
        self.revision = self.revision.wrapping_add(1);
        self.next_id += 1;
        let id = self.next_id;
        self.entries.push(ChromeTab {
            id,
            name: "Nouvel onglet".into(),
            path: None,
        });
        self.active = Some(id);
        id
    }
    pub fn begin_open(&mut self, path: String) -> TabOpenAttempt {
        let previous = self
            .entries
            .iter()
            .find(|tab| tab.path.as_ref() == Some(&path))
            .or_else(|| self.active_tab().filter(|tab| tab.path.is_none()))
            .cloned();
        let id = self.open_target(path);
        self.revision = self.revision.wrapping_add(1);
        TabOpenAttempt {
            id,
            revision: self.revision,
            previous,
        }
    }
    pub fn is_current(&self, attempt: &TabOpenAttempt) -> bool {
        self.revision == attempt.revision
            && self.active == Some(attempt.id)
            && self.entries.iter().any(|tab| tab.id == attempt.id)
    }
    pub fn complete_open(&mut self, attempt: TabOpenAttempt, path: String, name: String) -> bool {
        if !self.is_current(&attempt) {
            return false;
        }
        self.revision = self.revision.wrapping_add(1);
        self.update(attempt.id, path, name);
        true
    }
    pub fn reject_open(&mut self, attempt: TabOpenAttempt) -> bool {
        if !self.is_current(&attempt) {
            return false;
        }
        self.revision = self.revision.wrapping_add(1);
        if let Some(tab) = self.entries.iter_mut().find(|tab| tab.id == attempt.id) {
            *tab = attempt.previous.unwrap_or(ChromeTab {
                id: attempt.id,
                name: "Nouvel onglet".into(),
                path: None,
            });
        }
        true
    }
    fn open_target(&mut self, path: String) -> u64 {
        let draft = self
            .active_tab()
            .filter(|tab| tab.path.is_none())
            .map(|tab| tab.id);
        if let Some(id) = self
            .entries
            .iter()
            .find(|tab| tab.path.as_ref() == Some(&path))
            .map(|tab| tab.id)
        {
            if let Some(draft) = draft {
                self.entries.retain(|tab| tab.id != draft);
            }
            self.active = Some(id);
            return id;
        }
        let id = draft.unwrap_or_else(|| self.create());
        let name = path
            .trim_end_matches(['/', '\\'])
            .rsplit(['/', '\\'])
            .next()
            .filter(|name| !name.is_empty())
            .unwrap_or(&path)
            .to_owned();
        if let Some(tab) = self.entries.iter_mut().find(|tab| tab.id == id) {
            tab.path = Some(path);
            tab.name = name;
        }
        self.active = Some(id);
        id
    }
    pub fn activate(&mut self, id: u64) -> Option<String> {
        let tab = self.entries.iter().find(|tab| tab.id == id)?;
        if self.active != Some(id) {
            self.revision = self.revision.wrapping_add(1);
        }
        self.active = Some(id);
        tab.path.clone()
    }
    pub fn close(&mut self, id: u64) {
        let Some(index) = self.entries.iter().position(|tab| tab.id == id) else {
            return;
        };
        self.entries.remove(index);
        if self.active == Some(id) {
            self.revision = self.revision.wrapping_add(1);
            self.active = self
                .entries
                .get(index)
                .or_else(|| index.checked_sub(1).and_then(|i| self.entries.get(i)))
                .map(|tab| tab.id);
        }
    }
    fn update(&mut self, id: u64, path: String, name: String) {
        if !self.entries.iter().any(|tab| tab.id == id) {
            return;
        }
        if let Some(duplicate) = self
            .entries
            .iter()
            .find(|tab| tab.id != id && tab.path.as_ref() == Some(&path))
            .map(|tab| tab.id)
        {
            if let Some(tab) = self.entries.iter_mut().find(|tab| tab.id == duplicate) {
                tab.name = name;
            }
            self.entries.retain(|tab| tab.id != id);
            if self.active == Some(id) {
                self.active = Some(duplicate);
            }
        } else if let Some(tab) = self.entries.iter_mut().find(|tab| tab.id == id) {
            tab.path = Some(path);
            tab.name = name;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(tabs: &mut Tabs, path: &str) -> u64 {
        let attempt = tabs.begin_open(path.into());
        let id = attempt.id;
        assert!(tabs.complete_open(attempt, path.into(), path.into()));
        id
    }

    fn assert_valid_identity(tabs: &Tabs) {
        let ids = tabs
            .entries
            .iter()
            .map(|tab| tab.id)
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(ids.len(), tabs.entries.len());
        assert!(tabs.active.is_none_or(|active| ids.contains(&active)));
    }

    #[test]
    fn opening_existing_repository_consumes_draft_and_canonical_alias_merges_tabs() {
        let mut tabs = Tabs::default();
        let original = open(&mut tabs, "/repo");
        tabs.create();
        assert_eq!(open(&mut tabs, "/repo"), original);
        assert_eq!(tabs.entries.len(), 1);
        let alias = tabs.begin_open("/alias".into());
        assert!(tabs.complete_open(alias, "/repo".into(), "canonical".into()));
        assert_eq!(tabs.active, Some(original));
        assert_eq!(tabs.entries.len(), 1);
        assert_eq!(tabs.active_tab().unwrap().name, "canonical");
        assert_valid_identity(&tabs);
    }
    #[test]
    fn closing_active_tab_selects_its_next_neighbor_then_previous_then_welcome() {
        let mut tabs = Tabs::default();
        let a = open(&mut tabs, "a");
        let b = open(&mut tabs, "b");
        let c = open(&mut tabs, "c");
        tabs.activate(b);
        tabs.close(b);
        assert_eq!(tabs.active, Some(c));
        tabs.close(c);
        assert_eq!(tabs.active, Some(a));
        tabs.close(a);
        assert_eq!(tabs.active, None);
        assert_valid_identity(&tabs);
    }

    #[test]
    fn failed_open_from_repository_restores_target_as_draft_and_keeps_original_identity() {
        let mut tabs = Tabs::default();
        let original = open(&mut tabs, "/repo");
        let original_tab = tabs.active_tab().unwrap().clone();
        let failed = tabs.begin_open("/missing".into());
        let target = failed.id;
        assert_ne!(target, original);
        assert!(tabs.reject_open(failed));
        assert_eq!(tabs.entries[0], original_tab);
        assert_eq!(tabs.active, Some(target));
        assert_eq!(tabs.active_tab().unwrap().path, None);
        assert_eq!(tabs.active_tab().unwrap().name, "Nouvel onglet");
        assert_valid_identity(&tabs);
    }

    #[test]
    fn failed_open_reuses_and_restores_existing_draft() {
        let mut tabs = Tabs::default();
        open(&mut tabs, "/repo");
        let draft = tabs.create();
        let previous = tabs.active_tab().unwrap().clone();
        let failed = tabs.begin_open("/missing".into());
        assert_eq!(failed.id, draft);
        assert!(tabs.reject_open(failed));
        assert_eq!(tabs.active_tab(), Some(&previous));
        assert_eq!(tabs.entries.len(), 2);
        assert_valid_identity(&tabs);
    }

    #[test]
    fn failed_reopen_restores_target_instead_of_previous_active_tab() {
        let mut tabs = Tabs::default();
        let target = open(&mut tabs, "/first");
        let previous = tabs.active_tab().unwrap().clone();
        let other = open(&mut tabs, "/second");
        let failed = tabs.begin_open("/first".into());
        assert!(tabs.reject_open(failed));
        assert_eq!(tabs.active, Some(target));
        assert_eq!(tabs.active_tab(), Some(&previous));
        assert_eq!(tabs.entries[1].id, other);
        assert_valid_identity(&tabs);
    }

    #[test]
    fn superseded_attempt_cannot_rollback_same_target_or_replace_its_success() {
        let mut tabs = Tabs::default();
        let first = tabs.begin_open("/repo".into());
        let second = tabs.begin_open("/repo".into());
        assert_eq!(first.id, second.id);
        assert!(!tabs.is_current(&first));
        assert!(!tabs.reject_open(first));
        assert!(tabs.complete_open(second, "/canonical".into(), "repository".into()));
        assert_eq!(
            tabs.active_tab().unwrap().path.as_deref(),
            Some("/canonical")
        );
        assert_valid_identity(&tabs);
    }

    #[test]
    fn late_attempt_cannot_recreate_closed_target_or_change_new_tab() {
        let mut tabs = Tabs::default();
        let failed = tabs.begin_open("/missing".into());
        tabs.close(failed.id);
        let draft = tabs.create();
        let previous = tabs.active_tab().unwrap().clone();
        assert!(!tabs.reject_open(failed));
        assert_eq!(tabs.active, Some(draft));
        assert_eq!(tabs.active_tab(), Some(&previous));

        let late = tabs.begin_open("/repo".into());
        tabs.create();
        assert!(!tabs.complete_open(late, "/repo".into(), "repository".into()));
        assert_eq!(tabs.active_tab().unwrap().path, None);
        assert_valid_identity(&tabs);
    }

    #[test]
    fn closing_unrelated_tab_does_not_cancel_active_opening() {
        let mut tabs = Tabs::default();
        let other = open(&mut tabs, "/first");
        let attempt = tabs.begin_open("/second".into());
        tabs.close(other);
        assert!(tabs.is_current(&attempt));
        assert!(tabs.complete_open(attempt, "/second".into(), "second".into()));
        assert_eq!(tabs.entries.len(), 1);
        assert_valid_identity(&tabs);
    }
}
