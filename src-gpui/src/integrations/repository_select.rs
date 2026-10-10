use super::*;
use gpui_kit::component::{IndexPath, spinner::Spinner};
use std::{
    cell::RefCell,
    collections::{BTreeMap, HashSet},
    rc::Rc,
};

const MAX_REPOSITORIES: usize = 20_000;
const MAX_REPOSITORY_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug)]
pub(super) struct RepositoryOption {
    id: String,
    name: String,
    full_name: String,
}

impl SearchableListItem for RepositoryOption {
    type Value = String;

    fn title(&self) -> SharedString {
        self.full_name.clone().into()
    }

    fn value(&self) -> &String {
        &self.id
    }

    fn render(&self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(gpui_kit::component::Icon::new(IconName::FolderGit2).with_size(px(14.)))
            .child(div().min_w_0().truncate().child(self.name.clone()))
    }
}

pub(super) fn repository_option_groups(
    repositories: &[Repository],
) -> Vec<SearchableGroup<RepositoryOption>> {
    let mut groups = BTreeMap::<String, Vec<RepositoryOption>>::new();
    for repository in repositories {
        let namespace = repository
            .full_name
            .rsplit_once('/')
            .map_or("Dépôts", |(namespace, _)| namespace);
        groups
            .entry(namespace.to_string())
            .or_default()
            .push(RepositoryOption {
                id: repository.id.clone(),
                name: repository.name.clone(),
                full_name: repository.full_name.clone(),
            });
    }
    groups
        .into_iter()
        .map(|(namespace, items)| SearchableGroup::new(namespace).items(items))
        .collect()
}

#[derive(Default)]
pub(super) struct RepositoryMenuData {
    groups: Vec<SearchableGroup<RepositoryOption>>,
    query: String,
}

impl RepositoryMenuData {
    pub(super) fn append(
        &mut self,
        repositories: &mut Vec<Repository>,
        page: Vec<Repository>,
    ) -> mangrove_remote::Result<()> {
        let mut ids: HashSet<_> = repositories.iter().map(|item| item.id.clone()).collect();
        let added: Vec<_> = page
            .into_iter()
            .filter(|item| ids.insert(item.id.clone()))
            .collect();
        let bytes: usize = repositories
            .iter()
            .chain(&added)
            .map(repository_bytes)
            .sum();
        if repositories.len() + added.len() > MAX_REPOSITORIES || bytes > MAX_REPOSITORY_BYTES {
            return Err(mangrove_remote::RemoteError::ResponseTooLarge);
        }
        // Never insert into an existing section: Select confirms deferred IndexPaths,
        // and inserting above the viewport also shifts its scroll position.
        self.groups.extend(repository_option_groups(&added));
        repositories.extend(added);
        Ok(())
    }

    fn matched_groups(&self) -> Vec<SearchableGroup<RepositoryOption>> {
        if self.query.is_empty() {
            return self.groups.clone();
        }
        let query = self.query.to_lowercase();
        self.groups
            .iter()
            .filter_map(|group| {
                let items: Vec<_> = group
                    .items
                    .iter()
                    .filter(|item| item.full_name.to_lowercase().contains(&query))
                    .cloned()
                    .collect();
                (!items.is_empty()).then(|| SearchableGroup::new(group.title.clone()).items(items))
            })
            .collect()
    }
}

fn repository_bytes(item: &Repository) -> usize {
    item.id.len()
        + item.name.len()
        + item.full_name.len()
        + item.web_url.as_ref().map_or(0, String::len)
        + item.clone_url.as_ref().map_or(0, String::len)
        + item.ssh_clone_url.as_ref().map_or(0, String::len)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum RepositoryLoadState {
    Ready,
    Loading,
    Retry,
    Complete,
}

#[derive(Clone)]
pub(super) struct RepositoryLoadTrigger {
    view: WeakEntity<IntegrationsView>,
    scope: u64,
}

impl RepositoryLoadTrigger {
    pub(super) fn new(view: WeakEntity<IntegrationsView>, scope: u64) -> Self {
        Self { view, scope }
    }

    fn schedule(&self, retry: bool, cx: &mut App) {
        let trigger = self.clone();
        cx.defer(move |cx| {
            let _ = trigger.view.update(cx, |view, cx| {
                view.load_more_repositories(trigger.scope, retry, cx);
            });
        });
    }
}

pub(super) struct RepositoryMenu {
    data: Rc<RefCell<RepositoryMenuData>>,
    groups: Vec<SearchableGroup<RepositoryOption>>,
    trigger: RepositoryLoadTrigger,
    state: RepositoryLoadState,
    palette: Palette,
    offset: f32,
}

impl RepositoryMenu {
    pub(super) fn new(
        data: Rc<RefCell<RepositoryMenuData>>,
        trigger: RepositoryLoadTrigger,
        state: RepositoryLoadState,
        palette: Palette,
        offset: f32,
    ) -> Self {
        let groups = data.borrow().matched_groups();
        Self {
            data,
            groups,
            trigger,
            state,
            palette,
            offset,
        }
    }
}

impl SearchableListDelegate for RepositoryMenu {
    type Item = RepositoryOption;

    fn sections_count(&self, _: &App) -> usize {
        self.groups.len()
    }

    fn items_count(&self, section: usize) -> usize {
        self.groups
            .get(section)
            .map_or(0, |group| group.items.len())
    }

    fn item(&self, ix: IndexPath) -> Option<&RepositoryOption> {
        self.groups.get(ix.section)?.items.get(ix.row)
    }

    fn position<V>(&self, value: &V) -> Option<IndexPath>
    where
        Self::Item: SearchableListItem<Value = V>,
        V: PartialEq,
    {
        self.groups.iter().enumerate().find_map(|(section, group)| {
            group
                .items
                .iter()
                .position(|item| item.value() == value)
                .map(|row| IndexPath::new(row).section(section))
        })
    }

    fn perform_search(&mut self, query: &str, _: &mut Window, _: &mut App) -> Task<()> {
        let mut data = self.data.borrow_mut();
        data.query = query.to_string();
        self.groups = data.matched_groups();
        Task::ready(())
    }

    fn render_section_header(
        &self,
        section: usize,
        _: &mut Window,
        _: &mut App,
    ) -> Option<AnyElement> {
        Some(
            div()
                .w_full()
                .min_w_0()
                .truncate()
                .px(px(8.))
                .py(px(4.))
                .text_size(px(11. + self.offset))
                .text_color(rgb(self.palette.muted))
                .child(self.groups.get(section)?.title.clone())
                .into_any_element(),
        )
    }

    fn render_item(
        &self,
        ix: IndexPath,
        item: &RepositoryOption,
        checked: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        if ix.section + 1 != self.groups.len()
            || ix.row + 1 != self.items_count(ix.section)
            || self.state == RepositoryLoadState::Complete
        {
            return None;
        }
        let trailing = load_indicator(self.state, self.trigger.clone(), self.palette, self.offset);
        Some(
            div()
                .relative()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(item.render(window, cx)),
                )
                .when(checked, |view| {
                    view.child(icon(IconName::Check, 14., self.palette.accent))
                })
                .child(trailing)
                .when(self.state == RepositoryLoadState::Ready, |view| {
                    view.child(visibility_sentinel(self.trigger.clone()))
                })
                .into_any_element(),
        )
    }
}

fn visibility_sentinel(trigger: RepositoryLoadTrigger) -> impl IntoElement {
    // Lists and empty views can be rendered only to measure their layout.
    // Fetch when painted inside the viewport, never during that measurement.
    canvas(
        |_, _, _| (),
        move |bounds, _, window, cx| {
            if bounds.intersects(&window.content_mask().bounds) {
                trigger.schedule(false, cx);
            }
        },
    )
    .absolute()
    .size_full()
}

fn load_indicator(
    state: RepositoryLoadState,
    trigger: RepositoryLoadTrigger,
    p: Palette,
    offset: f32,
) -> Div {
    match state {
        RepositoryLoadState::Ready | RepositoryLoadState::Loading => div()
            .flex()
            .items_center()
            .gap(px(6.))
            .text_color(rgb(p.muted))
            .text_size(px(11. + offset))
            .child(Spinner::new().small())
            .child("Chargement…"),
        RepositoryLoadState::Retry => div().child(
            Button::new("repositories-retry")
                .px(px(6.))
                .py(px(2.))
                .text_size(px(11. + offset))
                .text_color(rgb(p.accent))
                .accessibility_label("Réessayer le chargement des dépôts")
                .child("Réessayer")
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    trigger.schedule(true, cx);
                }),
        ),
        RepositoryLoadState::Complete => div(),
    }
}

#[derive(IntoElement)]
pub(super) struct RepositoryEmpty {
    pub(super) trigger: RepositoryLoadTrigger,
    pub(super) state: RepositoryLoadState,
    pub(super) palette: Palette,
    pub(super) offset: f32,
}

impl RenderOnce for RepositoryEmpty {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div()
            .relative()
            .p(px(16.))
            .text_size(px(12. + self.offset))
            .text_color(rgb(self.palette.muted))
            .when(self.state == RepositoryLoadState::Ready, |view| {
                view.child(visibility_sentinel(self.trigger.clone()))
            })
            .child(if self.state == RepositoryLoadState::Complete {
                div().child("Aucun dépôt ne correspond.")
            } else {
                load_indicator(self.state, self.trigger, self.palette, self.offset)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_REPOSITORIES, MAX_REPOSITORY_BYTES, RepositoryMenuData};
    use mangrove_remote::Repository;

    fn repository(id: &str, full_name: &str) -> Repository {
        Repository {
            id: id.into(),
            name: "api".into(),
            full_name: full_name.into(),
            web_url: None,
            clone_url: None,
            ssh_clone_url: None,
        }
    }

    #[test]
    fn appending_pages_preserves_visible_indices_and_search_results() {
        let mut data = RepositoryMenuData::default();
        let mut repositories = vec![];
        data.append(
            &mut repositories,
            vec![repository("1", "b/api"), repository("2", "z/api")],
        )
        .unwrap();
        data.query = "API".into();
        let before = data.matched_groups();
        data.append(
            &mut repositories,
            vec![
                repository("1", "b/renamed"),
                repository("3", "a/api"),
                repository("4", "b/worker"),
                repository("3", "duplicate/api"),
            ],
        )
        .unwrap();
        let after = data.matched_groups();
        for (section, group) in before.iter().enumerate() {
            for (row, item) in group.items.iter().enumerate() {
                assert_eq!(after[section].items[row].id, item.id);
            }
        }
        assert_eq!(after[2].items[0].id, "3");
        assert_eq!(repositories.len(), 4);
        assert_eq!(data.query, "API");
        data.query.clear();
        let unfiltered = data.matched_groups();
        assert_eq!(unfiltered[0].items[0].id, "1");
        assert_eq!(unfiltered[1].items[0].id, "2");
        assert_eq!(unfiltered[3].items[0].id, "4");
    }

    #[test]
    fn oversized_append_leaves_existing_repositories_and_menu_unchanged() {
        let mut data = RepositoryMenuData::default();
        let mut repositories = vec![];
        data.append(&mut repositories, vec![repository("1", "team/api")])
            .unwrap();
        let too_large = repository("2", &"x".repeat(MAX_REPOSITORY_BYTES));
        assert_eq!(
            data.append(&mut repositories, vec![too_large]),
            Err(mangrove_remote::RemoteError::ResponseTooLarge)
        );
        assert_eq!(repositories.len(), 1);
        assert_eq!(data.matched_groups().len(), 1);
    }

    #[test]
    fn a_search_with_no_matches_in_loaded_pages_can_find_a_later_repository() {
        let mut data = RepositoryMenuData::default();
        let mut repositories = vec![];
        data.append(&mut repositories, vec![repository("1", "team/api")])
            .unwrap();
        data.query = "TARGET".into();
        assert!(data.matched_groups().is_empty());

        data.append(
            &mut repositories,
            vec![
                repository("2", "another/target"),
                repository("3", "team/web"),
            ],
        )
        .unwrap();
        let matched = data.matched_groups();
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].items[0].id, "2");
        assert_eq!(data.query, "TARGET");

        data.query.clear();
        assert_eq!(data.matched_groups()[0].items[0].id, "1");
        assert_eq!(repositories.len(), 3);
    }

    #[test]
    fn the_repository_count_limit_accepts_duplicates_but_rejects_new_items_atomically() {
        let mut data = RepositoryMenuData::default();
        let mut repositories = vec![];
        let page = (0..MAX_REPOSITORIES)
            .map(|id| repository(&id.to_string(), "team/api"))
            .collect();
        data.append(&mut repositories, page).unwrap();
        data.append(&mut repositories, vec![repository("0", "team/renamed")])
            .unwrap();

        assert_eq!(
            data.append(&mut repositories, vec![repository("new", "team/new")]),
            Err(mangrove_remote::RemoteError::ResponseTooLarge)
        );
        assert_eq!(repositories.len(), MAX_REPOSITORIES);
        assert_eq!(data.matched_groups()[0].items.len(), MAX_REPOSITORIES);
        assert_eq!(repositories[0].full_name, "team/api");
    }
}
