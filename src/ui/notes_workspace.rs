use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    hash::{Hash, Hasher},
    rc::Rc,
    time::Duration,
};

use ratatui::{Frame, layout::Rect};
use tuicore::{
    AnimationSettings, ChildKey, EventCtx, EventOutcome, EventRoute, FocusCtx, FocusId,
    FocusRequest, FocusTarget, Grid, GridItem, GridTrack, HotkeyEvent, HotkeyMatch,
    HotkeySequenceMatcher, InputChrome, InputPanelChrome, Language, LayoutCtx, LayoutProposal,
    LayoutResult, LayoutSizeHint, LifecycleCtx, Paragraph, RelativeDate, RelativeDateMode,
    ScrollContainer, ScrollOffset, TextareaInput, TextareaInputKeyBindings, TickResult, TreePath,
    TuiEvent, TuiNode,
};

use crate::{app_keymap::keys, notes_config::NoteEditingMode, persistence_coordinator::AppStore};

#[cfg(test)]
use crate::service::{NoteView, Versioned};

const PANEL_HEIGHT: u16 = 10;
const NARROW_BREAKPOINT: u16 = 100;
const DESKTOP_COLUMNS: [usize; 3] = [6, 4, 2];
const NARROW_COLUMNS: [usize; 2] = [2, 1];
const NOTE_PLACEHOLDERS: [&str; 30] = [
    "Capture a thought...",
    "What's on your mind?",
    "Start anywhere...",
    "What needs untangling?",
    "Write it down before it escapes...",
    "What are you noticing?",
    "A place for loose ends...",
    "Where should this idea go?",
    "Make room for thinking...",
    "What do you want to remember?",
    "Notes for later you...",
    "What changed today?",
    "Begin with one line...",
    "What are you working through?",
    "Jot, sketch, connect...",
    "What feels important right now?",
    "Leave yourself a trail...",
    "What would make this clearer?",
    "A thought worth keeping...",
    "What's the next small step?",
    "Collect the pieces...",
    "What don't you want to forget?",
    "Put the messy version here...",
    "What's taking up space in your head?",
    "Ideas, fragments, and sparks...",
    "What are you curious about?",
    "Give the thought somewhere to land...",
    "What needs a closer look?",
    "Save it for future you...",
    "Where does this lead?",
];

pub(crate) struct NotesWorkspace<M = ()> {
    scroll: ScrollContainer<Grid<M>, M>,
    base_columns: usize,
    columns: usize,
    panel_height: u16,
    zoom: crate::notes_config::NotesZoomLevels,
    zoom_sink: Option<Rc<dyn Fn(crate::notes_config::NotesZoomLevels) -> M>>,
    speed_read_sink: Option<Rc<dyn Fn(String) -> M>>,
    create_sink: Option<Rc<dyn Fn() -> M>>,
    command_hotkeys: Vec<String>,
    command_matcher: HotkeySequenceMatcher,
    notes: Vec<Rc<RefCell<String>>>,
    placeholders: Vec<String>,
    base_contents: Vec<String>,
    note_created_ats: Vec<String>,
    note_ids: Vec<String>,
    note_revisions: Vec<u64>,
    store: Option<AppStore>,
    source_version: u64,
    note_error: Option<String>,
    change_sink: Option<Rc<dyn Fn(NoteChange) -> M>>,
    delete_sink: Option<Rc<dyn Fn(String) -> M>>,
    quick_menu_sink: Option<Rc<dyn Fn(String) -> M>>,
    editing: Vec<Rc<Cell<bool>>>,
    focus_path: TreePath,
    focus_path_sink: Option<Rc<RefCell<TreePath>>>,
    focused_note_id: Option<String>,
    selected_target: Option<FocusTarget>,
    preserve_selected_until_focus_returns: bool,
    new_note_edit_request: Option<Rc<RefCell<Option<NewNoteEditRequest>>>>,
    focus_note_request: Option<Rc<RefCell<Option<String>>>>,
    pending_scroll_offset: Option<ScrollOffset>,
    grid_scroll_offset: ScrollOffset,
    reveal_after_rebuild: bool,
    last_layout_area: Rect,
    #[cfg(test)]
    rebuild_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NewNoteEditRequest {
    pub(crate) id: String,
    pub(crate) mode: NoteEditingMode,
    pub(crate) content: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct NoteChange {
    pub(crate) id: String,
    pub(crate) revision: u64,
    pub(crate) base_content: String,
    pub(crate) content: String,
}

impl<M: 'static> NotesWorkspace<M> {
    pub(crate) fn new() -> Self {
        let columns = 6;
        let notes = Vec::new();
        let base_contents = Vec::new();
        let editing = Vec::new();
        let command_hotkeys = vec![
            keys::NOTES_EDIT.hotkey(),
            keys::NOTES_OPEN_EDITOR.hotkey(),
            keys::NOTES_YANK.hotkey(),
            keys::NOTES_YANK_PROCESS.hotkey(),
            keys::NOTES_YANK_CLARIFY.hotkey(),
        ];
        Self {
            scroll: notes_grid(
                columns,
                PANEL_HEIGHT,
                None,
                &[],
                &[],
                &notes,
                &base_contents,
                &[],
                &[],
                &editing,
                Vec::new(),
                None,
                None,
                None,
            ),
            base_columns: columns,
            columns,
            panel_height: PANEL_HEIGHT,
            zoom: crate::notes_config::NotesZoomLevels::default(),
            zoom_sink: None,
            speed_read_sink: None,
            create_sink: None,
            command_matcher: HotkeySequenceMatcher::new(command_hotkeys.iter().cloned()),
            command_hotkeys,
            notes,
            placeholders: Vec::new(),
            base_contents,
            note_created_ats: Vec::new(),
            note_ids: Vec::new(),
            note_revisions: Vec::new(),
            store: None,
            source_version: 0,
            note_error: None,
            change_sink: None,
            delete_sink: None,
            quick_menu_sink: None,
            editing,
            focus_path: TreePath::new(),
            focus_path_sink: None,
            focused_note_id: None,
            selected_target: None,
            preserve_selected_until_focus_returns: false,
            new_note_edit_request: None,
            focus_note_request: None,
            pending_scroll_offset: None,
            grid_scroll_offset: ScrollOffset::default(),
            reveal_after_rebuild: false,
            last_layout_area: Rect::default(),
            #[cfg(test)]
            rebuild_count: 0,
        }
    }

    pub(crate) fn note_store(mut self, store: AppStore) -> Self {
        self.store = Some(store);
        self.sync_notes();
        self.rebuild_grid(false);
        self
    }

    #[cfg(test)]
    fn note_source(self, store: AppStore) -> Self {
        self.note_store(store)
    }

    pub(crate) fn on_change(mut self, sink: impl Fn(NoteChange) -> M + 'static) -> Self {
        self.change_sink = Some(Rc::new(sink));
        self.rebuild_grid(false);
        self
    }

    pub(crate) fn focus_path_sink(mut self, sink: Rc<RefCell<TreePath>>) -> Self {
        self.focus_path_sink = Some(sink);
        self
    }

    pub(crate) fn on_delete(mut self, sink: impl Fn(String) -> M + 'static) -> Self {
        self.delete_sink = Some(Rc::new(sink));
        self
    }

    pub(crate) fn on_quick_menu(mut self, sink: impl Fn(String) -> M + 'static) -> Self {
        self.quick_menu_sink = Some(Rc::new(sink));
        self
    }

    pub(crate) fn zoom_levels(mut self, zoom: crate::notes_config::NotesZoomLevels) -> Self {
        self.zoom = zoom;
        self
    }

    pub(crate) fn on_zoom_change(
        mut self,
        sink: impl Fn(crate::notes_config::NotesZoomLevels) -> M + 'static,
    ) -> Self {
        self.zoom_sink = Some(Rc::new(sink));
        self
    }

    pub(crate) fn on_speed_read(mut self, sink: impl Fn(String) -> M + 'static) -> Self {
        self.speed_read_sink = Some(Rc::new(sink));
        self.command_hotkeys.push(keys::NOTES_SPEED_READ.hotkey());
        self.command_matcher
            .set_hotkeys(self.command_hotkeys.iter().cloned());
        self.rebuild_grid(false);
        self
    }

    pub(crate) fn on_create(mut self, sink: impl Fn() -> M + 'static) -> Self {
        self.create_sink = Some(Rc::new(sink));
        self
    }

    pub(crate) fn new_note_edit_request(
        mut self,
        request: Rc<RefCell<Option<NewNoteEditRequest>>>,
    ) -> Self {
        self.new_note_edit_request = Some(request);
        self
    }

    pub(crate) fn focus_note_request(mut self, request: Rc<RefCell<Option<String>>>) -> Self {
        self.focus_note_request = Some(request);
        self
    }

    fn base_columns_for(width: u16) -> usize {
        if width < NARROW_BREAKPOINT { 2 } else { 6 }
    }

    fn available_columns(base_columns: usize) -> &'static [usize] {
        if base_columns == NARROW_COLUMNS[0] {
            &NARROW_COLUMNS
        } else {
            &DESKTOP_COLUMNS
        }
    }

    fn sync_layout(&mut self, width: u16) -> bool {
        let notes_changed = self.sync_notes();
        let focus_requested = self.take_focus_note_request();
        self.base_columns = Self::base_columns_for(width);
        let available_columns = Self::available_columns(self.base_columns);
        let columns = available_columns[self.current_zoom().min(available_columns.len() - 1)];
        let panel_height = PANEL_HEIGHT.saturating_mul(self.base_columns as u16) / columns as u16;
        if self.columns != columns || self.panel_height != panel_height {
            self.columns = columns;
            self.panel_height = panel_height;
            self.rebuild_grid(focus_requested);
            return true;
        }
        if notes_changed || focus_requested {
            self.rebuild_grid(focus_requested);
        }
        notes_changed
    }

    fn take_focus_note_request(&mut self) -> bool {
        let Some(note_id) = self
            .focus_note_request
            .as_ref()
            .and_then(|request| request.borrow_mut().take())
        else {
            return false;
        };
        if !self.note_ids.contains(&note_id) {
            return false;
        }
        self.focused_note_id = Some(note_id);
        true
    }

    fn sync_notes(&mut self) -> bool {
        let Some(store) = &self.store else {
            return false;
        };
        let store = store.borrow();
        let state = store.state();
        if self.source_version == state.notes_version
            && self.note_ids.len() == state.notes.len()
            && self.note_error == state.note_error
        {
            return false;
        }
        let previous = self
            .note_ids
            .drain(..)
            .zip(std::mem::take(&mut self.note_revisions))
            .zip(std::mem::take(&mut self.notes))
            .zip(std::mem::take(&mut self.base_contents))
            .zip(std::mem::take(&mut self.placeholders))
            .zip(std::mem::take(&mut self.editing))
            .zip(std::mem::take(&mut self.note_created_ats))
            .map(
                |(
                    (((((id, revision), content), base_content), placeholder), editing),
                    created_at,
                )| {
                    (
                        id,
                        (
                            revision,
                            content,
                            base_content,
                            placeholder,
                            editing,
                            created_at,
                        ),
                    )
                },
            )
            .collect::<HashMap<_, _>>();
        self.source_version = state.notes_version;
        self.note_error = state.note_error.clone();
        for note in &state.notes {
            self.note_ids.push(note.value.id.clone());
            self.note_created_ats.push(note.value.created_at.clone());
            let starts_editing = self.new_note_edit_request.as_ref().is_some_and(|request| {
                request
                    .borrow()
                    .as_ref()
                    .is_some_and(|request| request.id == note.value.id)
            });
            if let Some((revision, content, base_content, placeholder, editing, _)) =
                previous.get(&note.value.id)
                && editing.get()
            {
                self.note_revisions.push(
                    (note.value.content == *base_content)
                        .then_some(note.revision)
                        .unwrap_or(*revision),
                );
                self.notes.push(Rc::clone(content));
                self.base_contents.push(base_content.clone());
                self.placeholders.push(placeholder.clone());
                self.editing.push(Rc::clone(editing));
            } else {
                self.note_revisions.push(note.revision);
                self.notes
                    .push(Rc::new(RefCell::new(note.value.content.clone())));
                self.base_contents.push(note.value.content.clone());
                self.placeholders.push(
                    state
                        .note_placeholders
                        .get(&note.value.id)
                        .cloned()
                        .unwrap_or_else(|| note_placeholder(&note.value.id).to_string()),
                );
                self.editing.push(Rc::new(Cell::new(starts_editing)));
            }
        }
        self.focused_note_id = self
            .focused_note_id
            .take()
            .filter(|id| self.note_ids.contains(id));
        true
    }

    fn focused_index(&self) -> Option<usize> {
        self.focused_note_id
            .as_ref()
            .and_then(|id| self.note_ids.iter().position(|candidate| candidate == id))
    }

    fn rebuild_grid(&mut self, reveal_selected: bool) {
        #[cfg(test)]
        {
            self.rebuild_count += 1;
        }
        let focused = self.scroll.is_focused();
        self.pending_scroll_offset = Some(self.scroll.offset());
        self.grid_scroll_offset = self.scroll.offset();
        self.reveal_after_rebuild |= reveal_selected;
        self.scroll = notes_grid(
            self.columns,
            self.panel_height,
            self.scroll
                .is_focused()
                .then_some(self.focused_note_id.as_deref())
                .flatten(),
            &self.note_ids,
            &self.note_revisions,
            &self.notes,
            &self.base_contents,
            &self.placeholders,
            &self.note_created_ats,
            &self.editing,
            if self.columns == 1 {
                (0..self.notes.len()).collect()
            } else {
                visible_note_indices(
                    self.notes.len(),
                    self.columns,
                    self.panel_height,
                    self.grid_scroll_offset.y,
                    usize::from(self.last_layout_area.height),
                    self.focused_index(),
                )
            },
            self.speed_read_sink.clone(),
            self.change_sink.clone(),
            self.note_error.as_deref(),
        );
        self.scroll.set_focused(focused);
    }

    fn zoom(&mut self, event: &TuiEvent, ctx: &mut EventCtx<M>) -> Option<EventOutcome> {
        let available_columns = Self::available_columns(self.base_columns);
        let current_zoom = self.current_zoom();
        let zoom = if keys::NOTES_ZOOM_IN_EQUALS.matches(event)
            || keys::NOTES_ZOOM_IN_PLUS.matches(event)
        {
            (current_zoom + 1).min(available_columns.len() - 1)
        } else if keys::NOTES_ZOOM_OUT_MINUS.matches(event)
            || keys::NOTES_ZOOM_OUT_UNDERSCORE.matches(event)
        {
            current_zoom.saturating_sub(1)
        } else {
            return None;
        };

        if current_zoom != zoom {
            self.set_current_zoom(zoom);
            if let Some(sink) = &self.zoom_sink {
                ctx.emit(sink(self.zoom));
            }
            ctx.request_layout();
            ctx.request_redraw();
        }
        ctx.stop_propagation();
        Some(EventOutcome::Handled)
    }

    fn current_zoom(&self) -> usize {
        if self.base_columns == NARROW_COLUMNS[0] {
            self.zoom.narrow
        } else {
            self.zoom.desktop
        }
    }

    fn set_current_zoom(&mut self, zoom: usize) {
        if self.base_columns == NARROW_COLUMNS[0] {
            self.zoom.narrow = zoom;
        } else {
            self.zoom.desktop = zoom;
        }
    }

    fn move_focus(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<M>,
    ) -> Option<EventOutcome> {
        let current = self.note_index(panel_id(route.path.keys().last()?))?;
        let target = if keys::NOTES_MOVE_LEFT.matches(event) {
            current.checked_sub(1)
        } else if keys::NOTES_MOVE_DOWN.matches(event) {
            (current + self.columns < self.notes.len()).then_some(current + self.columns)
        } else if keys::NOTES_MOVE_UP.matches(event) {
            current.checked_sub(self.columns)
        } else if keys::NOTES_MOVE_RIGHT.matches(event) {
            (current + 1 < self.notes.len()).then_some(current + 1)
        } else {
            return None;
        };

        if let Some(target) = target {
            self.focused_note_id = Some(self.note_ids[target].clone());
            self.rebuild_grid(true);
            ctx.focus(FocusRequest::Path(note_path(
                self.focus_path.clone(),
                &self.note_ids[target],
            )));
            ctx.request_redraw();
        }
        ctx.stop_propagation();
        Some(EventOutcome::Handled)
    }

    fn handle_command_hotkey(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<M>,
    ) -> Option<EventOutcome> {
        if matches!(
            event,
            TuiEvent::Hotkey(HotkeyEvent::Canceled | HotkeyEvent::Commit(_))
        ) {
            self.command_matcher.cancel();
            return None;
        }
        let TuiEvent::Key(key) = event else {
            return None;
        };
        let hotkey = match self.command_matcher.on_key(*key) {
            HotkeyMatch::Ignored => return None,
            HotkeyMatch::Pending => HotkeyEvent::Pending(self.command_matcher.prefix().to_owned()),
            HotkeyMatch::Canceled => HotkeyEvent::Canceled,
            HotkeyMatch::Matched(index) => HotkeyEvent::Commit(self.command_hotkeys[index].clone()),
        };
        let event = TuiEvent::Hotkey(hotkey);
        if let Some(outcome) = self.handle_note_yank(&event, ctx) {
            return Some(outcome);
        }
        self.scroll.dispatch_event(route, &event, ctx);
        ctx.stop_propagation();
        Some(EventOutcome::Handled)
    }

    fn handle_note_yank(&self, event: &TuiEvent, ctx: &mut EventCtx<M>) -> Option<EventOutcome> {
        let note_id = self.focused_note_id.as_deref()?;
        let payload = match event {
            TuiEvent::Yank => note_reference(note_id),
            TuiEvent::Hotkey(HotkeyEvent::Commit(sequence))
                if sequence == &keys::NOTES_YANK.hotkey() =>
            {
                note_reference(note_id)
            }
            TuiEvent::Hotkey(HotkeyEvent::Commit(sequence))
                if sequence == &keys::NOTES_YANK_PROCESS.hotkey() =>
            {
                note_command(note_id, "process")
            }
            TuiEvent::Hotkey(HotkeyEvent::Commit(sequence))
                if sequence == &keys::NOTES_YANK_CLARIFY.hotkey() =>
            {
                note_command(note_id, "clarify")
            }
            _ => return None,
        };
        ctx.copy_to_clipboard(payload);
        ctx.stop_propagation();
        Some(EventOutcome::Handled)
    }
}

impl<M> NotesWorkspace<M> {
    fn note_index(&self, note_id: &str) -> Option<usize> {
        self.note_ids.iter().position(|id| id == note_id)
    }
}

fn notes_grid<M: 'static>(
    columns: usize,
    panel_height: u16,
    focused_note_id: Option<&str>,
    note_ids: &[String],
    note_revisions: &[u64],
    notes: &[Rc<RefCell<String>>],
    base_contents: &[String],
    placeholders: &[String],
    note_created_ats: &[String],
    editing: &[Rc<Cell<bool>>],
    visible_indices: Vec<usize>,
    speed_read_sink: Option<Rc<dyn Fn(String) -> M>>,
    change_sink: Option<Rc<dyn Fn(NoteChange) -> M>>,
    note_error: Option<&str>,
) -> ScrollContainer<Grid<M>, M> {
    let error_rows = usize::from(note_error.is_some());
    let rows = notes.len().max(1).div_ceil(columns);
    let mut row_tracks = if notes.is_empty() {
        vec![GridTrack::fill(1)]
    } else if columns == 1 {
        vec![GridTrack::fit_content(); rows]
    } else {
        vec![GridTrack::fixed(panel_height); rows]
    };
    if note_error.is_some() {
        row_tracks.insert(0, GridTrack::fixed(1));
    }
    let mut grid = Grid::new()
        .columns(vec![GridTrack::fill(1); columns])
        .rows(row_tracks);

    if let Some(note_error) = note_error {
        grid = grid.child(
            ChildKey::new("note-error"),
            Paragraph::new(note_error),
            GridItem::new(0, 0).span(1, columns),
        );
    }

    if notes.is_empty() {
        grid = grid.child(
            ChildKey::new("empty"),
            tuicore::SeasonalEmptyState::new("No notes captured yet"),
            GridItem::new(error_rows, 0).span(1, columns),
        );
    }
    for index in visible_indices {
        let value = notes
            .get(index)
            .cloned()
            .unwrap_or_else(|| Rc::new(RefCell::new(String::new())));
        let focused = focused_note_id == note_ids.get(index).map(String::as_str);
        let editing = editing
            .get(index)
            .cloned()
            .unwrap_or_else(|| Rc::new(Cell::new(false)));
        let textarea_rows = if columns == 1 {
            1
        } else {
            usize::from(panel_height.saturating_sub(2))
        };
        let max_textarea_rows = if columns == 1 { 18 } else { textarea_rows };
        let mut input = TextareaInput::new()
            .value(value.borrow().clone())
            .placeholder(placeholders.get(index).map_or("", String::as_str))
            .style(InputChrome::panel_chrome(InputPanelChrome::new()))
            .shared_syntax_cache(true)
            .language(Language::Markdown)
            .focused_events_before_global_hotkeys(false)
            .focused(focused)
            .min_rows(textarea_rows)
            .max_rows(max_textarea_rows);
        if let (Some(sink), Some(id)) = (change_sink.clone(), note_ids.get(index).cloned()) {
            let revision = note_revisions.get(index).copied().unwrap_or_default();
            let base_content = base_contents.get(index).cloned().unwrap_or_default();
            input = input.on_edit_end(move |content| {
                sink(NoteChange {
                    id: id.clone(),
                    revision,
                    base_content: base_content.clone(),
                    content,
                })
            });
        }
        input.set_insert_mode(editing.get());
        let mut note_input = NoteInput {
            input,
            value,
            editing,
            focused,
            number: index + 1,
            created_at: note_created_ats
                .get(index)
                .and_then(|created_at| relative_date(created_at)),
            speed_read_sink: speed_read_sink.clone(),
            area: Rect::default(),
        };
        note_input.sync_chrome();
        note_input.configure_focus(focused);
        grid = grid.child(
            panel_key(note_ids.get(index).map_or("", String::as_str)),
            note_input,
            GridItem::new(error_rows + index / columns, index % columns),
        );
    }
    ScrollContainer::vertical(grid).focus_reveal(true)
}

fn relative_date(timestamp: &str) -> Option<RelativeDate> {
    let timestamp = timestamp.parse::<i128>().ok()?;
    let timestamp = time::OffsetDateTime::from_unix_timestamp_nanos(timestamp).ok()?;
    Some(RelativeDate::new(timestamp).mode(RelativeDateMode::Distance))
}

fn visible_note_indices(
    note_count: usize,
    columns: usize,
    panel_height: u16,
    scroll_y: usize,
    viewport_height: usize,
    focused_index: Option<usize>,
) -> Vec<usize> {
    if note_count <= 100 {
        return (0..note_count).collect();
    }
    let first_row = scroll_y / usize::from(panel_height.max(1));
    let visible_rows = viewport_height.div_ceil(usize::from(panel_height.max(1))) + 2;
    let start = first_row.saturating_sub(1) * columns;
    let end = ((first_row + visible_rows) * columns).min(note_count);
    let mut indices: Vec<_> = (start..end).collect();
    if let Some(focused) = focused_index.filter(|index| *index < note_count)
        && !indices.contains(&focused)
    {
        indices.push(focused);
    }
    indices.sort_unstable();
    indices
}

pub(crate) fn note_placeholder(note_id: &str) -> &'static str {
    let mut hasher = std::hash::DefaultHasher::new();
    note_id.hash(&mut hasher);
    NOTE_PLACEHOLDERS[hasher.finish() as usize % NOTE_PLACEHOLDERS.len()]
}

pub(crate) fn note_reference(note_id: &str) -> String {
    format!("Tuido note {note_id}")
}

pub(crate) fn note_command(note_id: &str, action: &str) -> String {
    format!("Tuido note {action} {note_id}")
}

fn note_cancel_key(event: &TuiEvent) -> bool {
    let TuiEvent::Key(key) = event else {
        return false;
    };
    TextareaInputKeyBindings::default()
        .cancel
        .iter()
        .any(|binding| binding.matches(*key))
}

struct NoteInput<M> {
    input: TextareaInput<M>,
    value: Rc<RefCell<String>>,
    editing: Rc<Cell<bool>>,
    focused: bool,
    number: usize,
    created_at: Option<RelativeDate>,
    speed_read_sink: Option<Rc<dyn Fn(String) -> M>>,
    area: Rect,
}

impl<M: 'static> TuiNode<M> for NoteInput<M> {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        self.input.measure(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.area = area;
        if self.focused {
            ctx.with_focus_fallback_hotkey_sequences_status(
                FocusId::new("note"),
                area,
                [
                    keys::NOTES_YANK.hotkey(),
                    keys::NOTES_YANK_PROCESS.hotkey(),
                    keys::NOTES_YANK_CLARIFY.hotkey(),
                ],
                |ctx| self.input.layout(area, ctx),
            )
            .0
        } else {
            self.input.layout(area, ctx)
        }
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut tuicore::RenderCtx<'a>) {
        <TextareaInput<M> as TuiNode<M>>::render(&self.input, frame, area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<M>) -> EventOutcome {
        let outcome = self.input.event(event, ctx);
        self.sync_value(outcome)
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<M>,
    ) -> EventOutcome {
        let outcome = self.input.dispatch_event(route, event, ctx);
        self.sync_value(outcome)
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<M>) {
        self.configure_focus(focused);
        self.input.dispatch_focus(target, focused, ctx);
        self.editing.set(self.input.insert_mode());
    }

    fn focus_reveal_area(&self, _target: &FocusTarget) -> Option<Rect> {
        Some(self.area)
    }

    fn focus_reveal_centered(&self, target: &FocusTarget) -> bool {
        self.input.focus_reveal_centered(target)
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        let relative_date = self
            .created_at
            .as_mut()
            .map(|created_at| <RelativeDate as TuiNode<M>>::tick(created_at, dt, settings))
            .unwrap_or(TickResult::IDLE);
        if relative_date.changed {
            self.sync_chrome();
        }
        self.input.tick(dt, settings).merge(relative_date)
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<M>) {
        self.input.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<M>) {
        self.input.mount(ctx);
        if let Some(created_at) = &mut self.created_at {
            created_at.mount(ctx);
        }
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<M>) {
        self.input.unmount(ctx);
        if let Some(created_at) = &mut self.created_at {
            created_at.unmount(ctx);
        }
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<M>) {
        self.input.destroy(ctx);
        if let Some(created_at) = &mut self.created_at {
            created_at.destroy(ctx);
        }
    }
}

impl<M: 'static> NoteInput<M> {
    fn sync_chrome(&mut self) {
        let chrome = self
            .created_at
            .as_ref()
            .map_or_else(InputPanelChrome::new, |created_at| {
                InputPanelChrome::new().top_right(created_at.text())
            });
        self.input.set_style(InputChrome::panel_chrome(chrome));
    }

    fn configure_focus(&mut self, focused: bool) {
        self.focused = focused;
        self.input.set_insert_mode(self.editing.get());
        self.input.clear_action_hotkeys();
        if focused {
            self.input.set_hotkey(keys::NOTES_EDIT.hotkey());
            self.input.set_hotkey_enters_edit(true);
            self.input
                .set_editor_hotkey(keys::NOTES_OPEN_EDITOR.hotkey());
            let mut badge = format!(
                "{} · {}",
                keys::NOTES_EDIT.label(),
                keys::NOTES_OPEN_EDITOR.label(),
            );
            if let Some(sink) = self.speed_read_sink.clone() {
                self.input
                    .add_action_hotkey(keys::NOTES_SPEED_READ.hotkey(), move |value| sink(value));
                badge.push_str(&format!(" · {}", keys::NOTES_SPEED_READ.label()));
            }
            self.input
                .set_hotkey_badge(format!("{badge} · {}", self.number));
        } else {
            self.input.clear_editor_hotkey();
            self.input.set_hotkey_enters_edit(false);
            if self.number <= 12 {
                self.input.set_hotkey(self.number.to_string());
            } else {
                self.input.clear_hotkey();
            }
            self.input.set_hotkey_badge(self.number.to_string());
        }
        self.input
            .set_focused_events_before_global_hotkeys(self.editing.get());
    }

    fn sync_value(&self, outcome: EventOutcome) -> EventOutcome {
        *self.value.borrow_mut() = self.input.current_value().to_owned();
        self.editing.set(self.input.insert_mode());
        outcome
    }
}

fn panel_key(note_id: &str) -> ChildKey {
    ChildKey::new(format!("panel-{note_id}"))
}

fn panel_id(key: &ChildKey) -> &str {
    key.as_str().strip_prefix("panel-").unwrap_or_default()
}

pub(crate) fn note_path(parent: TreePath, note_id: &str) -> TreePath {
    parent.child(ChildKey::body()).child(panel_key(note_id))
}

impl<M: 'static> TuiNode<M> for NotesWorkspace<M> {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        self.scroll.measure(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        let resized = self.last_layout_area != area;
        self.last_layout_area = area;
        self.sync_layout(area.width);
        if self.grid_scroll_offset != self.scroll.offset() {
            self.rebuild_grid(false);
        }
        self.focus_path = ctx.current_path();
        if let Some(sink) = &self.focus_path_sink {
            *sink.borrow_mut() = self.focus_path.clone();
        }
        let result = self.scroll.layout(area, ctx);
        if let Some(offset) = self.pending_scroll_offset.take() {
            self.scroll.scroll_to(
                offset,
                AnimationSettings {
                    enabled: false,
                    ..AnimationSettings::default()
                },
            );
        }
        if self.scroll.is_focused()
            && (resized || std::mem::take(&mut self.reveal_after_rebuild))
            && let Some(panel) = self.focused_index()
        {
            self.scroll.scroll_to(
                ScrollOffset::new(0, panel / self.columns * usize::from(self.panel_height)),
                AnimationSettings {
                    enabled: false,
                    ..AnimationSettings::default()
                },
            );
        }
        result
    }

    fn render<'a>(&'a self, frame: &mut Frame, area: Rect, ctx: &mut tuicore::RenderCtx<'a>) {
        self.scroll.render(frame, area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<M>) -> EventOutcome {
        if let Some(outcome) = self.handle_note_yank(event, ctx) {
            return outcome;
        }
        let offset = self.scroll.offset();
        let outcome = self.scroll.event(event, ctx);
        if self.scroll.offset() != offset {
            ctx.request_layout();
        }
        outcome
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<M>,
    ) -> EventOutcome {
        if let Some(outcome) = self.handle_note_yank(event, ctx) {
            return outcome;
        }
        if self
            .focused_index()
            .is_some_and(|index| !self.editing[index].get())
            && let Some(outcome) = self.handle_command_hotkey(route, event, ctx)
        {
            return outcome;
        }
        if let TuiEvent::Hotkey(HotkeyEvent::Commit(sequence)) = event
            && sequence
                .parse::<usize>()
                .is_ok_and(|number| number > 0 && number <= self.notes.len().min(12))
        {
            ctx.stop_propagation();
            return EventOutcome::Handled;
        }
        if self
            .focused_index()
            .is_some_and(|index| !self.editing[index].get())
            && note_cancel_key(event)
        {
            ctx.stop_propagation();
            return EventOutcome::Handled;
        }
        if let Some(index) = self
            .focused_index()
            .filter(|index| self.editing[*index].get())
        {
            let offset = self.scroll.offset();
            let outcome = self.scroll.dispatch_event(route, event, ctx);
            if self.scroll.offset() != offset {
                ctx.request_layout();
            }
            if !self.editing[index].get() {
                ctx.request_layout();
            }
            return outcome;
        }
        if keys::NOTE_QUICK_CREATE.matches(event) {
            if let Some(sink) = &self.create_sink {
                ctx.emit(sink());
                ctx.stop_propagation();
                return EventOutcome::Handled;
            }
        }
        if keys::NOTES_QUICK_MENU.matches(event)
            && let Some(index) = self.focused_index()
            && let (Some(sink), Some(note_id)) = (&self.quick_menu_sink, self.note_ids.get(index))
        {
            self.preserve_selected_until_focus_returns = true;
            ctx.emit(sink(note_id.clone()));
            ctx.stop_propagation();
            return EventOutcome::Handled;
        }
        if keys::NOTES_DELETE.matches(event)
            && let Some(index) = self.focused_index()
            && let (Some(sink), Some(id)) = (&self.delete_sink, self.note_ids.get(index))
        {
            ctx.emit(sink(id.clone()));
            ctx.stop_propagation();
            return EventOutcome::Handled;
        }
        if let Some(outcome) = self.zoom(event, ctx) {
            return outcome;
        }
        if let Some(outcome) = self.move_focus(route, event, ctx) {
            return outcome;
        }
        let offset = self.scroll.offset();
        let outcome = self.scroll.dispatch_event(route, event, ctx);
        if self.scroll.offset() != offset {
            ctx.request_layout();
        }
        outcome
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<M>) {
        if focused {
            self.preserve_selected_until_focus_returns = false;
        }
        let Some(note_id) = target.path.keys().last().map(panel_id) else {
            self.scroll.dispatch_focus(target, focused, ctx);
            return;
        };
        if !focused {
            if self.preserve_selected_until_focus_returns {
                return;
            }
            if let Some(selected_target) = self.selected_target.as_ref() {
                self.scroll.dispatch_focus(selected_target, false, ctx);
            }
            return;
        }
        let previous = self.focused_note_id.clone();
        if let Some(previous_target) = self.selected_target.as_ref()
            && previous_target.path != target.path
        {
            self.scroll.dispatch_focus(previous_target, false, ctx);
        }
        self.focused_note_id = Some(note_id.to_string());
        self.selected_target = Some(target.clone());
        let edit_request = self.new_note_edit_request.as_ref().and_then(|request| {
            request
                .borrow()
                .as_ref()
                .filter(|request| request.id == note_id)
                .cloned()
        });
        if edit_request.is_some() {
            if let Some(index) = self.note_index(note_id) {
                self.editing[index].set(true);
            }
            self.new_note_edit_request
                .as_ref()
                .expect("new note edit request exists")
                .borrow_mut()
                .take();
        }
        self.scroll.dispatch_focus(target, true, ctx);
        if let Some(NewNoteEditRequest {
            mode: NoteEditingMode::External,
            content,
            ..
        }) = edit_request
        {
            let content = content.unwrap_or_else(|| {
                self.note_index(note_id)
                    .map(|index| self.notes[index].borrow().clone())
                    .unwrap_or_default()
            });
            ctx.request_external_editor_with_extension(content, 0, 0, "md");
        }
        if self.focused_note_id != previous || self.reveal_after_rebuild {
            ctx.request_layout();
        }
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        self.scroll.tick(dt, settings)
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<M>) {
        self.scroll.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<M>) {
        self.scroll.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<M>) {
        self.flush_editing_notes(ctx);
        self.scroll.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<M>) {
        self.scroll.destroy(ctx);
    }
}

impl<M: 'static> NotesWorkspace<M> {
    fn flush_editing_notes(&self, ctx: &mut LifecycleCtx<M>) {
        let Some(sink) = &self.change_sink else {
            return;
        };
        for (((id, revision), (content, base_content)), editing) in self
            .note_ids
            .iter()
            .zip(&self.note_revisions)
            .zip(self.notes.iter().zip(&self.base_contents))
            .zip(&self.editing)
        {
            if editing.get() && content.borrow().as_str() != base_content {
                ctx.emit(sink(NoteChange {
                    id: id.clone(),
                    revision: *revision,
                    base_content: base_content.clone(),
                    content: content.borrow().clone(),
                }));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{AppEvent, AppState, WorkspaceSnapshot, reduce_app_state};
    use ratatui::{Terminal, backend::TestBackend};
    use tuicore::{HotkeyEvent, Key, KeyEvent, KeyModifiers, Propagation, Store};

    fn note_source(count: usize) -> AppStore {
        let mut state = AppState::from_snapshot(WorkspaceSnapshot {
            tasks: Vec::new(),
            people: Vec::new(),
            workspaces: Vec::new(),
            tags: Vec::new(),
        });
        state.notes = (0..count)
            .map(|index| Versioned {
                revision: 1,
                value: NoteView {
                    id: format!("note-{index}"),
                    position: index as i64,
                    content: String::new(),
                    created_at: String::new(),
                    updated_at: String::new(),
                },
            })
            .collect();
        state.notes_version = 1;
        Rc::new(RefCell::new(Store::new(
            state,
            reduce_app_state as fn(&mut AppState, AppEvent) -> tuicore::DispatchOutcome,
        )))
    }

    fn test_panel_key(index: usize) -> ChildKey {
        panel_key(&format!("note-{index}"))
    }

    #[test]
    fn note_placeholder_is_stable_and_uses_an_approved_prompt() {
        let placeholder = note_placeholder("note-0");

        assert_eq!(placeholder, note_placeholder("note-0"));
        assert!(NOTE_PLACEHOLDERS.contains(&placeholder));
        assert!(NOTE_PLACEHOLDERS.contains(&"Capture a thought..."));
        assert!(
            NOTE_PLACEHOLDERS
                .iter()
                .all(|placeholder| !placeholder.contains('…'))
        );
    }

    #[test]
    fn notes_grid_uses_six_columns_on_desktop_and_two_on_small_terminals() {
        let mut workspace = NotesWorkspace::<()>::new().note_source(note_source(12));

        workspace.layout(Rect::new(0, 0, 120, 30), &mut LayoutCtx::new());
        assert_eq!(workspace.columns, 6);
        assert_eq!(
            workspace.scroll.child().child_rect(&test_panel_key(6)),
            Some(Rect::new(0, 10, 20, PANEL_HEIGHT))
        );

        workspace.layout(Rect::new(0, 0, 80, 70), &mut LayoutCtx::new());
        assert_eq!(workspace.columns, 2);
        assert_eq!(
            workspace.scroll.child().child_rect(&test_panel_key(2)),
            Some(Rect::new(0, 10, 40, PANEL_HEIGHT))
        );
    }

    #[test]
    fn hjkl_moves_focus_between_note_panels() {
        let mut workspace = NotesWorkspace::<()>::new().note_source(note_source(12));
        workspace.layout(Rect::new(0, 0, 120, 30), &mut LayoutCtx::new());

        for (from, key, to) in [
            (7, 'h', 6),
            (1, 'j', 7),
            (7, 'k', 1),
            (1, 'l', 2),
            (2, 'h', 1),
            (5, 'l', 6),
            (6, 'h', 5),
        ] {
            let route = EventRoute::new(TreePath::from_keys([
                ChildKey::body(),
                test_panel_key(from),
            ]));
            let mut ctx = EventCtx::default();
            let outcome = workspace.dispatch_event(
                &route,
                &TuiEvent::Key(KeyEvent::from(Key::Char(key))),
                &mut ctx,
            );

            assert!(outcome.handled());
            assert_eq!(
                ctx.focus_request(),
                Some(&FocusRequest::Path(TreePath::from_keys([
                    ChildKey::body(),
                    test_panel_key(to),
                ])))
            );
        }
    }

    #[test]
    fn focused_note_panels_scroll_into_view() {
        let mut workspace = NotesWorkspace::<()>::new().note_source(note_source(12));
        let mut layout = LayoutCtx::new();
        workspace.layout(Rect::new(0, 0, 120, 15), &mut layout);
        let target = layout
            .focus_targets()
            .iter()
            .find(|target| {
                target
                    .path
                    .keys()
                    .last()
                    .is_some_and(|key| key.as_str() == "panel-note-6")
            })
            .expect("second-row note should be focusable")
            .clone();
        let mut focus = FocusCtx::new(AnimationSettings {
            enabled: false,
            ..AnimationSettings::default()
        });

        workspace.dispatch_focus(&target, true, &mut focus);
        workspace.layout(Rect::new(0, 0, 120, 15), &mut LayoutCtx::new());

        assert!(workspace.scroll.offset().y > 0);
        assert!(focus.layout_requested());
    }

    #[test]
    fn requested_note_focus_materializes_and_reveals_a_virtualized_note() {
        let request = Rc::new(RefCell::new(Some("note-119".into())));
        let mut workspace = NotesWorkspace::<()>::new()
            .note_source(note_source(120))
            .focus_note_request(Rc::clone(&request));
        let area = Rect::new(0, 0, 120, 15);
        let mut layout = LayoutCtx::new();

        workspace.layout(area, &mut layout);
        let target = layout
            .focus_targets()
            .iter()
            .find(|target| target.path.keys().last() == Some(&test_panel_key(119)))
            .expect("requested virtualized note should be focusable")
            .clone();
        workspace.dispatch_focus(
            &target,
            true,
            &mut FocusCtx::new(AnimationSettings {
                enabled: false,
                ..AnimationSettings::default()
            }),
        );
        workspace.layout(area, &mut LayoutCtx::new());

        assert!(workspace.scroll.offset().y > 0);
    }

    #[test]
    fn empty_notes_message_is_centered_in_the_workspace() {
        let mut workspace = NotesWorkspace::<()>::new();
        let area = Rect::new(0, 0, 80, 30);
        workspace.layout(area, &mut LayoutCtx::new());
        let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();

        terminal
            .draw(|frame| workspace.render(frame, area, &mut tuicore::RenderCtx::new()))
            .unwrap();

        let buffer = terminal.backend().buffer();
        let message_row = (0..area.height).find(|y| {
            (0..area.width)
                .filter_map(|x| buffer.cell((x, *y)).map(|cell| cell.symbol()))
                .collect::<String>()
                .contains("No notes captured yet")
        });
        assert_eq!(message_row, Some(13));
    }

    #[test]
    fn focusing_last_note_reveals_its_full_card_including_footer() {
        let mut workspace = NotesWorkspace::<()>::new().note_source(note_source(13));
        let area = Rect::new(0, 0, 120, 15);
        let mut layout = LayoutCtx::new();
        workspace.layout(area, &mut layout);
        let target = layout
            .focus_targets()
            .iter()
            .find(|target| target.path.keys().last() == Some(&test_panel_key(12)))
            .unwrap()
            .clone();

        workspace.dispatch_focus(
            &target,
            true,
            &mut FocusCtx::new(AnimationSettings {
                enabled: false,
                ..AnimationSettings::default()
            }),
        );

        let geometry = workspace.scroll.scroll_geometry();
        assert_eq!(
            workspace.scroll.offset().y,
            geometry
                .content
                .height
                .saturating_sub(geometry.viewport.height)
        );
    }

    #[test]
    fn focused_notes_expose_actions_and_enter_or_dd_starts_editing() {
        let mut workspace = NotesWorkspace::<String>::new()
            .note_source(note_source(12))
            .on_speed_read(|value| value);
        let area = Rect::new(0, 0, 120, 15);
        let mut initial_layout = LayoutCtx::new();
        workspace.layout(area, &mut initial_layout);
        let target = initial_layout
            .focus_targets()
            .iter()
            .find(|target| target.path.keys().last() == Some(&test_panel_key(0)))
            .expect("first note should be focusable")
            .clone();
        workspace.dispatch_focus(
            &target,
            true,
            &mut FocusCtx::new(AnimationSettings {
                enabled: false,
                ..AnimationSettings::default()
            }),
        );

        let mut focused_layout = LayoutCtx::new();
        workspace.layout(area, &mut focused_layout);
        let target = focused_layout
            .focus_targets()
            .iter()
            .find(|target| target.path.keys().last() == Some(&test_panel_key(0)))
            .expect("focused note should be focusable")
            .clone();
        assert_eq!(
            target.hotkey_sequences,
            ["dd", "do", "ds", "yy", "yp", "yc"]
        );

        let route = EventRoute::new(target.path.clone());
        let mut prefix = EventCtx::default();
        assert_eq!(
            workspace.dispatch_event(
                &route,
                &TuiEvent::Key(KeyEvent::from(Key::Char('d'))),
                &mut prefix,
            ),
            EventOutcome::Handled
        );
        let mut speed_read = EventCtx::default();
        workspace.dispatch_event(
            &route,
            &TuiEvent::Hotkey(HotkeyEvent::Commit("ds".into())),
            &mut speed_read,
        );
        assert_eq!(speed_read.messages(), [""]);
        workspace.dispatch_event(
            &route,
            &TuiEvent::Key(KeyEvent::from(Key::Enter)),
            &mut EventCtx::default(),
        );
        let mut enter_layout = LayoutCtx::new();
        workspace.layout(area, &mut enter_layout);
        assert!(enter_layout.focus_targets()[0].focused_events_before_global_hotkeys);
        for key in ['-', '_', '=', '+'] {
            workspace.dispatch_event(
                &route,
                &TuiEvent::Key(KeyEvent::from(Key::Char(key))),
                &mut EventCtx::default(),
            );
        }
        assert_eq!(workspace.columns, 6);
        assert_eq!(workspace.notes[0].borrow().as_str(), "-_=+");

        workspace.dispatch_event(
            &route,
            &TuiEvent::Key(KeyEvent::from(Key::Esc)),
            &mut EventCtx::default(),
        );
        workspace.dispatch_event(
            &route,
            &TuiEvent::Hotkey(HotkeyEvent::Commit("dd".into())),
            &mut EventCtx::default(),
        );
        let mut edit_layout = LayoutCtx::new();
        workspace.layout(area, &mut edit_layout);
        assert!(edit_layout.focus_targets()[0].focused_events_before_global_hotkeys);
    }

    #[test]
    fn yanking_a_focused_note_copies_its_tuido_commands() {
        let mut workspace = NotesWorkspace::<()>::new().note_source(note_source(2));
        let area = Rect::new(0, 0, 120, 15);
        let mut layout = LayoutCtx::new();
        workspace.layout(area, &mut layout);
        let target = layout
            .focus_targets()
            .iter()
            .find(|target| target.path.keys().last() == Some(&test_panel_key(1)))
            .expect("second note should be focusable")
            .clone();
        workspace.dispatch_focus(&target, true, &mut FocusCtx::default());
        let route = EventRoute::new(target.path);

        let mut yank_ctx = EventCtx::default();
        let outcome = workspace.dispatch_event(&route, &TuiEvent::Yank, &mut yank_ctx);
        let effects = tuicore::DispatchEffects::from_event_ctx(outcome, yank_ctx);
        assert_eq!(effects.clipboard.as_deref(), Some("Tuido note note-1"));

        let mut prefix_ctx = EventCtx::default();
        workspace.dispatch_event(
            &route,
            &TuiEvent::Key(KeyEvent::from(Key::Char('y'))),
            &mut prefix_ctx,
        );
        let mut process_ctx = EventCtx::default();
        let outcome = workspace.dispatch_event(
            &route,
            &TuiEvent::Key(KeyEvent::from(Key::Char('p'))),
            &mut process_ctx,
        );
        let effects = tuicore::DispatchEffects::from_event_ctx(outcome, process_ctx);
        assert_eq!(
            effects.clipboard.as_deref(),
            Some("Tuido note process note-1")
        );
        assert_eq!(workspace.focused_index(), Some(1));

        let mut clarify_ctx = EventCtx::default();
        let outcome = workspace.dispatch_event(
            &route,
            &TuiEvent::Hotkey(HotkeyEvent::Commit(keys::NOTES_YANK_CLARIFY.hotkey())),
            &mut clarify_ctx,
        );
        let effects = tuicore::DispatchEffects::from_event_ctx(outcome, clarify_ctx);
        assert_eq!(
            effects.clipboard.as_deref(),
            Some("Tuido note clarify note-1")
        );
    }

    #[test]
    fn quick_menu_opens_only_for_a_focused_non_editing_note() {
        let mut workspace = NotesWorkspace::<String>::new()
            .note_source(note_source(1))
            .on_quick_menu(|note_id| note_id);
        let area = Rect::new(0, 0, 120, 15);
        let mut layout = LayoutCtx::new();
        workspace.layout(area, &mut layout);
        let target = layout.focus_targets()[0].clone();
        workspace.dispatch_focus(&target, true, &mut FocusCtx::default());
        let route = EventRoute::new(target.path);

        let mut quick_menu = EventCtx::default();
        assert!(
            workspace
                .dispatch_event(
                    &route,
                    &TuiEvent::Key(KeyEvent::from(Key::Char('.'))),
                    &mut quick_menu,
                )
                .handled()
        );
        assert_eq!(quick_menu.messages(), ["note-0"]);

        workspace.dispatch_event(
            &route,
            &TuiEvent::Key(KeyEvent::from(Key::Enter)),
            &mut EventCtx::default(),
        );
        let mut editing = EventCtx::default();
        workspace.dispatch_event(
            &route,
            &TuiEvent::Key(KeyEvent::from(Key::Char('.'))),
            &mut editing,
        );

        assert!(editing.messages().is_empty());
        assert_eq!(workspace.notes[0].borrow().as_str(), ".");
    }

    #[test]
    fn note_quick_menu_blur_keeps_selected_note_visually_focused() {
        let mut workspace = NotesWorkspace::<String>::new()
            .note_source(note_source(1))
            .on_quick_menu(|note_id| note_id);
        let area = Rect::new(0, 0, 120, 15);
        let mut layout = LayoutCtx::new();
        workspace.layout(area, &mut layout);
        let target = layout.focus_targets()[0].clone();
        workspace.dispatch_focus(&target, true, &mut FocusCtx::default());
        let route = EventRoute::new(target.path.clone());

        workspace.dispatch_event(
            &route,
            &TuiEvent::Key(KeyEvent::from(Key::Char('.'))),
            &mut EventCtx::default(),
        );
        workspace.dispatch_focus(&target, false, &mut FocusCtx::default());
        workspace.layout(area, &mut LayoutCtx::new());
        let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
        terminal
            .draw(|frame| workspace.render(frame, area, &mut tuicore::RenderCtx::new()))
            .unwrap();

        assert!(workspace.scroll.is_focused());
        assert_eq!(
            terminal.backend().buffer().cell((0, 0)).unwrap().fg,
            tuicore::theme().accent_fg()
        );

        workspace.dispatch_focus(&target, true, &mut FocusCtx::default());
        assert!(!workspace.preserve_selected_until_focus_returns);
    }

    #[test]
    fn editing_note_ctrl_page_keys_do_not_navigate_between_notes() {
        let mut workspace = NotesWorkspace::<()>::new().note_source(note_source(12));
        *workspace.notes[4].borrow_mut() = (0..20)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        workspace.rebuild_grid(false);
        let area = Rect::new(0, 0, 120, 15);
        let mut layout = LayoutCtx::new();
        workspace.layout(area, &mut layout);
        let target = layout
            .focus_targets()
            .iter()
            .find(|target| target.path.keys().last() == Some(&test_panel_key(4)))
            .expect("fifth note should be focusable")
            .clone();
        workspace.dispatch_focus(&target, true, &mut FocusCtx::default());
        workspace.layout(area, &mut LayoutCtx::new());
        let route = EventRoute::new(target.path);
        workspace.dispatch_event(
            &route,
            &TuiEvent::Hotkey(HotkeyEvent::Commit("dd".into())),
            &mut EventCtx::default(),
        );
        workspace.layout(area, &mut LayoutCtx::new());
        let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
        terminal
            .draw(|frame| workspace.render(frame, area, &mut tuicore::RenderCtx::new()))
            .unwrap();
        let rendered = |terminal: &Terminal<TestBackend>| {
            (0..area.height)
                .flat_map(|y| {
                    (0..area.width).filter_map(move |x| {
                        terminal
                            .backend()
                            .buffer()
                            .cell((x, y))
                            .map(|cell| cell.symbol())
                    })
                })
                .collect::<String>()
        };
        assert!(rendered(&terminal).contains("line 0"));
        let rebuild_count = workspace.rebuild_count;
        let outer_scroll = workspace.scroll.offset();
        let mut ctx = EventCtx::new(AnimationSettings {
            enabled: false,
            ..AnimationSettings::default()
        });

        let outcome = workspace.dispatch_event(
            &route,
            &TuiEvent::Key(KeyEvent {
                code: Key::Char('d'),
                modifiers: KeyModifiers::CONTROL,
            }),
            &mut ctx,
        );
        terminal
            .draw(|frame| workspace.render(frame, area, &mut tuicore::RenderCtx::new()))
            .unwrap();

        assert_eq!(outcome, EventOutcome::Handled);
        assert_eq!(workspace.focused_note_id.as_deref(), Some("note-4"));
        assert_eq!(workspace.rebuild_count, rebuild_count);
        assert_eq!(workspace.scroll.offset(), outer_scroll);
        assert!(ctx.focus_request().is_none());
        assert!(!rendered(&terminal).contains("line 0"));

        workspace.dispatch_event(
            &route,
            &TuiEvent::Key(KeyEvent {
                code: Key::Char('u'),
                modifiers: KeyModifiers::CONTROL,
            }),
            &mut EventCtx::new(AnimationSettings {
                enabled: false,
                ..AnimationSettings::default()
            }),
        );
        terminal
            .draw(|frame| workspace.render(frame, area, &mut tuicore::RenderCtx::new()))
            .unwrap();

        assert!(rendered(&terminal).contains("line 0"));
    }

    #[test]
    fn raw_note_command_keys_handle_edit_editor_and_speed_read() {
        let mut workspace = NotesWorkspace::<String>::new()
            .note_source(note_source(1))
            .on_speed_read(|value| value);
        let area = Rect::new(0, 0, 120, 15);
        let mut layout = LayoutCtx::new();
        workspace.layout(area, &mut layout);
        let target = layout.focus_targets()[0].clone();
        workspace.dispatch_focus(&target, true, &mut FocusCtx::default());
        workspace.layout(area, &mut LayoutCtx::new());
        let route = EventRoute::new(target.path.clone());

        for key in ['d', 'd'] {
            assert!(
                workspace
                    .dispatch_event(
                        &route,
                        &TuiEvent::Key(KeyEvent::from(Key::Char(key))),
                        &mut EventCtx::default(),
                    )
                    .handled()
            );
        }
        assert!(workspace.editing[0].get());

        let mut exit_ctx = EventCtx::default();
        workspace.dispatch_event(
            &route,
            &TuiEvent::Key(KeyEvent::from(Key::Esc)),
            &mut exit_ctx,
        );
        assert!(exit_ctx.layout_requested());
        let mut exited_layout = LayoutCtx::new();
        workspace.layout(area, &mut exited_layout);
        assert!(!exited_layout.focus_targets()[0].focused_events_before_global_hotkeys);
        for key in [
            KeyEvent::from(Key::Esc),
            KeyEvent {
                code: Key::Char('['),
                modifiers: KeyModifiers::CONTROL,
            },
        ] {
            let mut cancel_ctx = EventCtx::default();
            assert_eq!(
                workspace.dispatch_event(&route, &TuiEvent::Key(key), &mut cancel_ctx),
                EventOutcome::Handled
            );
            assert_eq!(cancel_ctx.propagation(), Propagation::Stopped);
        }
        let mut editor_prefix = EventCtx::default();
        assert!(
            workspace
                .dispatch_event(
                    &route,
                    &TuiEvent::Key(KeyEvent::from(Key::Char('d'))),
                    &mut editor_prefix,
                )
                .handled()
        );
        let mut editor = EventCtx::default();
        assert!(
            workspace
                .dispatch_event(
                    &route,
                    &TuiEvent::Key(KeyEvent::from(Key::Char('o'))),
                    &mut editor,
                )
                .handled()
        );
        assert!(editor.external_editor_request().is_some());

        workspace.dispatch_event(
            &route,
            &TuiEvent::Key(KeyEvent::from(Key::Esc)),
            &mut EventCtx::default(),
        );
        let mut speed_read = EventCtx::default();
        for key in ['d', 's'] {
            assert!(
                workspace
                    .dispatch_event(
                        &route,
                        &TuiEvent::Key(KeyEvent::from(Key::Char(key))),
                        &mut speed_read,
                    )
                    .handled()
            );
        }
        assert_eq!(speed_read.messages(), [""]);
    }

    #[test]
    fn note_editing_keeps_new_note_hotkey_as_text() {
        let mut workspace = NotesWorkspace::<String>::new()
            .note_source(note_source(1))
            .on_create(|| "create".into());
        let area = Rect::new(0, 0, 120, 15);
        let mut layout = LayoutCtx::new();
        workspace.layout(area, &mut layout);
        let target = layout.focus_targets()[0].clone();
        workspace.dispatch_focus(&target, true, &mut FocusCtx::default());
        workspace.layout(area, &mut LayoutCtx::new());
        let mut ctx = EventCtx::default();
        workspace.dispatch_event(
            &EventRoute::new(target.path.clone()),
            &TuiEvent::Key(KeyEvent::from(Key::Enter)),
            &mut EventCtx::default(),
        );
        assert!(workspace.editing[0].get());

        let outcome = workspace.dispatch_event(
            &EventRoute::new(target.path),
            &TuiEvent::Key(KeyEvent {
                code: Key::Char('N'),
                modifiers: KeyModifiers::SHIFT,
            }),
            &mut ctx,
        );

        assert!(outcome.handled());
        assert!(ctx.messages().is_empty());
        assert_eq!(workspace.notes[0].borrow().as_str(), "N");
    }

    #[test]
    fn first_enter_after_note_focus_stays_in_edit_mode() {
        let mut workspace = NotesWorkspace::<()>::new().note_source(note_source(1));
        let area = Rect::new(0, 0, 120, 15);
        let mut layout = LayoutCtx::new();
        workspace.layout(area, &mut layout);
        let target = layout.focus_targets()[0].clone();
        workspace.dispatch_focus(&target, true, &mut FocusCtx::default());
        let mut focused_layout = LayoutCtx::new();
        workspace.layout(area, &mut focused_layout);

        assert!(!focused_layout.focus_targets()[0].focused_events_before_global_hotkeys);
        workspace.dispatch_event(
            &EventRoute::new(target.path),
            &TuiEvent::Key(KeyEvent::from(Key::Enter)),
            &mut EventCtx::default(),
        );
        let mut editing_layout = LayoutCtx::new();
        workspace.layout(area, &mut editing_layout);

        assert!(workspace.editing[0].get());
        assert!(editing_layout.focus_targets()[0].focused_events_before_global_hotkeys);
    }

    #[test]
    fn focused_note_allows_global_hotkeys_until_editing_starts() {
        let mut workspace = NotesWorkspace::<()>::new().note_source(note_source(1));
        let area = Rect::new(0, 0, 120, 15);
        let mut layout = LayoutCtx::new();
        workspace.layout(area, &mut layout);
        let target = layout.focus_targets()[0].clone();
        workspace.dispatch_focus(&target, true, &mut FocusCtx::default());
        workspace.layout(area, &mut layout);

        assert!(!layout.focus_targets()[0].focused_events_before_global_hotkeys);
    }

    #[test]
    fn new_note_focus_starts_configured_editing_mode() {
        let source = note_source(1);
        let request = Rc::new(RefCell::new(Some(NewNoteEditRequest {
            id: "note-0".into(),
            mode: NoteEditingMode::External,
            content: None,
        })));
        let mut workspace = NotesWorkspace::<()>::new()
            .note_source(source)
            .new_note_edit_request(request);
        *workspace.notes[0].borrow_mut() = "Existing note content".into();
        let area = Rect::new(0, 0, 120, 15);
        let mut layout = LayoutCtx::new();
        workspace.layout(area, &mut layout);
        let target = layout.focus_targets()[0].clone();
        let mut focus = FocusCtx::default();

        workspace.dispatch_focus(&target, true, &mut focus);

        assert!(workspace.editing[0].get());
        assert_eq!(
            focus
                .external_editor_request()
                .and_then(|request| request.file_extension.as_deref()),
            Some("md")
        );
        assert_eq!(
            focus
                .external_editor_request()
                .map(|request| request.value.as_str()),
            Some("Existing note content")
        );
    }

    #[test]
    fn numeric_note_hotkey_requests_focus_without_entering_edit_mode() {
        let mut workspace = NotesWorkspace::<()>::new().note_source(note_source(2));
        let area = Rect::new(0, 0, 120, 15);
        let mut layout = LayoutCtx::new();
        workspace.layout(area, &mut layout);
        let target = layout
            .focus_targets()
            .iter()
            .find(|target| target.path.keys().last() == Some(&test_panel_key(1)))
            .unwrap()
            .clone();
        let mut ctx = EventCtx::default();
        workspace.dispatch_focus(&target, true, &mut FocusCtx::default());

        workspace.dispatch_event(
            &EventRoute::new(target.path.clone()),
            &TuiEvent::Hotkey(HotkeyEvent::Commit("2".into())),
            &mut ctx,
        );

        assert!(!workspace.editing[1].get());
    }

    #[test]
    fn ctrl_x_on_focused_note_emits_its_id_for_confirmation() {
        let mut workspace = NotesWorkspace::<String>::new()
            .note_source(note_source(2))
            .on_delete(|id| id);
        let area = Rect::new(0, 0, 120, 15);
        let mut layout = LayoutCtx::new();
        workspace.layout(area, &mut layout);
        let target = layout
            .focus_targets()
            .iter()
            .find(|target| target.path.keys().last() == Some(&test_panel_key(0)))
            .unwrap()
            .clone();
        workspace.dispatch_focus(&target, true, &mut FocusCtx::default());
        let mut ctx = EventCtx::default();

        workspace.dispatch_event(
            &EventRoute::new(target.path),
            &TuiEvent::Key(KeyEvent {
                code: Key::Char('x'),
                modifiers: KeyModifiers::CONTROL,
            }),
            &mut ctx,
        );

        assert_eq!(ctx.messages(), ["note-0"]);
    }

    #[test]
    fn opening_delete_confirmation_preserves_selected_note_and_scroll_offset() {
        let mut workspace = NotesWorkspace::<String>::new()
            .note_source(note_source(12))
            .on_delete(|id| id);
        let area = Rect::new(0, 0, 120, 15);
        let mut layout = LayoutCtx::new();
        workspace.layout(area, &mut layout);
        let target = layout
            .focus_targets()
            .iter()
            .find(|target| target.path.keys().last() == Some(&test_panel_key(6)))
            .unwrap()
            .clone();
        workspace.dispatch_focus(
            &target,
            true,
            &mut FocusCtx::new(AnimationSettings {
                enabled: false,
                ..AnimationSettings::default()
            }),
        );
        let offset = workspace.scroll.offset();
        assert!(offset.y > 0);

        workspace.dispatch_focus(&target, false, &mut FocusCtx::default());
        let mut dialog_layout = LayoutCtx::new();
        workspace.layout(area, &mut dialog_layout);

        assert_eq!(workspace.focused_note_id.as_deref(), Some("note-6"));
        assert_eq!(workspace.scroll.offset(), offset);
        assert!(!workspace.scroll.is_focused());
        assert_eq!(workspace.focused_note_id.as_deref(), Some("note-6"));
        let selected = dialog_layout
            .focus_targets()
            .iter()
            .find(|candidate| candidate.path == target.path)
            .unwrap();
        assert_eq!(selected.hotkey_sequences, ["7"]);
    }

    #[test]
    fn navigating_between_notes_does_not_rebuild_markdown_inputs() {
        let mut workspace = NotesWorkspace::<()>::new().note_source(note_source(2));
        let area = Rect::new(0, 0, 120, 15);
        let mut layout = LayoutCtx::new();
        workspace.layout(area, &mut layout);
        let first = layout.focus_targets()[0].clone();
        let second = layout.focus_targets()[1].clone();
        workspace.dispatch_focus(&first, true, &mut FocusCtx::default());
        let rebuild_count = workspace.rebuild_count;

        workspace.dispatch_focus(&first, false, &mut FocusCtx::default());
        workspace.dispatch_focus(&second, true, &mut FocusCtx::default());

        assert_eq!(workspace.rebuild_count, rebuild_count);
        assert_eq!(workspace.focused_note_id.as_deref(), Some("note-1"));
    }

    #[test]
    fn leaving_note_edit_mode_emits_revisioned_content_change() {
        let mut workspace = NotesWorkspace::<NoteChange>::new()
            .note_source(note_source(1))
            .on_change(|change| change);
        let area = Rect::new(0, 0, 120, 15);
        let mut layout = LayoutCtx::new();
        workspace.layout(area, &mut layout);
        let target = layout.focus_targets()[0].clone();
        workspace.dispatch_focus(&target, true, &mut FocusCtx::default());
        let route = EventRoute::new(target.path);
        workspace.dispatch_event(
            &route,
            &TuiEvent::Key(KeyEvent::from(Key::Enter)),
            &mut EventCtx::default(),
        );
        workspace.dispatch_event(
            &route,
            &TuiEvent::Key(KeyEvent::from(Key::Char('x'))),
            &mut EventCtx::default(),
        );
        let mut ctx = EventCtx::default();

        workspace.dispatch_event(&route, &TuiEvent::Key(KeyEvent::from(Key::Esc)), &mut ctx);

        let change = &ctx.messages()[0];
        assert_eq!(change.id, "note-0");
        assert_eq!(change.revision, 1);
        assert_eq!(change.base_content, "");
        assert_eq!(change.content, "x");
    }

    #[test]
    fn editing_note_uses_the_created_revision_when_its_base_content_is_unchanged() {
        let source = note_source(1);
        let mut workspace = NotesWorkspace::<NoteChange>::new()
            .note_source(Rc::clone(&source))
            .on_change(|change| change);
        let area = Rect::new(0, 0, 120, 15);
        let mut layout = LayoutCtx::new();
        workspace.layout(area, &mut layout);
        let target = layout.focus_targets()[0].clone();
        let route = EventRoute::new(target.path.clone());
        workspace.dispatch_focus(&target, true, &mut FocusCtx::default());
        workspace.dispatch_event(
            &route,
            &TuiEvent::Key(KeyEvent::from(Key::Enter)),
            &mut EventCtx::default(),
        );
        workspace.dispatch_event(
            &route,
            &TuiEvent::Key(KeyEvent::from(Key::Char('x'))),
            &mut EventCtx::default(),
        );
        let mut created = source.borrow().state().notes.clone();
        created[0].revision = 2;
        source.borrow_mut().dispatch(AppEvent::NotesLoaded(created));

        workspace.layout(area, &mut LayoutCtx::new());

        assert_eq!(workspace.note_revisions[0], 2);
        let mut ctx = EventCtx::default();
        workspace.dispatch_event(&route, &TuiEvent::Key(KeyEvent::from(Key::Esc)), &mut ctx);
        assert!(
            matches!(ctx.messages(), [change] if change.revision == 2 && change.content == "x")
        );
    }

    #[test]
    fn runtime_unmount_flushes_changed_editing_note() {
        let mut workspace = NotesWorkspace::<NoteChange>::new()
            .note_source(note_source(1))
            .on_change(|change| change);
        workspace.notes[0].borrow_mut().push('x');
        workspace.editing[0].set(true);
        let mut ctx = LifecycleCtx::default();

        workspace.unmount(&mut ctx);

        assert_eq!(ctx.messages().len(), 1);
        assert_eq!(ctx.messages()[0].content, "x");
        assert_eq!(ctx.messages()[0].revision, 1);
    }

    #[test]
    fn large_note_grid_only_builds_viewport_cards_and_focused_card() {
        let visible = visible_note_indices(10_000, 6, PANEL_HEIGHT, 2_000, 30, Some(9_999));

        assert!(visible.len() <= 6 * 6 + 1);
        assert!(visible.contains(&9_999));
        assert!(
            visible
                .iter()
                .all(|index| *index >= 1_194 || *index == 9_999)
        );
    }

    #[test]
    fn refresh_retains_active_unsaved_note_by_stable_id() {
        let source = note_source(3);
        let mut workspace = NotesWorkspace::<()>::new().note_source(Rc::clone(&source));
        let area = Rect::new(0, 0, 120, 30);
        let mut layout = LayoutCtx::new();
        workspace.layout(area, &mut layout);
        let target = layout
            .focus_targets()
            .iter()
            .find(|target| target.path.keys().last() == Some(&test_panel_key(1)))
            .unwrap()
            .clone();
        workspace.dispatch_focus(&target, true, &mut FocusCtx::default());
        *workspace.notes[1].borrow_mut() = "unsaved edit".into();
        workspace.editing[1].set(true);
        {
            let mut refreshed = source.borrow().state().notes.clone();
            refreshed.insert(
                0,
                Versioned {
                    revision: 1,
                    value: NoteView {
                        id: "inserted".into(),
                        position: 0,
                        content: "inserted".into(),
                        created_at: String::new(),
                        updated_at: String::new(),
                    },
                },
            );
            refreshed.remove(1);
            source
                .borrow_mut()
                .dispatch(AppEvent::NotesLoaded(refreshed));
        }

        workspace.layout(area, &mut LayoutCtx::new());

        let index = workspace.note_index("note-1").unwrap();
        assert_eq!(workspace.focused_note_id.as_deref(), Some("note-1"));
        assert_eq!(workspace.notes[index].borrow().as_str(), "unsaved edit");
        assert!(workspace.editing[index].get());

        workspace.editing[index].set(false);
        {
            let mut refreshed = source.borrow().state().notes.clone();
            let note = refreshed
                .iter_mut()
                .find(|note| note.value.id == "note-1")
                .unwrap();
            note.value.content = "authoritative content".into();
            note.revision = 2;
            source
                .borrow_mut()
                .dispatch(AppEvent::NotesLoaded(refreshed));
        }
        workspace.layout(area, &mut LayoutCtx::new());

        let index = workspace.note_index("note-1").unwrap();
        assert_eq!(
            workspace.notes[index].borrow().as_str(),
            "authoritative content"
        );
        assert_eq!(workspace.note_revisions[index], 2);
    }

    #[test]
    fn refresh_keeps_selection_when_notes_change_before_it() {
        let source = note_source(3);
        let mut workspace = NotesWorkspace::<()>::new().note_source(Rc::clone(&source));
        let area = Rect::new(0, 0, 120, 30);
        let mut layout = LayoutCtx::new();
        workspace.layout(area, &mut layout);
        let target = layout
            .focus_targets()
            .iter()
            .find(|target| target.path.keys().last() == Some(&test_panel_key(2)))
            .unwrap()
            .clone();
        workspace.dispatch_focus(&target, true, &mut FocusCtx::default());
        {
            let mut refreshed = source.borrow().state().notes.clone();
            refreshed.insert(
                0,
                Versioned {
                    revision: 1,
                    value: NoteView {
                        id: "inserted".into(),
                        position: 0,
                        content: String::new(),
                        created_at: String::new(),
                        updated_at: String::new(),
                    },
                },
            );
            refreshed.remove(1);
            source
                .borrow_mut()
                .dispatch(AppEvent::NotesLoaded(refreshed));
        }

        workspace.layout(area, &mut LayoutCtx::new());

        assert_eq!(workspace.focused_note_id.as_deref(), Some("note-2"));
        assert_eq!(
            workspace
                .selected_target
                .as_ref()
                .map(|target| &target.path),
            Some(&target.path)
        );
    }

    #[test]
    fn selected_panel_stays_visually_focused_after_a_layout_change() {
        let mut workspace = NotesWorkspace::<()>::new().note_source(note_source(12));
        let mut narrow_layout = LayoutCtx::new();
        workspace.layout(Rect::new(0, 0, 80, 45), &mut narrow_layout);
        let target = narrow_layout
            .focus_targets()
            .iter()
            .find(|target| {
                target
                    .path
                    .keys()
                    .last()
                    .is_some_and(|key| key.as_str() == "panel-note-6")
            })
            .expect("note panel should be focusable")
            .clone();
        workspace.dispatch_focus(
            &target,
            true,
            &mut FocusCtx::new(AnimationSettings {
                enabled: false,
                ..AnimationSettings::default()
            }),
        );
        let area = Rect::new(0, 0, 120, 15);
        workspace.layout(area, &mut LayoutCtx::new());
        assert!(workspace.scroll.is_focused());
        assert_eq!(workspace.scroll.offset().y, 5);
        let mut terminal = Terminal::new(TestBackend::new(area.width, area.height))
            .expect("terminal should build");

        terminal
            .draw(|frame| workspace.render(frame, area, &mut tuicore::RenderCtx::new()))
            .expect("notes should render");

        assert_eq!(
            terminal.backend().buffer().cell((0, 10)).unwrap().fg,
            tuicore::theme().accent_fg()
        );
    }

    #[test]
    fn zoom_adjusts_note_columns_and_panel_heights() {
        let mut workspace = NotesWorkspace::<()>::new().note_source(note_source(12));
        let route = EventRoute::new(TreePath::from_keys([ChildKey::body(), test_panel_key(0)]));
        let zoom_in = TuiEvent::Key(KeyEvent::from(Key::Char('=')));
        let zoom_in_plus = TuiEvent::Key(KeyEvent::from(Key::Char('+')));
        let zoom_out = TuiEvent::Key(KeyEvent::from(Key::Char('-')));
        let zoom_out_underscore = TuiEvent::Key(KeyEvent::from(Key::Char('_')));

        workspace.layout(Rect::new(0, 0, 120, 70), &mut LayoutCtx::new());
        assert_eq!((workspace.columns, workspace.panel_height), (6, 10));
        workspace.dispatch_event(&route, &zoom_in_plus, &mut EventCtx::default());
        workspace.layout(Rect::new(0, 0, 120, 70), &mut LayoutCtx::new());
        assert_eq!((workspace.columns, workspace.panel_height), (4, 15));
        workspace.dispatch_event(&route, &zoom_in, &mut EventCtx::default());
        workspace.layout(Rect::new(0, 0, 120, 70), &mut LayoutCtx::new());
        assert_eq!((workspace.columns, workspace.panel_height), (2, 30));
        workspace.dispatch_event(&route, &zoom_out_underscore, &mut EventCtx::default());
        workspace.layout(Rect::new(0, 0, 120, 70), &mut LayoutCtx::new());
        assert_eq!((workspace.columns, workspace.panel_height), (4, 15));
        workspace.layout(Rect::new(0, 0, 80, 70), &mut LayoutCtx::new());
        workspace.dispatch_event(&route, &zoom_in_plus, &mut EventCtx::default());
        workspace.layout(Rect::new(0, 0, 80, 70), &mut LayoutCtx::new());
        assert_eq!((workspace.columns, workspace.panel_height), (1, 20));
        workspace.layout(Rect::new(0, 0, 120, 70), &mut LayoutCtx::new());
        assert_eq!((workspace.columns, workspace.panel_height), (4, 15));

        let mut small = NotesWorkspace::<()>::new().note_source(note_source(12));
        small.layout(Rect::new(0, 0, 80, 70), &mut LayoutCtx::new());
        assert_eq!((small.columns, small.panel_height), (2, 10));
        small.dispatch_event(&route, &zoom_in, &mut EventCtx::default());
        small.layout(Rect::new(0, 0, 80, 70), &mut LayoutCtx::new());
        assert_eq!((small.columns, small.panel_height), (1, 20));
        small.dispatch_event(&route, &zoom_out, &mut EventCtx::default());
        small.layout(Rect::new(0, 0, 80, 70), &mut LayoutCtx::new());
        assert_eq!((small.columns, small.panel_height), (2, 10));
    }

    #[test]
    fn single_column_notes_fit_content_up_to_eighteen_textarea_rows() {
        let mut workspace = NotesWorkspace::<()>::new().note_source(note_source(2));
        *workspace.notes[0].borrow_mut() = "short note".into();
        *workspace.notes[1].borrow_mut() = (0..19).map(|_| "line").collect::<Vec<_>>().join("\n");
        let area = Rect::new(0, 0, 80, 70);

        workspace.layout(area, &mut LayoutCtx::new());
        workspace.set_current_zoom(1);
        workspace.layout(area, &mut LayoutCtx::new());

        assert_eq!(workspace.columns, 1);
        assert_eq!(
            workspace.scroll.child().child_rect(&test_panel_key(0)),
            Some(Rect::new(0, 0, 80, 3))
        );
        assert_eq!(
            workspace.scroll.child().child_rect(&test_panel_key(1)),
            Some(Rect::new(0, 3, 80, 20))
        );
    }

    #[test]
    fn single_column_note_card_contains_wrapped_markdown_content() {
        let mut workspace = NotesWorkspace::<()>::new().note_source(note_source(1));
        *workspace.notes[0].borrow_mut() = "# Refinement — onboarding reminder\n\nTurn the account setup reminder into a calm, one-step prompt.\n\n## Decision\n- Show it only after profile completion.\n- Offer **Set up now** and **Not now**.\n- Do not block navigation.\n\n## Done when\n- Copy fits one screen.\n- Dismissal stays dismissed for 14 days.".into();
        let area = Rect::new(0, 0, 70, 30);

        workspace.layout(area, &mut LayoutCtx::new());
        workspace.set_current_zoom(1);
        workspace.layout(area, &mut LayoutCtx::new());
        let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();

        terminal
            .draw(|frame| workspace.render(frame, area, &mut tuicore::RenderCtx::new()))
            .unwrap();

        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("Done when"));
        assert!(rendered.contains("Dismissal stays dismissed"));
    }

    #[test]
    fn focused_single_column_note_reveals_its_full_card() {
        let mut workspace = NotesWorkspace::<()>::new().note_source(note_source(4));
        for note in &workspace.notes[..3] {
            *note.borrow_mut() = "# Earlier note\n\nShort content.".into();
        }
        *workspace.notes[3].borrow_mut() = "# Refinement — onboarding reminder\n\nTurn the account setup reminder into a calm, one-step prompt.\n\n## Decision\n- Show it only after profile completion.\n- Offer **Set up now** and **Not now**.\n- Do not block navigation.\n\n## Done when\n- Copy fits one screen.\n- Dismissal stays dismissed for 14 days.".into();
        let area = Rect::new(0, 0, 70, 20);

        workspace.layout(area, &mut LayoutCtx::new());
        workspace.set_current_zoom(1);
        let mut layout = LayoutCtx::new();
        workspace.layout(area, &mut layout);
        let target = layout
            .focus_targets()
            .iter()
            .find(|target| target.path.keys().last() == Some(&test_panel_key(3)))
            .unwrap()
            .clone();
        workspace.dispatch_focus(
            &target,
            true,
            &mut FocusCtx::new(AnimationSettings {
                enabled: false,
                ..AnimationSettings::default()
            }),
        );
        workspace.layout(area, &mut LayoutCtx::new());
        let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();

        terminal
            .draw(|frame| workspace.render(frame, area, &mut tuicore::RenderCtx::new()))
            .unwrap();

        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("Done when"));
        assert!(rendered.contains("Dismissal stays dismissed"));
    }

    #[test]
    fn textarea_navigation_keys_do_not_move_the_note_selection() {
        let mut workspace = NotesWorkspace::<()>::new().note_source(note_source(2));
        *workspace.notes[0].borrow_mut() = (0..20)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        workspace.rebuild_grid(false);
        let area = Rect::new(0, 0, 120, 15);
        let mut layout = LayoutCtx::new();
        workspace.layout(area, &mut layout);
        let target = layout
            .focus_targets()
            .iter()
            .find(|target| target.path.keys().last() == Some(&test_panel_key(0)))
            .unwrap()
            .clone();
        workspace.dispatch_focus(&target, true, &mut FocusCtx::default());
        let route = EventRoute::new(target.path);

        for key in [
            KeyEvent::from(Key::Home),
            KeyEvent::from(Key::End),
            KeyEvent::from(Key::PageUp),
            KeyEvent::from(Key::PageDown),
            KeyEvent {
                code: Key::Char('G'),
                modifiers: KeyModifiers::SHIFT,
            },
            KeyEvent::from(Key::Char('g')),
            KeyEvent::from(Key::Char('g')),
        ] {
            let mut ctx = EventCtx::default();

            assert_eq!(
                workspace.dispatch_event(&route, &TuiEvent::Key(key), &mut ctx),
                EventOutcome::Handled
            );
            assert_eq!(workspace.focused_note_id.as_deref(), Some("note-0"));
            assert!(ctx.focus_request().is_none());
            assert_eq!(ctx.propagation(), Propagation::Stopped);
        }
    }
}

#[cfg(test)]
#[path = "notes_workspace/tests.rs"]
mod regression_tests;
