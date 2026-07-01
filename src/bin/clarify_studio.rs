use std::error::Error;

use ratatui::layout::Constraint;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use tuicore::{
    Button, CellContext, Column, DataView, Dialog, DialogBackdrop, DialogCloseReason, DialogHost,
    DialogLayer, Dropdown, DropdownCommitMode, DropdownSearchMode, Flex, FlexItem, Panel,
    SelectionGlyphs, SelectionMode, SelectionTrigger, Split, Tab, Tabs, TabsVariant, TextInput,
    TextareaInput, Toggle,
};
use tuido::app_keymap::{self, keys};

#[derive(Debug)]
enum Msg {
    OpenDialog(&'static str),
    CloseDialog(DialogCloseReason),
}

#[derive(Clone)]
struct ItemRow {
    id: &'static str,
    label: &'static str,
    state: &'static str,
}

#[derive(Clone)]
struct SuggestionRow {
    id: &'static str,
    title: &'static str,
    confidence: u8,
    explanation: &'static str,
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
                root.set_backdrop(DialogBackdrop::dim().amount(0.55));
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
    let capture = Panel::new()
        .top_left("Capture raw lead")
        .bottom_left(format!(
            "{} submit • raw is untrusted until accepted • mock clock 09:42",
            tuicore::keybindings().button().press_label()
        ))
        .host(
            TextInput::new()
                .placeholder(format!(
                    "{}: paste email/slack/note; AI may suggest, never commit",
                    keys::CAPTURE_RAW_LEAD.label()
                ))
                .hotkey(keys::CAPTURE_RAW_LEAD.hotkey())
                .max_len(180),
        );

    let body = Split::horizontal(
        left_rail(),
        Split::horizontal(editor(), ai_rail()).ratio(7, 4),
    )
    .ratio(3, 9)
    .gap(1);

    let actions = Flex::row()
        .gap(2)
        .child(
            "accept",
            Button::new("Accept Split")
                .hotkey(keys::ACCEPT_SPLIT.hotkey())
                .on_press(|| Msg::OpenDialog("Commit clarified action?")),
            FlexItem::fixed(16),
        )
        .child(
            "merge",
            Button::new("Merge Selected")
                .hotkey(keys::MERGE_SELECTED.hotkey())
                .on_press(|| Msg::OpenDialog("Merge pending suggestions")),
            FlexItem::fixed(18),
        )
        .child(
            "discard",
            Button::new("Discard…")
                .hotkey(keys::DISCARD_SUGGESTION.hotkey())
                .on_press(|| Msg::OpenDialog("Discard / archive suggestion")),
            FlexItem::fixed(12),
        )
        .child(
            "pull",
            Button::new("Pull to Board")
                .hotkey(keys::PULL_TO_BOARD.hotkey())
                .on_press(|| Msg::OpenDialog("Pull allowed only after action item exists")),
            FlexItem::fixed(16),
        )
        .child(
            "snooze",
            Button::new("Snooze…")
                .hotkey(keys::SNOOZE_ACTION.hotkey())
                .on_press(|| Msg::OpenDialog("Snooze clarified action")),
            FlexItem::fixed(12),
        )
        .child(
            "palette",
            Button::new(format!("{}: commands", keys::COMMAND_PALETTE.label()))
                .hotkey(keys::COMMAND_PALETTE.hotkey())
                .on_press(|| Msg::OpenDialog("Command palette")),
            FlexItem::fixed(14),
        );

    Flex::column()
        .gap(1)
        .child("capture", capture, FlexItem::fixed(3))
        .child("body", body, FlexItem::fill(1))
        .child(
            "actions",
            Panel::new()
                .top_left("Explicit actions")
                .bottom_left(
                    "Raw buttons show denied reasons; AI suggestions stay pending until accepted",
                )
                .host(actions),
            FlexItem::fixed(3),
        )
}

fn left_rail() -> impl tuicore::TuiNode<Msg> {
    Panel::new()
        .top_left("Triage queues")
        .hotkey(keys::TRIAGE_QUEUES_PANEL.hotkey())
        .host(
            Tabs::new(vec![
                Tab::new(
                    "Raw Inbox",
                    item_list(raw_items()).hotkey(keys::RAW_INBOX_TAB.hotkey()),
                )
                .hotkey(keys::RAW_INBOX_TAB.hotkey()),
                Tab::new(
                    "Returned",
                    item_list(returned_items()).hotkey(keys::RETURNED_TAB.hotkey()),
                )
                .hotkey(keys::RETURNED_TAB.hotkey()),
                Tab::new(
                    "Actions",
                    item_table(action_items()).hotkey(keys::ACTIONS_TAB.hotkey()),
                )
                .hotkey(keys::ACTIONS_TAB.hotkey()),
                Tab::new(
                    "Notes",
                    item_list(note_items()).hotkey(keys::NOTES_TAB.hotkey()),
                )
                .hotkey(keys::NOTES_TAB.hotkey()),
            ])
            .variant(TabsVariant::Boxed)
            .bordered(true),
        )
}

fn editor() -> impl tuicore::TuiNode<Msg> {
    Panel::new().top_left("Clarification workbench").host(
        Tabs::new(vec![
            Tab::new("Clarify", clarify_tab()).hotkey(keys::CLARIFY_TAB.hotkey()),
            Tab::new("Context", context_tab()).hotkey(keys::CONTEXT_TAB.hotkey()),
            Tab::new("Dates", dates_tab()).hotkey(keys::DATES_TAB.hotkey()),
            Tab::text("AI Rationale", "Mock AI split: detected ask, owner, possible due date. User must review every field before accept. Confidence is advisory, never source of truth.").hotkey(keys::AI_RATIONALE_TAB.hotkey()),
            Tab::text("History", "09:34 raw captured\n09:35 AI suggested two actions\n09:36 returned marker detected\nNo board pull performed yet.").hotkey(keys::HISTORY_TAB.hotkey()),
        ])
        .variant(TabsVariant::Underline)
        .bordered(true),
    )
}

fn ai_rail() -> impl tuicore::TuiNode<Msg> {
    Panel::new()
        .top_left("Pending AI split suggestions")
        .bottom_left(format!(
            "{} selects candidates • confidence/explanation shown; uncommitted",
            tuicore::keybindings().data_view().toggle_selection_label()
        ))
        .host(ai_table().hotkey(keys::AI_SUGGESTIONS_TABLE.hotkey()))
}

fn clarify_tab() -> Flex<Msg> {
    Flex::column()
        .gap(1)
        .child("raw", TextareaInput::new().value("Raw: Alice asks if we can chase Carter on contract redlines and maybe schedule kickoff next Tue. Returned from Sales with missing owner.").placeholder("Raw untrusted item body").hotkey(keys::RAW_BODY_FIELD.hotkey()).max_lines(5), FlexItem::fixed(5))
        .child("title", TextInput::new().value("Clarify contract redline follow-up").placeholder("Action title").hotkey(keys::ACTION_TITLE_FIELD.hotkey()), FlexItem::fixed(1))
        .child("type", dropdown_single("Type", &[Choice { id: "action", label: "Action item" }, Choice { id: "note", label: "Note only" }, Choice { id: "raw", label: "Keep raw / needs clarification" }], "action"), FlexItem::fixed(3))
        .child("subtype", dropdown_single("Subtype", &[Choice { id: "email", label: "Email / follow-up" }, Choice { id: "meeting", label: "Meeting" }, Choice { id: "decision", label: "Decision needed" }], "email"), FlexItem::fixed(3))
        .child("reviewed", Toggle::new("AI suggestion reviewed by user").checked(true).hotkey(keys::AI_REVIEWED_TOGGLE.hotkey()), FlexItem::fixed(1))
        .child("returned", Toggle::new("Returned marker acknowledged").checked(true).hotkey(keys::RETURNED_ACK_TOGGLE.hotkey()), FlexItem::fixed(1))
        .child("guard", Panel::new().top_left("Trust guard").content(["Raw cannot snooze or pull. Only accepted action items unlock board/focus.", "Done archives immediately; snoozed hidden until return date."]), FlexItem::fill(1))
}

fn context_tab() -> Flex<Msg> {
    Flex::column()
        .gap(1)
        .child("people", dropdown_multi("People", &[Choice { id: "alice", label: "Alice" }, Choice { id: "carter", label: "Carter" }, Choice { id: "mina", label: "Mina" }], ["alice", "carter"]), FlexItem::fixed(3))
        .child("teams", dropdown_multi("Teams", &[Choice { id: "sales", label: "Sales" }, Choice { id: "legal", label: "Legal" }, Choice { id: "success", label: "Customer Success" }], ["sales", "legal"]), FlexItem::fixed(3))
        .child("systems", dropdown_multi("Systems", &[Choice { id: "crm", label: "CRM" }, Choice { id: "mail", label: "Mail" }, Choice { id: "docs", label: "Docs" }], ["crm", "docs"]), FlexItem::fixed(3))
        .child("projects", dropdown_multi("Projects", &[Choice { id: "launch", label: "Launch" }, Choice { id: "renewal", label: "Renewal" }, Choice { id: "audit", label: "Audit" }], ["renewal"]), FlexItem::fixed(3))
        .child("note", TextareaInput::new().value("Context source remains attached to raw item. Clarified action stores user-reviewed fields only.").hotkey(keys::NOTES_TAB.hotkey()), FlexItem::fill(1))
}

fn dates_tab() -> Flex<Msg> {
    Flex::column()
        .gap(1)
        .child(
            "size",
            dropdown_single(
                "Size",
                &[
                    Choice {
                        id: "big",
                        label: "Big",
                    },
                    Choice {
                        id: "medium",
                        label: "Medium",
                    },
                    Choice {
                        id: "small",
                        label: "Small",
                    },
                ],
                "medium",
            ),
            FlexItem::fixed(3),
        )
        .child(
            "start",
            dropdown_single(
                "Start",
                &[
                    Choice {
                        id: "today",
                        label: "Today",
                    },
                    Choice {
                        id: "tomorrow",
                        label: "Tomorrow",
                    },
                    Choice {
                        id: "future",
                        label: "Next week",
                    },
                ],
                "today",
            ),
            FlexItem::fixed(3),
        )
        .child(
            "due",
            dropdown_single(
                "Due",
                &[
                    Choice {
                        id: "none",
                        label: "No due date",
                    },
                    Choice {
                        id: "fri",
                        label: "Friday",
                    },
                    Choice {
                        id: "next",
                        label: "Next Tuesday",
                    },
                ],
                "next",
            ),
            FlexItem::fixed(3),
        )
        .child(
            "snooze",
            Panel::new().top_left("Date rules").content([
                "Snoozed items hide until return date.",
                "Returned marker remains until user acknowledges it.",
                "Daily Focus receives action items only.",
            ]),
            FlexItem::fill(1),
        )
}

fn modal() -> DialogHost<Tabs<Msg>, Msg> {
    Dialog::new()
        .top_left("Command palette")
        .bottom_left(format!("{} closes", keys::DIALOG_CLOSE.label()))
        .bottom_right("mock operations")
        .on_close(Msg::CloseDialog)
        .host(
            Tabs::new(vec![
                Tab::new("Commands", command_palette()).hotkey(keys::COMMAND_PALETTE.hotkey()),
                Tab::new("Snooze", snooze_form()).hotkey(keys::SNOOZE_ACTION.hotkey()),
                Tab::new("Confirm", confirm_form()).hotkey(keys::DISCARD_SUGGESTION.hotkey()),
            ])
            .variant(TabsVariant::Boxed)
            .bordered(true),
        )
}

fn command_palette() -> Flex<Msg> {
    Flex::column()
        .gap(1)
        .child(
            "query",
            TextInput::new()
                .placeholder(format!(
                    "{}split {}returned {}action pull",
                    keys::COMMAND_BAR.label(),
                    keys::FILTER_PREFIX.label(),
                    keys::FILTER_PREFIX.label()
                ))
                .hotkey(keys::COMMAND_BAR.hotkey()),
            FlexItem::fixed(1),
        )
        .child("list", command_table(), FlexItem::fill(1))
}

fn snooze_form() -> Flex<Msg> {
    Flex::column()
        .gap(1)
        .child(
            "date",
            dropdown_single(
                "Return date",
                &[
                    Choice {
                        id: "tomorrow",
                        label: "Tomorrow 09:00",
                    },
                    Choice {
                        id: "monday",
                        label: "Monday 09:00",
                    },
                    Choice {
                        id: "custom",
                        label: "Custom date",
                    },
                ],
                "monday",
            ),
            FlexItem::fixed(3),
        )
        .child(
            "reason",
            TextInput::new()
                .placeholder("Reason required for snooze")
                .hotkey(keys::RAW_INBOX_TAB.hotkey()),
            FlexItem::fixed(1),
        )
        .child(
            "explain",
            Panel::new().top_left("Invariant").content([
                "Snooze applies to clarified action only.",
                "Raw item selected: disabled — clarify first.",
            ]),
            FlexItem::fill(1),
        )
}

fn confirm_form() -> Flex<Msg> {
    Flex::column()
        .gap(1)
        .child(
            "confirm",
            TextInput::new()
                .placeholder("Type DISCARD to archive pending suggestion")
                .hotkey(keys::CLARIFY_TAB.hotkey()),
            FlexItem::fixed(1),
        )
        .child(
            "details",
            Panel::new().top_left("Confirmation").content([
                "Destructive operations require explicit confirmation.",
                "AI cards are archived only after user accepts discard.",
            ]),
            FlexItem::fill(1),
        )
}

fn dropdown_single(
    label: &'static str,
    rows: &[Choice],
    selected: &'static str,
) -> Dropdown<Choice, &'static str> {
    Dropdown::single(rows.to_vec(), |row| row.id, |row| row.label.to_string())
        .label(label)
        .selected_one(selected)
        .search_mode(DropdownSearchMode::Contains)
        .commit_mode(DropdownCommitMode::Immediate)
}

fn dropdown_multi<const N: usize>(
    label: &'static str,
    rows: &[Choice],
    selected: [&'static str; N],
) -> Dropdown<Choice, &'static str> {
    Dropdown::multi(rows.to_vec(), |row| row.id, |row| row.label.to_string())
        .label(label)
        .placeholder("Select context")
        .selected(selected)
        .search_mode(DropdownSearchMode::Contains)
}

fn item_list(rows: Vec<ItemRow>) -> DataView<ItemRow, &'static str> {
    DataView::list(
        rows,
        |row| row.id,
        |row| format!("{} — {}", row.state, row.label),
    )
    .selection_mode(SelectionMode::Single)
    .selection_trigger(SelectionTrigger::OnNavigate)
}

fn item_table(rows: Vec<ItemRow>) -> DataView<ItemRow, &'static str> {
    DataView::new(rows, |row| row.id)
        .headers(true)
        .columns(vec![
            Column::text(
                "state",
                "State",
                Constraint::Percentage(25),
                |row: &ItemRow| row.state.to_string(),
            ),
            Column::text(
                "label",
                "Item",
                Constraint::Percentage(75),
                |row: &ItemRow| row.label.to_string(),
            ),
        ])
}

fn ai_table() -> DataView<SuggestionRow, &'static str> {
    DataView::new(ai_suggestions(), |row| row.id)
        .headers(true)
        .selection_mode(SelectionMode::Multi)
        .selection_glyphs(SelectionGlyphs::ASCII)
        .selection_trigger(SelectionTrigger::OnActivate)
        .columns(vec![
            Column::text(
                "title",
                "Suggested split",
                Constraint::Percentage(45),
                |row: &SuggestionRow| row.title.to_string(),
            ),
            Column::rich(
                "confidence",
                "Conf",
                Constraint::Percentage(15),
                |row: &SuggestionRow, _: &CellContext<&'static str>| {
                    let theme = tuicore::theme();
                    Line::from(Span::styled(
                        format!("{}%", row.confidence),
                        Style::default()
                            .fg(theme.accent_fg())
                            .add_modifier(Modifier::BOLD),
                    ))
                },
            ),
            Column::text(
                "why",
                "Explanation",
                Constraint::Percentage(40),
                |row: &SuggestionRow| row.explanation.to_string(),
            ),
        ])
}

fn command_table() -> DataView<ItemRow, &'static str> {
    item_table(vec![
        ItemRow {
            id: "clarify",
            state: "enabled",
            label: "Clarify selected raw",
        },
        ItemRow {
            id: "pull",
            state: "disabled",
            label: "Pull raw to board — clarify first",
        },
        ItemRow {
            id: "snooze",
            state: "disabled",
            label: "Snooze raw — action items only",
        },
        ItemRow {
            id: "accept",
            state: "enabled",
            label: "Accept reviewed split suggestion",
        },
    ])
}

fn raw_items() -> Vec<ItemRow> {
    vec![
        ItemRow {
            id: "raw-1",
            state: "RAW",
            label: "Slack: Carter redline follow-up maybe next Tue",
        },
        ItemRow {
            id: "raw-2",
            state: "RAW",
            label: "Email: unclear demo prep ask from Sales",
        },
        ItemRow {
            id: "raw-3",
            state: "RAW",
            label: "Voicemail: pricing question without owner",
        },
    ]
}

fn returned_items() -> Vec<ItemRow> {
    vec![
        ItemRow {
            id: "ret-1",
            state: "RETURNED",
            label: "Renewal ask missing owner; marker visible",
        },
        ItemRow {
            id: "ret-2",
            state: "RETURNED",
            label: "Pilot checklist sent back by Ops",
        },
    ]
}

fn action_items() -> Vec<ItemRow> {
    vec![
        ItemRow {
            id: "act-1",
            state: "ACTION",
            label: "Email Carter for contract redlines",
        },
        ItemRow {
            id: "act-2",
            state: "SNOOZED",
            label: "Send kickoff agenda after stakeholder list returns",
        },
    ]
}

fn note_items() -> Vec<ItemRow> {
    vec![ItemRow {
        id: "note-1",
        state: "NOTE",
        label: "Customer prefers afternoon calls; excluded from board",
    }]
}

fn ai_suggestions() -> Vec<SuggestionRow> {
    vec![
        SuggestionRow {
            id: "s1",
            title: "Email Carter on redlines",
            confidence: 84,
            explanation: "explicit follow-up verb + person",
        },
        SuggestionRow {
            id: "s2",
            title: "Schedule kickoff next Tuesday",
            confidence: 71,
            explanation: "date phrase but owner missing",
        },
        SuggestionRow {
            id: "s3",
            title: "Store Sales returned marker",
            confidence: 66,
            explanation: "returned source detected",
        },
    ]
}
