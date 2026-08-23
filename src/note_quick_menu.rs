use std::{cell::RefCell, rc::Rc, time::Duration};

use ratatui::{Frame, layout::Rect, text::Line};
use tuicore::{
    AnimationSettings, Dropdown, DropdownCommitMode, DropdownLabelPosition, DropdownSearchMode,
    DropdownVariant, EventCtx, EventOutcome, EventRoute, FocusCtx, FocusTarget, LayoutCtx,
    LayoutProposal, LayoutResult, LayoutSizeHint, LifecycleCtx, RenderCtx, TickResult, TuiEvent,
    TuiNode, keybindings, line_width,
};

use crate::{
    app::AppMsg,
    app_keymap::keys,
    ui::notes_workspace::{note_command, note_reference},
};

const MENU_HOST_WIDTH: u16 = 46;
const MENU_HOST_HEIGHT: u16 = 14;
const MENU_FIELD_WIDTH: u16 = 36;
const MENU_POPUP_WIDTH: u16 = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum NoteQuickAction {
    CopyReference,
    CopyProcessCommand,
    CopyClarifyCommand,
    Delete,
}

impl NoteQuickAction {
    const ALL: [Self; 4] = [
        Self::CopyReference,
        Self::CopyProcessCommand,
        Self::CopyClarifyCommand,
        Self::Delete,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::CopyReference => "Copy note reference",
            Self::CopyProcessCommand => "Copy process command",
            Self::CopyClarifyCommand => "Copy clarify command",
            Self::Delete => "Delete",
        }
    }

    fn hotkey(self) -> Option<String> {
        match self {
            Self::CopyReference => Some(keys::NOTES_YANK.label()),
            Self::CopyProcessCommand => Some(keys::NOTES_YANK_PROCESS.label()),
            Self::CopyClarifyCommand => Some(keys::NOTES_YANK_CLARIFY.label()),
            Self::Delete => Some(keys::NOTES_DELETE.label()),
        }
    }

    fn menu_label(self) -> String {
        let label = self.label();
        let Some(hotkey) = self.hotkey() else {
            return label.to_string();
        };
        let gap = usize::from(MENU_POPUP_WIDTH)
            .saturating_sub(
                line_width(&Line::from(label)) + line_width(&Line::from(hotkey.as_str())),
            )
            .max(1);
        format!("{label}{}{hotkey}", " ".repeat(gap))
    }
}

struct NoteQuickOption {
    action: NoteQuickAction,
    label: String,
}

pub(crate) struct NoteQuickMenu {
    note_id: String,
    dropdown: Dropdown<NoteQuickOption, NoteQuickAction>,
    actions: Rc<RefCell<Vec<NoteQuickAction>>>,
    field_area: Rect,
}

impl NoteQuickMenu {
    pub(crate) fn new(note_id: String) -> Self {
        let actions = Rc::new(RefCell::new(Vec::new()));
        let selected_actions = Rc::clone(&actions);
        let options = NoteQuickAction::ALL.map(|action| NoteQuickOption {
            action,
            label: action.menu_label(),
        });
        let mut dropdown = Dropdown::single(
            options,
            |option| option.action,
            |option| option.label.clone(),
        )
        .variant(DropdownVariant::Filled)
        .label("Note actions")
        .label_position(DropdownLabelPosition::Inline)
        .search_mode(DropdownSearchMode::Fuzzy)
        .commit_mode(DropdownCommitMode::Explicit)
        .centered(true)
        .backdrop_amount(0.0)
        .tab_stop(false)
        .max_popup_height(10)
        .on_select(move |ids| {
            if let Some(action) = ids.first() {
                selected_actions.borrow_mut().push(*action);
            }
        });
        dropdown.open();
        Self {
            note_id,
            dropdown,
            actions,
            field_area: Rect::default(),
        }
    }

    fn drain_actions(&mut self, ctx: &mut EventCtx<AppMsg>) -> bool {
        let actions = self.actions.borrow_mut().drain(..).collect::<Vec<_>>();
        let handled = !actions.is_empty();
        for action in actions {
            let note_id = self.note_id.clone();
            match action {
                NoteQuickAction::CopyReference => {
                    ctx.copy_to_clipboard(note_reference(&note_id));
                    ctx.emit(AppMsg::CloseDialog);
                }
                NoteQuickAction::CopyProcessCommand => {
                    ctx.copy_to_clipboard(note_command(&note_id, "process"));
                    ctx.emit(AppMsg::CloseDialog);
                }
                NoteQuickAction::CopyClarifyCommand => {
                    ctx.copy_to_clipboard(note_command(&note_id, "clarify"));
                    ctx.emit(AppMsg::CloseDialog);
                }
                NoteQuickAction::Delete => ctx.emit(AppMsg::OpenDeleteNote(note_id)),
            }
        }
        handled
    }

    fn centered_field_area(&self, area: Rect) -> Rect {
        let width = MENU_FIELD_WIDTH.min(area.width);
        let hint = <Dropdown<NoteQuickOption, NoteQuickAction> as TuiNode<AppMsg>>::measure(
            &self.dropdown,
            LayoutProposal::at_most(width, area.height),
        );
        let height = hint.preferred.height.min(area.height);
        Rect::new(
            area.x.saturating_add(area.width.saturating_sub(width) / 2),
            area.y
                .saturating_add(area.height.saturating_sub(height) / 2),
            width,
            height,
        )
    }

    fn finish_event(
        &mut self,
        was_open: bool,
        outcome: EventOutcome,
        ctx: &mut EventCtx<AppMsg>,
    ) -> EventOutcome {
        let activated = self.drain_actions(ctx);
        if was_open && !self.dropdown.is_open() && !activated {
            ctx.emit(AppMsg::CloseDialog);
        }
        outcome
    }
}

impl TuiNode<AppMsg> for NoteQuickMenu {
    fn measure(&self, proposal: LayoutProposal) -> LayoutSizeHint {
        LayoutSizeHint::content(MENU_HOST_WIDTH, MENU_HOST_HEIGHT).normalized(proposal)
    }

    fn layout(&mut self, area: Rect, ctx: &mut LayoutCtx) -> LayoutResult {
        self.field_area = self.centered_field_area(area);
        <Dropdown<NoteQuickOption, NoteQuickAction> as TuiNode<AppMsg>>::layout(
            &mut self.dropdown,
            self.field_area,
            ctx,
        );
        LayoutResult::new(area)
    }

    fn render<'a>(&'a self, frame: &mut Frame, _area: Rect, ctx: &mut RenderCtx<'a>) {
        self.dropdown.render(frame, self.field_area, ctx);
    }

    fn event(&mut self, event: &TuiEvent, ctx: &mut EventCtx<AppMsg>) -> EventOutcome {
        if let TuiEvent::Key(key) = event
            && keybindings().focus().unfocus_matches(*key)
        {
            ctx.emit(AppMsg::CloseDialog);
            ctx.stop_propagation();
            return EventOutcome::Handled;
        }
        let was_open = self.dropdown.is_open();
        let outcome = self.dropdown.event(event, ctx);
        self.finish_event(was_open, outcome, ctx)
    }

    fn dispatch_event(
        &mut self,
        route: &EventRoute,
        event: &TuiEvent,
        ctx: &mut EventCtx<AppMsg>,
    ) -> EventOutcome {
        let was_open = self.dropdown.is_open();
        let outcome = self.dropdown.dispatch_event(route, event, ctx);
        self.finish_event(was_open, outcome, ctx)
    }

    fn dispatch_focus(&mut self, target: &FocusTarget, focused: bool, ctx: &mut FocusCtx<AppMsg>) {
        self.dropdown.dispatch_focus(target, focused, ctx);
    }

    fn tick(&mut self, dt: Duration, settings: AnimationSettings) -> TickResult {
        <Dropdown<NoteQuickOption, NoteQuickAction> as TuiNode<AppMsg>>::tick(
            &mut self.dropdown,
            dt,
            settings,
        )
    }

    fn init(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.dropdown.init(ctx);
    }

    fn mount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.dropdown.mount(ctx);
    }

    fn unmount(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.dropdown.unmount(ctx);
    }

    fn destroy(&mut self, ctx: &mut LifecycleCtx<AppMsg>) {
        self.dropdown.destroy(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quick_menu_copies_selected_note_process_command() {
        let mut menu = NoteQuickMenu::new("note-1".into());
        menu.actions
            .borrow_mut()
            .push(NoteQuickAction::CopyProcessCommand);
        let mut ctx = EventCtx::default();

        assert!(menu.drain_actions(&mut ctx));
        assert_eq!(ctx.clipboard_request(), Some("Tuido note process note-1"));
        assert!(matches!(ctx.messages(), [AppMsg::CloseDialog]));
    }
}
