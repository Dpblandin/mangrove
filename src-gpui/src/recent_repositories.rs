//! A metadata reply may rename only the recent entries that requested it.
//! Opening or removing an entry invalidates its pending reply, even when its
//! visible name is unchanged. Preferences retain the persisted list and format.
use crate::preferences::RecentRepository;
use mangrove_core::model::RepositoryName;
use std::collections::HashMap;

pub struct RecentNamesRequest {
    revision: u64,
    paths: Vec<String>,
}

impl RecentNamesRequest {
    pub fn paths(&self) -> &[String] {
        &self.paths
    }
}

#[derive(Default)]
pub struct RecentMetadata {
    revision: u64,
    pending: HashMap<String, u64>,
}

impl RecentMetadata {
    pub fn begin(&mut self, recent: &[RecentRepository]) -> Option<RecentNamesRequest> {
        self.revision = self.revision.wrapping_add(1);
        self.pending.clear();
        let paths = recent
            .iter()
            .take(6)
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>();
        for path in &paths {
            self.pending.insert(path.clone(), self.revision);
        }
        if paths.is_empty() {
            None
        } else {
            Some(RecentNamesRequest {
                revision: self.revision,
                paths,
            })
        }
    }

    pub fn invalidate(&mut self, path: &str) {
        self.pending.remove(path);
    }

    pub fn apply(
        &mut self,
        request: RecentNamesRequest,
        recent: &mut [RecentRepository],
        names: &[RepositoryName],
    ) -> bool {
        let mut changed = false;
        for path in request.paths {
            if self.pending.get(&path) != Some(&request.revision) {
                continue;
            }
            self.pending.remove(&path);
            if let Some(entry) = recent.iter_mut().find(|entry| entry.path == path)
                && let Some(name) = names
                    .iter()
                    .find(|name| name.path == path)
                    .and_then(|name| name.name.as_ref())
                && entry.name != *name
            {
                entry.name = name.clone();
                changed = true;
            }
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, name: &str) -> RecentRepository {
        RecentRepository {
            path: path.into(),
            name: name.into(),
        }
    }

    fn name(path: &str, name: Option<&str>) -> RepositoryName {
        RepositoryName {
            path: path.into(),
            name: name.map(str::to_owned),
        }
    }

    #[test]
    fn old_metadata_cannot_replace_new_opening_but_can_update_other_entries() {
        let mut recent = vec![entry("/opened", "old"), entry("/other", "folder")];
        let mut metadata = RecentMetadata::default();
        let request = metadata.begin(&recent).unwrap();
        metadata.invalidate("/opened");
        recent[0].name = "new opening".into();
        assert!(metadata.apply(
            request,
            &mut recent,
            &[
                name("/opened", Some("old metadata")),
                name("/other", Some("repository · branch")),
            ],
        ));
        assert_eq!(recent[0].name, "new opening");
        assert_eq!(recent[1].name, "repository · branch");
    }

    #[test]
    fn removal_and_reopening_with_unchanged_name_do_not_reaccept_old_metadata() {
        let mut recent = vec![entry("/repo", "folder")];
        let mut metadata = RecentMetadata::default();
        let removed_request = metadata.begin(&recent).unwrap();
        metadata.invalidate("/repo");
        recent.clear();
        assert!(!metadata.apply(
            removed_request,
            &mut recent,
            &[name("/repo", Some("stale"))],
        ));
        assert!(recent.is_empty());

        recent.push(entry("/repo", "folder"));
        let reopened_request = metadata.begin(&recent).unwrap();
        metadata.invalidate("/repo");
        recent.clear();
        recent.push(entry("/repo", "folder"));
        assert!(!metadata.apply(
            reopened_request,
            &mut recent,
            &[name("/repo", Some("stale"))],
        ));
        assert_eq!(recent, vec![entry("/repo", "folder")]);
    }

    #[test]
    fn newer_metadata_request_wins_regardless_of_reply_order() {
        let mut recent = vec![entry("/repo", "folder")];
        let mut metadata = RecentMetadata::default();
        let old = metadata.begin(&recent).unwrap();
        let current = metadata.begin(&recent).unwrap();
        assert!(metadata.apply(current, &mut recent, &[name("/repo", Some("current"))]));
        assert!(!metadata.apply(old, &mut recent, &[name("/repo", Some("old"))]));
        assert_eq!(recent[0].name, "current");
    }

    #[test]
    fn requests_are_bounded_and_missing_metadata_keeps_last_known_name() {
        let mut recent = (0..7)
            .map(|index| entry(&format!("/{index}"), "last known"))
            .collect::<Vec<_>>();
        let mut metadata = RecentMetadata::default();
        let request = metadata.begin(&recent).unwrap();
        assert_eq!(request.paths().len(), 6);
        assert!(!metadata.apply(request, &mut recent, &[name("/0", None)]));
        assert!(recent.iter().all(|entry| entry.name == "last known"));
        assert!(metadata.begin(&[]).is_none());
    }
}
