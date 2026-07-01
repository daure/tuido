use std::error::Error;

use ratatui::layout::Constraint;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use tuicore::{
    Button, CellContext, Column, DataView, Dialog, DialogBackdrop, DialogCloseReason, DialogHost,
    DialogLayer, Dropdown, DropdownSearchMode, Flex, FlexItem, Grid, GridItem, GridTrack, Panel,
    SelectionGlyphs, SelectionMode, SelectionTrigger, Tab, Tabs, TabsVariant, TextInput, Toggle,
};
use tuido::app_keymap::{self, keys};

#[derive(Debug)]
enum Msg {
    OpenDialog(&'static str),
    CloseDialog(DialogCloseReason),
}

#[derive(Clone)]
struct Item {
    id: &'static str,
    state: &'static str,
    kind: &'static str,
    size: &'static str,
    date: &'static str,
    context: &'static str,
    disabled: &'static str,
}

#[derive(Clone)]
struct Node {
    id: &'static str,
    label: &'static str,
}

#[derive(Clone)]
struct Choice {
    id: &'static str,
    label: &'static str,
}

fn main() -> Result<(), Box<dyn Error>> {
    tuicore::try_init()?;
    app_keymap::try_init()?;
    let root = DialogLayer::new(screen(), modal()).active(false);
    tuicore::TreeApp::new(root)
        .on_message(|root, msg, ctx| match msg {
            Msg::OpenDialog(title) => {
                root.layer_mut().dialog_mut().set_top_left(title);
                root.set_backdrop(DialogBackdrop::dim().amount(0.5));
                root.set_active_with_dialog_focus(true, ctx);
            }
            Msg::CloseDialog(reason) => {
                let _ = reason;
                root.set_active_with_context(false, ctx);
            }
        })
        .run()?;
    Ok(())
}

fn screen() -> Flex<Msg> {
    let header = Panel::new()
        .top_left("Context command bar")
        .bottom_left(format!(
            "{}person alice • {}project launch • {}returned • {}due • simulated parser",
            keys::COMMAND_BAR.label(),
            keys::COMMAND_BAR.label(),
            keys::FILTER_PREFIX.label(),
            keys::FILTER_PREFIX.label()
        ))
        .host(
            TextInput::new()
                .placeholder(format!(
                    "{}person alice {}due",
                    keys::COMMAND_BAR.label(),
                    keys::FILTER_PREFIX.label()
                ))
                .hotkey(keys::COMMAND_BAR.hotkey())
                .max_len(120),
        );
    let filters = Panel::new().top_left("Filters").host(filter_row());
    let body = Grid::new()
        .columns([
            GridTrack::percent(22),
            GridTrack::percent(53),
            GridTrack::percent(25),
        ])
        .rows([GridTrack::fill(1)])
        .gap(1, 0)
        .child("tree", context_tree(), GridItem::new(0, 0))
        .child("items", item_panel(), GridItem::new(0, 1))
        .child("preview", preview_panel(), GridItem::new(0, 2));
    let actions = Panel::new()
        .top_left("State scoped operations")
        .bottom_left("Raw → clarify only • Actions → pull/snooze/focus • Notes excluded from board")
        .host(
            Flex::row()
                .gap(2)
                .child(
                    "palette",
                    Button::new("Action palette")
                        .hotkey(keys::ACTION_PALETTE_BUTTON.hotkey())
                        .on_press(|| Msg::OpenDialog("Action palette")),
                    FlexItem::fixed(18),
                )
                .child(
                    "confirm",
                    Button::new("Archive confirm")
                        .hotkey(keys::ARCHIVE_CONFIRM_BUTTON.hotkey())
                        .on_press(|| Msg::OpenDialog("Typed archive confirmation")),
                    FlexItem::fixed(20),
                )
                .child(
                    "snooze",
                    Button::new("Bulk snooze")
                        .hotkey(keys::BULK_SNOOZE_BUTTON.hotkey())
                        .on_press(|| Msg::OpenDialog("Bulk snooze operation form")),
                    FlexItem::fixed(16),
                )
                .child(
                    "focus",
                    Button::new("Pull/focus")
                        .hotkey(keys::PULL_FOCUS_BUTTON.hotkey())
                        .on_press(|| Msg::OpenDialog("Operation plan")),
                    FlexItem::fixed(14),
                ),
        );
    Flex::column()
        .gap(1)
        .child("header", header, FlexItem::fixed(3))
        .child("filters", filters, FlexItem::fixed(3))
        .child("body", body, FlexItem::fill(1))
        .child("actions", actions, FlexItem::fixed(3))
}

fn filter_row() -> Flex<Msg> {
    Flex::row()
        .gap(2)
        .child(
            "state",
            dropdown_single("State", status_choices(), "all"),
            FlexItem::fixed(24),
        )
        .child(
            "context",
            dropdown_multi("Context", context_choices(), ["alice", "launch"]),
            FlexItem::fixed(32),
        )
        .child(
            "future",
            Toggle::new("show future-start")
                .checked(true)
                .hotkey(keys::SHOW_FUTURE_TOGGLE.hotkey()),
            FlexItem::fixed(20),
        )
        .child(
            "snoozed",
            Toggle::new("snoozed counts only").hotkey(keys::SNOOZED_FILTER_TOGGLE.hotkey()),
            FlexItem::fixed(22),
        )
        .child(
            "returned",
            Toggle::new(format!(
                "{}returned filter active",
                keys::FILTER_PREFIX.label()
            ))
            .hotkey(keys::FILTER_PREFIX.hotkey()),
            FlexItem::fill(1),
        )
}

fn context_tree() -> impl tuicore::TuiNode<Msg> {
    Panel::new()
        .top_left("Contexts")
        .hotkey(keys::CONTEXTS_PANEL.hotkey())
        .host(
            DataView::list(context_nodes(), |row| row.id, |row| row.label.to_string())
                .selection_mode(SelectionMode::Single)
                .selection_trigger(SelectionTrigger::OnNavigate),
        )
}

fn item_panel() -> impl tuicore::TuiNode<Msg> {
    Panel::new()
        .top_left("Items")
        .bottom_left(format!(
            "{} toggles batch selection; disabled reason column explains invalid ops",
            tuicore::keybindings().data_view().toggle_selection_label()
        ))
        .host(
            DataView::new(items(), |row| row.id)
                .headers(true)
                .selection_mode(SelectionMode::Multi)
                .selection_glyphs(SelectionGlyphs::ASCII)
                .selection_trigger(SelectionTrigger::OnActivate)
                .columns(item_columns()),
        )
}

fn preview_panel() -> impl tuicore::TuiNode<Msg> {
    Panel::new().top_left("Preview").host(
        Tabs::new(vec![
            Tab::text("Detail", "Selected: raw Slack lead. Parsed context: Alice, Launch. Raw is untrusted and can only open clarify.").hotkey(keys::DETAIL_TAB.hotkey()),
            Tab::text("AI Evidence", "Mock extraction found a person mention, project tag, and due phrase. Suggestion remains advisory until user confirms.").hotkey(keys::AI_EVIDENCE_TAB.hotkey()),
            Tab::text("Relationships", "Alice → Launch → CRM account. Team Sales returned item due to missing owner.").hotkey(keys::RELATIONSHIPS_TAB.hotkey()),
            Tab::text("Operation Plan", "Batch has mixed states. Plan stages valid actions and lists skipped rows with reasons before confirmation.").hotkey(keys::OPERATION_PLAN_TAB.hotkey()),
        ])
        .variant(TabsVariant::Underline)
        .bordered(true),
    )
}

fn modal() -> DialogHost<Tabs<Msg>, Msg> {
    Dialog::new()
        .top_left("Action palette")
        .bottom_left(format!("{} closes", keys::DIALOG_CLOSE.label()))
        .bottom_right("valid actions only")
        .on_close(Msg::CloseDialog)
        .host(
            Tabs::new(vec![
                Tab::new("Palette", action_palette()).hotkey(keys::ACTION_PALETTE_BUTTON.hotkey()),
                Tab::new("Confirm", confirm_form()).hotkey(keys::CONFIRM_TAB.hotkey()),
                Tab::new("Snooze", snooze_form()).hotkey(keys::SNOOZE_TAB.hotkey()),
            ])
            .variant(TabsVariant::Boxed)
            .bordered(true),
        )
}

fn action_palette() -> Flex<Msg> {
    Flex::column()
        .gap(1)
        .child(
            "query",
            TextInput::new()
                .placeholder(format!(
                    "{}archive {} clarify {} pull {} focus",
                    keys::COMMAND_BAR.label(),
                    keys::FILTER_PREFIX.label(),
                    keys::FILTER_PREFIX.label(),
                    keys::FILTER_PREFIX.label()
                ))
                .hotkey(keys::COMMAND_BAR.hotkey()),
            FlexItem::fixed(1),
        )
        .child(
            "commands",
            DataView::new(action_rows(), |row| row.id)
                .headers(true)
                .columns(item_columns()),
            FlexItem::fill(1),
        )
}

fn confirm_form() -> Flex<Msg> {
    Flex::column()
        .gap(1)
        .child(
            "typed",
            TextInput::new()
                .placeholder("Type ARCHIVE to commit staged destructive operation")
                .hotkey(keys::ARCHIVE_CONFIRM_TEXT.hotkey()),
            FlexItem::fixed(1),
        )
        .child(
            "plan",
            Panel::new().top_left("Staged commit").content([
                "3 selected; 1 raw skipped from archive until clarified.",
                "No state mutation happens until confirm text matches.",
            ]),
            FlexItem::fill(1),
        )
}

fn snooze_form() -> Flex<Msg> {
    Flex::column()
        .gap(1)
        .child(
            "date",
            dropdown_single("Return", date_choices(), "mon"),
            FlexItem::fixed(3),
        )
        .child(
            "reason",
            TextInput::new()
                .placeholder("Reason for bulk snooze")
                .hotkey(keys::SNOOZE_REASON_FIELD.hotkey()),
            FlexItem::fixed(1),
        )
        .child(
            "details",
            Panel::new().top_left("Operation form").content([
                "Raw rows disabled: cannot snooze.",
                "Snoozed rows hidden until return; count can remain visible.",
            ]),
            FlexItem::fill(1),
        )
}

fn dropdown_single(
    label: &'static str,
    rows: Vec<Choice>,
    selected: &'static str,
) -> Dropdown<Choice, &'static str> {
    Dropdown::single(rows, |row| row.id, |row| row.label.to_string())
        .label(label)
        .selected_one(selected)
        .search_mode(DropdownSearchMode::Contains)
}

fn dropdown_multi<const N: usize>(
    label: &'static str,
    rows: Vec<Choice>,
    selected: [&'static str; N],
) -> Dropdown<Choice, &'static str> {
    Dropdown::multi(rows, |row| row.id, |row| row.label.to_string())
        .label(label)
        .placeholder("contexts")
        .selected(selected)
        .search_mode(DropdownSearchMode::Contains)
}

fn item_columns() -> Vec<Column<Item, &'static str>> {
    vec![
        Column::rich(
            "state",
            "State",
            Constraint::Percentage(14),
            |row: &Item, _: &CellContext<&'static str>| {
                let theme = tuicore::theme();
                let color = match row.state {
                    "ACTION" => theme.success_fg(),
                    "RAW" => theme.error_fg(),
                    "RETURNED" => theme.warning_fg(),
                    _ => theme.muted_fg(),
                };
                Line::from(Span::styled(
                    row.state.to_string(),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ))
            },
        ),
        Column::text("kind", "Kind", Constraint::Percentage(12), |row: &Item| {
            row.kind.to_string()
        }),
        Column::text("size", "Size", Constraint::Percentage(9), |row: &Item| {
            row.size.to_string()
        }),
        Column::text(
            "date",
            "Due/start",
            Constraint::Percentage(16),
            |row: &Item| row.date.to_string(),
        ),
        Column::text(
            "context",
            "Context",
            Constraint::Percentage(24),
            |row: &Item| row.context.to_string(),
        ),
        Column::text(
            "disabled",
            "Disabled reason",
            Constraint::Percentage(25),
            |row: &Item| row.disabled.to_string(),
        ),
    ]
}

fn context_nodes() -> Vec<Node> {
    vec![
        Node {
            id: "inbox",
            label: "Inbox (raw 3)",
        },
        Node {
            id: "returned",
            label: "Returned (2)",
        },
        Node {
            id: "people",
            label: "People / Alice / Carter",
        },
        Node {
            id: "teams",
            label: "Teams / Sales / Legal",
        },
        Node {
            id: "systems",
            label: "Systems / CRM / Mail",
        },
        Node {
            id: "projects",
            label: "Projects / Launch / Renewal",
        },
    ]
}

fn items() -> Vec<Item> {
    vec![
        Item {
            id: "r1",
            state: "RAW",
            kind: "lead",
            size: "?",
            date: "due Fri?",
            context: "Alice / Launch",
            disabled: "clarify first",
        },
        Item {
            id: "a1",
            state: "ACTION",
            kind: "email",
            size: "S",
            date: "start today",
            context: "Carter / Legal",
            disabled: "",
        },
        Item {
            id: "n1",
            state: "NOTE",
            kind: "note",
            size: "-",
            date: "none",
            context: "Renewal",
            disabled: "notes excluded from board",
        },
        Item {
            id: "ret1",
            state: "RETURNED",
            kind: "task",
            size: "M",
            date: "visible now",
            context: "Sales",
            disabled: "ack marker before focus",
        },
        Item {
            id: "s1",
            state: "SNOOZED",
            kind: "call",
            size: "S",
            date: "returns Mon",
            context: "Ops",
            disabled: "hidden until return",
        },
    ]
}

fn action_rows() -> Vec<Item> {
    vec![
        Item {
            id: "clarify",
            state: "VALID",
            kind: "cmd",
            size: "-",
            date: "now",
            context: "raw",
            disabled: "",
        },
        Item {
            id: "pull",
            state: "VALID",
            kind: "cmd",
            size: "-",
            date: "now",
            context: "action",
            disabled: "",
        },
        Item {
            id: "snooze",
            state: "VALID",
            kind: "cmd",
            size: "-",
            date: "form",
            context: "action",
            disabled: "",
        },
        Item {
            id: "rawpull",
            state: "SKIP",
            kind: "cmd",
            size: "-",
            date: "never",
            context: "raw",
            disabled: "raw cannot pull",
        },
    ]
}

fn status_choices() -> Vec<Choice> {
    vec![
        Choice {
            id: "all",
            label: "All visible",
        },
        Choice {
            id: "raw",
            label: "Raw",
        },
        Choice {
            id: "action",
            label: "Action",
        },
        Choice {
            id: "returned",
            label: "Returned",
        },
    ]
}

fn context_choices() -> Vec<Choice> {
    vec![
        Choice {
            id: "alice",
            label: "Alice",
        },
        Choice {
            id: "launch",
            label: "Launch",
        },
        Choice {
            id: "legal",
            label: "Legal",
        },
        Choice {
            id: "crm",
            label: "CRM",
        },
    ]
}

fn date_choices() -> Vec<Choice> {
    vec![
        Choice {
            id: "tom",
            label: "Tomorrow",
        },
        Choice {
            id: "mon",
            label: "Monday",
        },
        Choice {
            id: "next",
            label: "Next week",
        },
    ]
}
