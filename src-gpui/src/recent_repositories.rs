//! A metadata reply may update only the recent entries that requested it.
//! Opening or removing an entry invalidates its pending reply, even when its
//! visible name is unchanged. Preferences retain the persisted list and format.
use crate::preferences::RecentRepository;
use mangrove_core::model::RepositoryName;
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

pub struct RecentRepositoryGroup {
    pub repository: RecentRepository,
    pub removable: bool,
    pub worktrees: Vec<RecentRepository>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct RecentMetadataUpdate {
    pub names_changed: bool,
    pub locations_changed: bool,
}

#[derive(PartialEq, Eq)]
struct RecentLocation {
    repository_path: String,
    primary_path: Option<String>,
}

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
    locations: HashMap<String, RecentLocation>,
}

impl RecentMetadata {
    pub fn begin(&mut self, recent: &[RecentRepository]) -> Option<RecentNamesRequest> {
        self.revision = self.revision.wrapping_add(1);
        self.pending.clear();
        self.locations
            .retain(|path, _| recent.iter().any(|entry| entry.path == *path));
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

    pub fn equivalent_paths(&self, path: &str) -> Vec<String> {
        let repository_path = self
            .locations
            .get(path)
            .map_or(path, |location| location.repository_path.as_str());
        let mut paths = vec![path.to_owned()];
        paths.extend(
            self.locations
                .iter()
                .filter(|(candidate, location)| {
                    candidate.as_str() != path && location.repository_path == repository_path
                })
                .map(|(candidate, _)| candidate.clone()),
        );
        paths
    }

    pub fn groups(&self, recent: &[RecentRepository], query: &str) -> Vec<RecentRepositoryGroup> {
        let mut groups: Vec<RecentRepositoryGroup> = Vec::new();
        let mut indices = HashMap::new();
        let mut seen = HashSet::new();
        for entry in recent {
            let location = self.locations.get(&entry.path);
            let repository_path = location
                .map(|location| location.repository_path.as_str())
                .unwrap_or(&entry.path);
            if !seen.insert(repository_path) {
                continue;
            }
            let primary_path = location.and_then(|location| location.primary_path.as_deref());
            let group_path = primary_path.unwrap_or(repository_path);
            let index = *indices.entry(group_path).or_insert_with(|| {
                let repository = recent.iter().find(|candidate| {
                    self.locations.get(&candidate.path).map_or(
                        candidate.path == group_path,
                        |location| {
                            location.primary_path.is_none()
                                && location.repository_path == group_path
                        },
                    )
                });
                groups.push(RecentRepositoryGroup {
                    repository: repository.cloned().unwrap_or_else(|| RecentRepository {
                        path: group_path.into(),
                        name: Path::new(group_path)
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned(),
                    }),
                    removable: repository.is_some(),
                    worktrees: Vec::new(),
                });
                groups.len() - 1
            });
            if primary_path.is_some() {
                groups[index].worktrees.push(entry.clone());
            }
        }
        groups.retain_mut(|group| {
            if matches_query(&group.repository, query) {
                return true;
            }
            group.worktrees.retain(|entry| matches_query(entry, query));
            !group.worktrees.is_empty()
        });
        groups
    }

    pub fn apply(
        &mut self,
        request: RecentNamesRequest,
        recent: &mut [RecentRepository],
        names: &[RepositoryName],
    ) -> RecentMetadataUpdate {
        let mut changed = RecentMetadataUpdate::default();
        for path in request.paths {
            if self.pending.get(&path) != Some(&request.revision) {
                continue;
            }
            self.pending.remove(&path);
            if let Some(entry) = recent.iter_mut().find(|entry| entry.path == path)
                && let Some(metadata) = names.iter().find(|name| name.path == path)
            {
                if let Some(name) = &metadata.name
                    && entry.name != *name
                {
                    entry.name = name.clone();
                    changed.names_changed = true;
                }
                if let Some(repository_path) = &metadata.repository_path {
                    let location = RecentLocation {
                        repository_path: repository_path.clone(),
                        primary_path: metadata.primary_path.clone(),
                    };
                    if self.locations.get(&path) != Some(&location) {
                        self.locations.insert(path, location);
                        changed.locations_changed = true;
                    }
                }
            }
        }
        changed
    }
}

/// Profile favorites hold a local path or a clone URL (`https://…`, `git@host:…`).
pub fn is_remote_favorite(favorite: &str) -> bool {
    favorite.contains("://")
        || favorite
            .split_once(':')
            .is_some_and(|(host, _)| host.contains('@') && !host.contains(['/', '\\']))
}

/// Last path segment, without a `.git` suffix for URLs.
pub fn favorite_name(favorite: &str) -> String {
    let remote = is_remote_favorite(favorite);
    let trimmed = favorite.trim_end_matches(['/', '\\']);
    let name = trimmed
        .rsplit(|c| matches!(c, '/' | '\\') || (remote && c == ':'))
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(trimmed);
    if remote {
        name.strip_suffix(".git").unwrap_or(name).to_owned()
    } else {
        name.to_owned()
    }
}

fn matches_query(entry: &RecentRepository, query: &str) -> bool {
    query.is_empty()
        || format!("{} {}", entry.name, entry.path)
            .to_lowercase()
            .contains(query)
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
            repository_path: name.map(|_| path.to_owned()),
            primary_path: None,
        }
    }

    fn worktree(path: &str, label: &str, primary: &str) -> RepositoryName {
        RepositoryName {
            primary_path: Some(primary.into()),
            ..name(path, Some(label))
        }
    }

    #[test]
    fn worktrees_follow_their_primary_repository_in_recent_order() {
        let mut recent = vec![
            entry("/worktrees/review", "mangrove · review"),
            entry("/projects/easy-care", "easy-care"),
            entry("/projects/mangrove", "mangrove"),
            entry("/worktrees/theme", "mangrove · theme"),
        ];
        let mut metadata = RecentMetadata::default();
        let request = metadata.begin(&recent).unwrap();
        metadata.apply(
            request,
            &mut recent,
            &[
                worktree(
                    "/worktrees/review",
                    "mangrove · review",
                    "/projects/mangrove",
                ),
                name("/projects/easy-care", Some("easy-care")),
                name("/projects/mangrove", Some("mangrove")),
                worktree("/worktrees/theme", "mangrove · theme", "/projects/mangrove"),
            ],
        );
        let groups = metadata.groups(&recent, "");
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].repository, recent[2]);
        assert!(groups[0].removable);
        assert_eq!(
            groups[0].worktrees,
            vec![recent[0].clone(), recent[3].clone()]
        );
        assert_eq!(groups[1].repository, recent[1]);

        let filtered = metadata.groups(&recent, "review");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].repository, recent[2]);
        assert_eq!(filtered[0].worktrees, vec![recent[0].clone()]);
        assert_eq!(metadata.groups(&recent, "mangrove")[0].worktrees.len(), 2);
        assert!(metadata.groups(&recent, "unknown").is_empty());

        recent.remove(2);
        let groups = metadata.groups(&recent, "");
        assert_eq!(groups[0].repository.path, "/projects/mangrove");
        assert!(!groups[0].removable);
        assert_eq!(groups[0].worktrees.len(), 2);
    }

    #[test]
    fn grouping_uses_git_paths_even_for_same_named_repositories_and_aliases() {
        let mut recent = vec![
            entry("/worktrees/one", "project · topic"),
            entry("/worktrees/two", "project · topic"),
            entry("/alias", "project"),
            entry("/other/project", "project"),
        ];
        let mut metadata = RecentMetadata::default();
        let request = metadata.begin(&recent).unwrap();
        let mut alias = name("/alias", Some("project"));
        alias.repository_path = Some("/projects/project".into());
        metadata.apply(
            request,
            &mut recent,
            &[
                worktree("/worktrees/one", "project · topic", "/projects/project"),
                worktree("/worktrees/two", "project · topic", "/other/project"),
                alias,
                name("/other/project", Some("project")),
            ],
        );
        let groups = metadata.groups(&recent, "");
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].repository.path, "/alias");
        assert_eq!(groups[0].worktrees, vec![recent[0].clone()]);
        assert_eq!(groups[1].repository.path, "/other/project");
        assert_eq!(groups[1].worktrees, vec![recent[1].clone()]);
    }

    #[test]
    fn canonical_aliases_only_show_the_most_recent_worktree_once() {
        let mut recent = vec![
            entry("/alias/topic", "project · topic"),
            entry("/worktrees/topic", "project · topic"),
            entry("/project", "project"),
        ];
        let mut metadata = RecentMetadata::default();
        let request = metadata.begin(&recent).unwrap();
        let mut alias = worktree("/alias/topic", "project · topic", "/project");
        alias.repository_path = Some("/worktrees/topic".into());
        metadata.apply(
            request,
            &mut recent,
            &[
                alias,
                worktree("/worktrees/topic", "project · topic", "/project"),
                name("/project", Some("project")),
            ],
        );
        let groups = metadata.groups(&recent, "");
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].repository, recent[2]);
        assert_eq!(groups[0].worktrees, vec![recent[0].clone()]);
        let equivalent_paths = metadata.equivalent_paths("/alias/topic");
        assert_eq!(equivalent_paths.len(), 2);
        recent.retain(|entry| !equivalent_paths.contains(&entry.path));
        let groups = metadata.groups(&recent, "");
        assert_eq!(groups.len(), 1);
        assert!(groups[0].worktrees.is_empty());
    }

    #[test]
    fn old_metadata_cannot_replace_new_opening_but_can_update_other_entries() {
        let mut recent = vec![entry("/opened", "old"), entry("/other", "folder")];
        let mut metadata = RecentMetadata::default();
        let request = metadata.begin(&recent).unwrap();
        metadata.invalidate("/opened");
        recent[0].name = "new opening".into();
        assert!(
            metadata
                .apply(
                    request,
                    &mut recent,
                    &[
                        name("/opened", Some("old metadata")),
                        name("/other", Some("repository · branch")),
                    ],
                )
                .names_changed
        );
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
        assert_eq!(
            metadata.apply(
                removed_request,
                &mut recent,
                &[name("/repo", Some("stale"))],
            ),
            RecentMetadataUpdate::default()
        );
        assert!(recent.is_empty());

        recent.push(entry("/repo", "folder"));
        let reopened_request = metadata.begin(&recent).unwrap();
        metadata.invalidate("/repo");
        recent.clear();
        recent.push(entry("/repo", "folder"));
        assert_eq!(
            metadata.apply(
                reopened_request,
                &mut recent,
                &[name("/repo", Some("stale"))],
            ),
            RecentMetadataUpdate::default()
        );
        assert_eq!(recent, vec![entry("/repo", "folder")]);
    }

    #[test]
    fn newer_metadata_request_wins_regardless_of_reply_order() {
        let mut recent = vec![entry("/repo", "folder")];
        let mut metadata = RecentMetadata::default();
        let old = metadata.begin(&recent).unwrap();
        let current = metadata.begin(&recent).unwrap();
        assert!(
            metadata
                .apply(current, &mut recent, &[name("/repo", Some("current"))])
                .names_changed
        );
        assert_eq!(
            metadata.apply(old, &mut recent, &[name("/repo", Some("old"))]),
            RecentMetadataUpdate::default()
        );
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
        assert_eq!(
            metadata.apply(request, &mut recent, &[name("/0", None)]),
            RecentMetadataUpdate::default()
        );
        assert!(recent.iter().all(|entry| entry.name == "last known"));
        assert!(metadata.begin(&[]).is_none());
    }

    #[test]
    fn location_updates_redraw_without_requiring_preferences_to_be_saved() {
        let mut recent = vec![entry("/worktrees/topic", "project · topic")];
        let mut metadata = RecentMetadata::default();
        let names = [worktree("/worktrees/topic", "project · topic", "/project")];
        let request = metadata.begin(&recent).unwrap();
        assert_eq!(
            metadata.apply(request, &mut recent, &names),
            RecentMetadataUpdate {
                names_changed: false,
                locations_changed: true,
            }
        );
        assert_eq!(metadata.groups(&recent, "")[0].repository.path, "/project");

        let request = metadata.begin(&recent).unwrap();
        assert_eq!(
            metadata.apply(request, &mut recent, &names),
            RecentMetadataUpdate::default()
        );
    }

    #[test]
    fn favorites_distinguish_clone_urls_from_local_paths() {
        for (favorite, remote, name) in [
            ("/Users/me/projects/mangrove/", false, "mangrove"),
            ("C:\\code\\mangrove", false, "mangrove"),
            ("https://example.com/team/project.git", true, "project"),
            ("ssh://git@example.com:22/team/tool", true, "tool"),
            ("git@example.com:team/service.git", true, "service"),
            ("/srv/repos/name.git", false, "name.git"),
        ] {
            assert_eq!(is_remote_favorite(favorite), remote, "{favorite}");
            assert_eq!(favorite_name(favorite), name, "{favorite}");
        }
    }
}
