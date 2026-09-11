use std::{cell::RefCell, rc::Rc};

use crate::{
    domain::{AppEvent, AppState, WorkspaceSnapshot, reduce_app_state},
    persistence_coordinator::AppStore,
    service::{NoteView, Versioned},
};
use ratatui::{Terminal, backend::TestBackend, layout::Rect};
use tuicore::{LayoutCtx, Store, TuiNode};

use super::NotesWorkspace;

#[test]
fn note_persistence_error_is_visible_in_notes_workspace() {
    let mut state = AppState::from_snapshot(WorkspaceSnapshot {
        tasks: Vec::new(),
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    state.notes = vec![Versioned {
        revision: 1,
        value: NoteView {
            id: "note-1".into(),
            position: 0,
            content: "content".into(),
            created_at: String::new(),
            updated_at: String::new(),
        },
    }];
    state.notes_version = 1;
    let store: AppStore = Rc::new(RefCell::new(Store::new(
        state,
        reduce_app_state as fn(&mut AppState, AppEvent) -> tuicore::DispatchOutcome,
    )));
    store.borrow_mut().dispatch(AppEvent::NoteDeleteFailed {
        note: Versioned {
            revision: 1,
            value: NoteView {
                id: "restored".into(),
                position: -1,
                content: String::new(),
                created_at: String::new(),
                updated_at: String::new(),
            },
        },
        index: 0,
        error: "database unavailable".into(),
    });
    let mut workspace = NotesWorkspace::<()>::new().note_store(store);
    let area = Rect::new(0, 0, 100, 30);
    workspace.layout(area, &mut LayoutCtx::new());
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();

    terminal
        .draw(|frame| workspace.render(frame, area, &mut tuicore::RenderCtx::new()))
        .unwrap();

    let text = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(text.contains("Note delete failed: database unavailable"));
}

#[test]
fn note_panel_shows_its_creation_time_in_the_top_right() {
    let mut state = AppState::from_snapshot(WorkspaceSnapshot {
        tasks: Vec::new(),
        people: Vec::new(),
        workspaces: Vec::new(),
        tags: Vec::new(),
    });
    state.notes = vec![Versioned {
        revision: 1,
        value: NoteView {
            id: "note-1".into(),
            position: 0,
            content: "content".into(),
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
                .to_string(),
            updated_at: String::new(),
        },
    }];
    state.notes_version = 1;
    let store: AppStore = Rc::new(RefCell::new(Store::new(
        state,
        reduce_app_state as fn(&mut AppState, AppEvent) -> tuicore::DispatchOutcome,
    )));
    let mut workspace = NotesWorkspace::<()>::new().note_store(store);
    let area = Rect::new(0, 0, 120, 30);
    workspace.layout(area, &mut LayoutCtx::new());
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();

    terminal
        .draw(|frame| workspace.render(frame, area, &mut tuicore::RenderCtx::new()))
        .unwrap();

    let buffer = terminal.backend().buffer();
    let top_row = (0..area.width)
        .filter_map(|x| buffer.cell((x, 0)).map(|cell| cell.symbol()))
        .collect::<String>();
    let title = "A moment ago";
    let position = top_row
        .find(title)
        .expect("note creation time should render in its panel header");
    let panel_width = usize::from(area.width.div_ceil(workspace.columns as u16));
    assert!(position < panel_width);
    assert!(position + title.len() >= panel_width - 1);
}
