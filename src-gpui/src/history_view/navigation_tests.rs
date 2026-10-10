use super::{HistoryEvent, HistoryView};
use crate::{history::PAGE_SIZE, theme::Palette};
use git2::{Oid, Repository, Signature, Time};
use gpui_kit::{AppContext, Entity, TestAppContext, WindowOptions, px, size, test::TestWindowExt};
use mangrove_core::{Workspace, model::RepositoryInfo};
use std::{cell::RefCell, path::PathBuf, rc::Rc, sync::Arc, time::Duration};
use tempfile::TempDir;

struct Fixture {
    root: TempDir,
    main: PathBuf,
    linked: PathBuf,
    head: String,
    newest: String,
}

fn commit(repository: &Repository, parent: Option<Oid>, time: i64, text: &str) -> Oid {
    let mut builder = repository.treebuilder(None).unwrap();
    builder
        .insert(
            "file.txt",
            repository.blob(text.as_bytes()).unwrap(),
            0o100644,
        )
        .unwrap();
    let tree = repository.find_tree(builder.write().unwrap()).unwrap();
    let signature = Signature::new("Test", "test@example.test", &Time::new(time, 0)).unwrap();
    let parent = parent.map(|oid| repository.find_commit(oid).unwrap());
    repository
        .commit(
            Some("HEAD"),
            &signature,
            &signature,
            text,
            &tree,
            &parent.iter().collect::<Vec<_>>(),
        )
        .unwrap()
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let main = root.path().join("main");
        let linked = root.path().join("linked");
        let repository = Repository::init(&main).unwrap();
        repository.set_head("refs/heads/main").unwrap();
        let mut base = commit(&repository, None, 1_699_999_000, "base");
        for index in 0..32 {
            base = commit(
                &repository,
                Some(base),
                1_699_999_001 + index,
                "older change",
            );
        }
        repository.worktree("topic", &linked, None).unwrap();
        let topic = Repository::open(&linked).unwrap();
        let head = commit(&topic, Some(base), 1_700_000_001, "worktree change");
        let mut newest = base;
        // The linked HEAD is absent from page zero of the shared history.
        for index in 0..PAGE_SIZE + 4 {
            newest = commit(
                &repository,
                Some(newest),
                1_700_000_002 + index as i64,
                &format!("newer change {index}"),
            );
        }
        Self {
            root,
            main,
            linked,
            head: head.to_string(),
            newest: newest.to_string(),
        }
    }

    fn workspace(&self) -> Arc<Workspace> {
        Arc::new(Workspace::new(self.root.path().join("cache")).unwrap())
    }
}

fn finish(engine: &Workspace, info: &RepositoryInfo) {
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        let progress = engine.index_status(&info.id).unwrap();
        assert!(progress.error.is_none(), "{:?}", progress.error);
        if progress.complete {
            return;
        }
        assert!(std::time::Instant::now() < deadline, "index timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn open_view(cx: &mut TestAppContext, engine: Arc<Workspace>) -> Entity<HistoryView> {
    cx.update(gpui_kit::init);
    cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
            cx.new(|cx| HistoryView::new(engine, Palette::default(), 0., window, cx))
        })
        .unwrap()
        .1
    })
}

fn set_repository(cx: &mut TestAppContext, view: &Entity<HistoryView>, info: RepositoryInfo) {
    let window = cx.windows()[0];
    cx.update_window(window, |_, window, cx| {
        view.update(cx, |view, cx| {
            view.set_repository(Arc::new(info), window, cx)
        });
    })
    .unwrap();
}

#[gpui_kit::test]
fn opening_a_worktree_selects_its_head_beyond_page_zero_with_cold_and_warm_indexes(
    cx: &mut TestAppContext,
) {
    let fixture = Fixture::new();
    let engine = fixture.workspace();
    let view = open_view(cx, engine.clone());
    let selections = Rc::new(RefCell::new(Vec::new()));
    let captured = selections.clone();
    let _subscription = cx.update(|cx| {
        cx.subscribe(&view, move |_, event, _| {
            if let HistoryEvent::Selected { sha, .. } = event {
                captured.borrow_mut().push(sha.clone());
            }
        })
    });
    for warm in [false, true] {
        let info = engine
            .open_repository(fixture.linked.to_str().unwrap())
            .unwrap();
        assert_eq!(info.index.complete, warm);
        assert_eq!(info.head.as_deref(), Some(fixture.head.as_str()));
        set_repository(cx, &view, info.clone());
        finish(&engine, &info);
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(350));
        cx.run_until_parked();
        let position = engine
            .commit_position(&info.id, &fixture.head)
            .unwrap()
            .unwrap();
        assert!(position >= PAGE_SIZE);
        cx.update_window(cx.windows()[0], |_, window, cx| {
            window.render_frame(cx);
            let history = view.read(cx);
            assert_eq!(history.selected_sha, fixture.head);
            assert_eq!(history.selected_index, position);
            assert!(history.pages.current(position / PAGE_SIZE));
            assert!(history.error.is_none(), "{:?}", history.error);
            let scroll = history.scroll.0.borrow();
            let target_top = px(super::ROW_HEIGHT) * position + scroll.base_handle.offset().y;
            assert!(target_top >= px(0.));
            assert!(target_top + px(super::ROW_HEIGHT) <= scroll.base_handle.bounds().size.height);
        })
        .unwrap();
        // Late panel/header layout must leave the selected commit visible.
        cx.update_window(cx.windows()[0], |_, window, cx| {
            window.resize(size(px(980.), window.viewport_size().height - px(50.)));
            window.render_frame(cx);
            let scroll = view.read(cx).scroll.0.borrow();
            let target_top = px(super::ROW_HEIGHT) * position + scroll.base_handle.offset().y;
            assert!(target_top >= px(0.));
            assert!(target_top + px(super::ROW_HEIGHT) <= scroll.base_handle.bounds().size.height);
        })
        .unwrap();
        assert_eq!(
            selections.borrow().as_slice(),
            std::slice::from_ref(&fixture.head)
        );
        view.update(cx, |view, cx| view.clear(cx));
        selections.borrow_mut().clear();
    }
}

#[gpui_kit::test]
fn refreshing_keeps_the_chosen_commit_and_replaced_openings_cannot_select_an_old_head(
    cx: &mut TestAppContext,
) {
    let fixture = Fixture::new();
    let engine = fixture.workspace();
    let view = open_view(cx, engine.clone());
    let info = engine
        .open_repository(fixture.linked.to_str().unwrap())
        .unwrap();
    finish(&engine, &info);
    set_repository(cx, &view, engine.open_repository(&info.path).unwrap());
    cx.run_until_parked();
    cx.update_window(cx.windows()[0], |_, window, cx| {
        view.update(cx, |view, cx| {
            view.jump_to(fixture.newest.clone(), window, cx)
        });
    })
    .unwrap();
    cx.run_until_parked();

    let main = Repository::open(&fixture.main).unwrap();
    let new_head = commit(
        &main,
        Some(Oid::from_str(&fixture.newest).unwrap()),
        1_700_001_000,
        "external change",
    )
    .to_string();
    let refreshed = engine.open_repository(&info.path).unwrap();
    assert_ne!(info.id, refreshed.id);
    finish(&engine, &refreshed);
    set_repository(cx, &view, engine.open_repository(&info.path).unwrap());
    cx.run_until_parked();
    cx.read(|cx| assert_eq!(view.read(cx).selected_sha, fixture.newest));

    view.update(cx, |view, cx| view.clear(cx));
    set_repository(cx, &view, engine.open_repository(&info.path).unwrap());
    // Switch before the linked worktree's position or page reads finish.
    let main_info = engine
        .open_repository(fixture.main.to_str().unwrap())
        .unwrap();
    finish(&engine, &main_info);
    set_repository(cx, &view, engine.open_repository(&main_info.path).unwrap());
    cx.run_until_parked();
    cx.read(|cx| {
        let history = view.read(cx);
        assert_eq!(history.selected_sha, new_head);
        assert_eq!(history.selected_index, 0);
        assert!(history.error.is_none(), "{:?}", history.error);
    });
}

#[gpui_kit::test]
fn opening_a_solo_history_does_not_navigate_to_a_worktree_head_outside_it(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let engine = fixture.workspace();
    let view = open_view(cx, engine.clone());
    let solo = vec!["refs/heads/main".to_owned()];
    let info = engine
        .open_repository_with_solo(fixture.linked.to_str().unwrap(), &solo)
        .unwrap();
    finish(&engine, &info);
    let info = engine.open_repository_with_solo(&info.path, &solo).unwrap();
    assert!(
        engine
            .commit_position(&info.id, &fixture.head)
            .unwrap()
            .is_none()
    );
    set_repository(cx, &view, info);
    cx.run_until_parked();
    cx.read(|cx| {
        let history = view.read(cx);
        assert_eq!(history.selected_sha, fixture.newest);
        assert_eq!(history.repository.as_ref().unwrap().solo_refs, solo);
        assert!(history.error.is_none(), "{:?}", history.error);
    });
}

fn settle(cx: &mut TestAppContext) {
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(400));
    cx.run_until_parked();
}

fn search(cx: &mut TestAppContext, view: &Entity<HistoryView>, query: &str) {
    view.update(cx, |view, cx| view.set_query(query.into(), cx));
    settle(cx);
}

fn jump(cx: &mut TestAppContext, view: &Entity<HistoryView>, sha: &str) {
    cx.update_window(cx.windows()[0], |_, window, cx| {
        view.update(cx, |view, cx| view.jump_to(sha.into(), window, cx));
    })
    .unwrap();
    settle(cx);
}

fn record_selections(
    cx: &mut TestAppContext,
    view: &Entity<HistoryView>,
) -> Rc<RefCell<Vec<String>>> {
    let selections = Rc::new(RefCell::new(Vec::new()));
    let captured = selections.clone();
    cx.update(|cx| {
        cx.subscribe(view, move |_, event, _| {
            if let HistoryEvent::Selected { sha, .. } = event {
                captured.borrow_mut().push(sha.clone());
            }
        })
        .detach()
    });
    selections
}

/// Selected commit, its list offset, and whether its row is in the viewport.
fn selection(cx: &mut TestAppContext, view: &Entity<HistoryView>) -> (String, usize, bool) {
    cx.update_window(cx.windows()[0], |_, window, cx| {
        window.render_frame(cx);
        let history = view.read(cx);
        let scroll = history.scroll.0.borrow();
        let top = px(super::ROW_HEIGHT) * history.selected_index + scroll.base_handle.offset().y;
        let visible =
            top >= px(0.) && top + px(super::ROW_HEIGHT) <= scroll.base_handle.bounds().size.height;
        (
            history.selected_sha.clone(),
            history.selected_index,
            visible,
        )
    })
    .unwrap()
}

fn open_main(
    cx: &mut TestAppContext,
    fixture: &Fixture,
) -> (Arc<Workspace>, RepositoryInfo, Entity<HistoryView>) {
    let engine = fixture.workspace();
    let view = open_view(cx, engine.clone());
    let info = engine
        .open_repository(fixture.main.to_str().unwrap())
        .unwrap();
    finish(&engine, &info);
    let info = engine.open_repository(&info.path).unwrap();
    set_repository(cx, &view, info.clone());
    settle(cx);
    (engine, info, view)
}

fn commit_at(engine: &Workspace, info: &RepositoryInfo, position: usize) -> String {
    let mut page = engine.list_commits(&info.id, position, 1, "").unwrap();
    let commit = page.commits.remove(0);
    assert_eq!(commit.summary, "older change");
    commit.sha
}

// Walk: 132 newer changes, the linked worktree commit, 32 older changes, base.
const FIRST_OLDER: usize = PAGE_SIZE + 5;

#[gpui_kit::test]
fn searching_keeps_a_matching_selection_and_clearing_reveals_it_again(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let (engine, info, view) = open_main(cx, &fixture);
    let position = FIRST_OLDER + 7;
    let target = commit_at(&engine, &info, position);
    jump(cx, &view, &target);
    assert_eq!(selection(cx, &view), (target.clone(), position, true));
    let selections = record_selections(cx, &view);

    // Every result is on page zero.
    search(cx, &view, "older");
    assert_eq!(selection(cx, &view), (target.clone(), 7, true));
    // A result beyond page zero is found by its offset.
    search(cx, &view, "change");
    assert_eq!(selection(cx, &view), (target.clone(), position, true));
    search(cx, &view, "");
    assert_eq!(selection(cx, &view), (target.clone(), position, true));
    assert!(selections.borrow().is_empty());

    // A commit outside the results gives way to the first result.
    search(cx, &view, "newer");
    assert_eq!(selection(cx, &view), (fixture.newest.clone(), 0, true));
    assert_eq!(
        selections.borrow().as_slice(),
        std::slice::from_ref(&fixture.newest)
    );
    cx.read(|cx| assert!(view.read(cx).error.is_none()));
}

#[gpui_kit::test]
fn navigating_to_a_reference_keeps_a_search_only_when_it_contains_the_commit(
    cx: &mut TestAppContext,
) {
    let fixture = Fixture::new();
    let (engine, info, view) = open_main(cx, &fixture);
    let target = commit_at(&engine, &info, FIRST_OLDER + 3);
    search(cx, &view, "older");
    jump(cx, &view, &target);
    assert_eq!(selection(cx, &view), (target.clone(), 3, true));
    cx.read(|cx| assert_eq!(view.read(cx).query, "older"));

    jump(cx, &view, &fixture.newest);
    assert_eq!(selection(cx, &view), (fixture.newest.clone(), 0, true));
    cx.read(|cx| {
        let history = view.read(cx);
        assert!(history.query.is_empty() && !history.filtered());
    });
}

#[gpui_kit::test]
fn applying_solo_replaces_a_selection_outside_the_filtered_history(cx: &mut TestAppContext) {
    let fixture = Fixture::new();
    let (engine, info, view) = open_main(cx, &fixture);
    assert_eq!(selection(cx, &view).0, fixture.newest);
    let selections = record_selections(cx, &view);
    let solo = vec!["refs/heads/topic".to_owned()];
    let soloed = engine.open_repository_with_solo(&info.path, &solo).unwrap();
    finish(&engine, &soloed);
    let soloed = engine.open_repository_with_solo(&info.path, &solo).unwrap();
    set_repository(cx, &view, soloed);
    settle(cx);
    // The topic tip is the first commit of its Solo history.
    assert_eq!(selection(cx, &view), (fixture.head.clone(), 0, true));
    assert_eq!(
        selections.borrow().as_slice(),
        std::slice::from_ref(&fixture.head)
    );
}
