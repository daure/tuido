use std::error::Error;

use ratatui::layout::Constraint;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use tuicore::{
    Button, CellContext, Column, CrossAlign, DataView, Dialog, DialogBackdrop, DialogCloseReason,
    DialogHost, DialogLayer, Dropdown, DropdownSearchMode, Flex, FlexItem, MainAlign, Panel,
    SelectionGlyphs, SelectionMode, SelectionTrigger, Split, Tab, Tabs, TabsVariant, TextInput,
    Toggle,
};
use tuido::app_keymap::{self, keys};

#[derive(Debug)]
enum Msg {
    OpenDialog(&'static str),
    CloseDialog(DialogCloseReason),
}

#[derive(Clone)]
struct FocusItem {
    id: &'static str,
    size: &'static str,
    title: &'static str,
    context: &'static str,
    source: &'static str,
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
    let controls = Panel::new()
        .top_left("Daily 1-3-5 Focus Ritual")
        .bottom_left("Planner suggests; user confirms. Capacity mismatch is advisory.")
        .host(control_row());
    let body = Split::horizontal(
        candidate_pools(),
        Split::horizontal(plan_panel(), preview_panel()).ratio(5, 4),
    )
    .ratio(4, 8)
    .gap(1);
    let actions = Panel::new()
        .top_left("Ritual actions")
        .bottom_left(
            "Done archives immediately • mid-day swap stages add/remove and preserves source state",
        )
        .host(
            Flex::row()
                .gap(2)
                .child(
                    "picker",
                    Button::new("Candidate picker")
                        .hotkey(keys::CANDIDATE_PICKER_BUTTON.hotkey())
                        .on_press(|| Msg::OpenDialog("Candidate picker")),
                    FlexItem::fixed(18),
                )
                .child(
                    "frog",
                    Button::new("Pick frog")
                        .hotkey(keys::PICK_FROG_BUTTON.hotkey())
                        .on_press(|| Msg::OpenDialog("Frog picker")),
                    FlexItem::fixed(14),
                )
                .child(
                    "swap",
                    Button::new("Mid-day swap")
                        .hotkey(keys::MIDDAY_SWAP_BUTTON.hotkey())
                        .on_press(|| Msg::OpenDialog("Mid-day swap staging")),
                    FlexItem::fixed(16),
                )
                .child(
                    "done",
                    Button::new("Done → archive")
                        .hotkey(keys::DONE_ARCHIVE_BUTTON.hotkey())
                        .on_press(|| Msg::OpenDialog("Done archives immediately")),
                    FlexItem::fixed(18),
                )
                .child(
                    "help",
                    Button::new(format!("{}: commands", keys::COMMAND_PALETTE.label()))
                        .hotkey(keys::COMMAND_PALETTE.hotkey())
                        .on_press(|| Msg::OpenDialog("Focus commands")),
                    FlexItem::fixed(14),
                ),
        );
    Flex::column()
        .gap(1)
        .child("controls", controls, FlexItem::content())
        .child("body", body, FlexItem::fill(1))
        .child("actions", actions, FlexItem::fixed(3))
}

fn control_row() -> Flex<Msg> {
    let selectors = Flex::row()
        .gap(2)
        .align(CrossAlign::Center)
        .child(
            "planner",
            dropdown_single(
                "Planner",
                vec![
                    Choice {
                        id: "steady",
                        label: "Steady 1-3-5",
                    },
                    Choice {
                        id: "recovery",
                        label: "Recovery day",
                    },
                    Choice {
                        id: "push",
                        label: "Push day",
                    },
                ],
                "steady",
            ),
            FlexItem::fixed(26),
        )
        .child(
            "horizon",
            dropdown_single(
                "Horizon",
                vec![
                    Choice {
                        id: "today",
                        label: "Today",
                    },
                    Choice {
                        id: "week",
                        label: "This week",
                    },
                    Choice {
                        id: "returned",
                        label: "Returned now",
                    },
                ],
                "today",
            ),
            FlexItem::fixed(24),
        );

    let toggles = Flex::row()
        .gap(2)
        .align(CrossAlign::Center)
        .child(
            "returned",
            Toggle::new("include returned")
                .checked(true)
                .hotkey(keys::INCLUDE_RETURNED_TOGGLE.hotkey()),
            FlexItem::fixed(18),
        )
        .child(
            "due",
            Toggle::new("due-soon")
                .checked(true)
                .hotkey(keys::DUE_SOON_TOGGLE.hotkey()),
            FlexItem::fixed(12),
        )
        .child(
            "future",
            Toggle::new("future-start visible").hotkey(keys::FUTURE_START_TOGGLE.hotkey()),
            FlexItem::fixed(24),
        );

    Flex::row()
        .justify(MainAlign::SpaceBetween)
        .align(CrossAlign::Center)
        .child("selectors", selectors, FlexItem::content())
        .child("toggles", toggles, FlexItem::content())
}

fn candidate_pools() -> impl tuicore::TuiNode<Msg> {
    Panel::new().top_left("Candidate pools by size").host(
        Tabs::new(vec![
            Tab::new(
                "Big",
                candidate_table(big_items()).hotkey(keys::BIG_CANDIDATES_TAB.hotkey()),
            )
            .hotkey(keys::BIG_CANDIDATES_TAB.hotkey()),
            Tab::new(
                "Medium",
                candidate_table(medium_items()).hotkey(keys::MEDIUM_CANDIDATES_TAB.hotkey()),
            )
            .hotkey(keys::MEDIUM_CANDIDATES_TAB.hotkey()),
            Tab::new(
                "Small",
                candidate_table(small_items()).hotkey(keys::SMALL_CANDIDATES_TAB.hotkey()),
            )
            .hotkey(keys::SMALL_CANDIDATES_TAB.hotkey()),
        ])
        .variant(TabsVariant::Boxed)
        .bordered(true),
    )
}

fn plan_panel() -> impl tuicore::TuiNode<Msg> {
    Panel::new().top_left("Focus plan").bottom_left("Frog is user-confirmed • 1 big / 3 medium / 5 small advisory meter").host(
        Tabs::new(vec![
            Tab::new("Plan", plan_table()).hotkey(keys::PLAN_TAB.hotkey()),
            Tab::text("Meter", "Current: frog 1/1, big 1/1, medium 2/3, small 4/5. Advisory mismatch: add 1 medium + 1 small or accept lighter day.").hotkey(keys::METER_TAB.hotkey()),
            Tab::text("Rules", "Planner suggestions never auto-commit. Removed swap items leave focus only and preserve backlog/source state.").hotkey(keys::RULES_TAB.hotkey()),
        ])
        .variant(TabsVariant::Underline)
        .bordered(true),
    )
}

fn preview_panel() -> impl tuicore::TuiNode<Msg> {
    Panel::new().top_left("Preview").host(
        Tabs::new(vec![
            Tab::text("Rationale", "Mock planner chose urgent renewal frog because returned marker is acknowledged and due soon. User confirmation still required.").hotkey(keys::RATIONALE_TAB.hotkey()),
            Tab::text("Swap Impact", "Incoming urgent item stages remove: draft launch email. Removed item returns unchanged to board, not snoozed, not archived.").hotkey(keys::SWAP_IMPACT_TAB.hotkey()),
            Tab::text("History", "08:55 plan created\n09:00 frog confirmed\n11:40 urgent swap staged\nNo outcome committed yet.").hotkey(keys::HISTORY_TAB.hotkey()),
            Tab::text("Board State", "Action board has 12 active, 2 returned, 3 snoozed hidden. Done immediately archives and records outcome.").hotkey(keys::BOARD_STATE_TAB.hotkey()),
        ])
        .variant(TabsVariant::Underline)
        .bordered(true),
    )
}

fn modal() -> DialogHost<Tabs<Msg>, Msg> {
    Dialog::new()
        .top_left("Candidate picker")
        .bottom_left(format!("{} closes", keys::DIALOG_CLOSE.label()))
        .bottom_right("staged operations")
        .on_close(Msg::CloseDialog)
        .host(
            Tabs::new(vec![
                Tab::new("Candidates", picker_form()).hotkey(keys::CANDIDATES_TAB.hotkey()),
                Tab::new("Frog", frog_form()).hotkey(keys::FROG_TAB.hotkey()),
                Tab::new("Swap", swap_form()).hotkey(keys::SWAP_TAB.hotkey()),
                Tab::new("Confirm", confirm_form()).hotkey(keys::FOCUS_CONFIRM_TAB.hotkey()),
            ])
            .variant(TabsVariant::Boxed)
            .bordered(true),
        )
}

fn picker_form() -> Flex<Msg> {
    Flex::column()
        .gap(1)
        .child(
            "search",
            TextInput::new()
                .placeholder("Search candidates by person/project/context")
                .hotkey(keys::CANDIDATE_SEARCH_FIELD.hotkey()),
            FlexItem::fixed(1),
        )
        .child("list", candidate_table(all_candidates()), FlexItem::fill(1))
}

fn frog_form() -> Flex<Msg> {
    Flex::column()
        .gap(1)
        .child(
            "search",
            TextInput::new()
                .placeholder("Pick one frog; must confirm")
                .hotkey(keys::FROG_SEARCH_FIELD.hotkey()),
            FlexItem::fixed(1),
        )
        .child("frog", candidate_table(big_items()), FlexItem::fill(1))
        .child(
            "note",
            Panel::new()
                .top_left("Frog invariant")
                .content(["Frog is user-confirmed, never auto-selected by planner."]),
            FlexItem::fixed(3),
        )
}

fn swap_form() -> Flex<Msg> {
    Flex::column()
        .gap(1)
        .child(
            "incoming",
            dropdown_single(
                "Incoming urgent",
                vec![
                    Choice {
                        id: "vip",
                        label: "VIP escalation call",
                    },
                    Choice {
                        id: "renewal",
                        label: "Renewal redline fix",
                    },
                ],
                "vip",
            ),
            FlexItem::fixed(3),
        )
        .child("outgoing", candidate_table(plan_items()), FlexItem::fill(1))
        .child(
            "impact",
            Panel::new().top_left("Swap staging").content([
                "Outgoing multiselect removes from focus only.",
                "Source state preserved; toasts: moved out returns unchanged, done archived.",
            ]),
            FlexItem::fixed(4),
        )
}

fn confirm_form() -> Flex<Msg> {
    Flex::column()
        .gap(1)
        .child(
            "confirm",
            TextInput::new()
                .placeholder("Type CONFIRM to apply staged focus change")
                .hotkey(keys::FOCUS_CONFIRM_TAB.hotkey()),
            FlexItem::fixed(1),
        )
        .child(
            "summary",
            Panel::new().top_left("Commit summary").content([
                "Add 1 urgent small. Remove 1 small from focus; board state unchanged.",
                "Mark done archives immediately and logs outcome.",
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

fn candidate_table(rows: Vec<FocusItem>) -> DataView<FocusItem, &'static str> {
    DataView::new(rows, |row| row.id)
        .headers(true)
        .selection_mode(SelectionMode::Multi)
        .selection_glyphs(SelectionGlyphs::ASCII)
        .selection_trigger(SelectionTrigger::OnActivate)
        .columns(vec![
            Column::rich(
                "size",
                "Size",
                Constraint::Percentage(12),
                |row: &FocusItem, _: &CellContext<&'static str>| {
                    let theme = tuicore::theme();
                    let color = match row.size {
                        "BIG" => theme.error_fg(),
                        "MED" => theme.warning_fg(),
                        _ => theme.success_fg(),
                    };
                    Line::from(Span::styled(
                        row.size.to_string(),
                        Style::default().fg(color).add_modifier(Modifier::BOLD),
                    ))
                },
            ),
            Column::text(
                "title",
                "Candidate",
                Constraint::Percentage(43),
                |row: &FocusItem| row.title.to_string(),
            ),
            Column::text(
                "context",
                "Context",
                Constraint::Percentage(25),
                |row: &FocusItem| row.context.to_string(),
            ),
            Column::text(
                "source",
                "Source state",
                Constraint::Percentage(20),
                |row: &FocusItem| row.source.to_string(),
            ),
        ])
}

fn plan_table() -> DataView<FocusItem, &'static str> {
    candidate_table(plan_items())
}

fn big_items() -> Vec<FocusItem> {
    vec![
        FocusItem {
            id: "b1",
            size: "BIG",
            title: "Resolve renewal redline blocker",
            context: "Carter / Legal",
            source: "returned acknowledged",
        },
        FocusItem {
            id: "b2",
            size: "BIG",
            title: "Draft launch cutover plan",
            context: "Launch",
            source: "action board",
        },
    ]
}

fn medium_items() -> Vec<FocusItem> {
    vec![
        FocusItem {
            id: "m1",
            size: "MED",
            title: "Prepare kickoff agenda",
            context: "Sales",
            source: "action board",
        },
        FocusItem {
            id: "m2",
            size: "MED",
            title: "Review CRM import errors",
            context: "CRM",
            source: "due soon",
        },
        FocusItem {
            id: "m3",
            size: "MED",
            title: "Call pilot owner",
            context: "Success",
            source: "returned",
        },
    ]
}

fn small_items() -> Vec<FocusItem> {
    vec![
        FocusItem {
            id: "s1",
            size: "SML",
            title: "Send Carter reminder",
            context: "Legal",
            source: "action board",
        },
        FocusItem {
            id: "s2",
            size: "SML",
            title: "Archive done demo prep",
            context: "Launch",
            source: "done → archive",
        },
        FocusItem {
            id: "s3",
            size: "SML",
            title: "Confirm Tuesday room",
            context: "Ops",
            source: "due soon",
        },
        FocusItem {
            id: "s4",
            size: "SML",
            title: "Update customer note",
            context: "Renewal",
            source: "note excluded",
        },
        FocusItem {
            id: "s5",
            size: "SML",
            title: "Check snoozed return",
            context: "Support",
            source: "returns today",
        },
    ]
}

fn plan_items() -> Vec<FocusItem> {
    vec![
        FocusItem {
            id: "frog",
            size: "BIG",
            title: "FROG: Resolve renewal redline blocker",
            context: "Carter / Legal",
            source: "user-confirmed",
        },
        FocusItem {
            id: "m1",
            size: "MED",
            title: "Prepare kickoff agenda",
            context: "Sales",
            source: "focus",
        },
        FocusItem {
            id: "m2",
            size: "MED",
            title: "Review CRM import errors",
            context: "CRM",
            source: "focus",
        },
        FocusItem {
            id: "s1",
            size: "SML",
            title: "Send Carter reminder",
            context: "Legal",
            source: "focus",
        },
        FocusItem {
            id: "s3",
            size: "SML",
            title: "Confirm Tuesday room",
            context: "Ops",
            source: "focus",
        },
    ]
}

fn all_candidates() -> Vec<FocusItem> {
    let mut rows = big_items();
    rows.extend(medium_items());
    rows.extend(small_items());
    rows
}
