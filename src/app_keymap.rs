use std::{collections::HashMap, env, error::Error, fmt, sync::OnceLock};

use tuicore::{Key, KeyEvent, KeyModifiers, KeySpec, TuiEvent};

static KEYMAP: OnceLock<AppKeymap> = OnceLock::new();

#[derive(Debug, Clone, Copy)]
pub struct AppBinding {
    name: &'static str,
    default: &'static str,
    sequence: bool,
}

impl AppBinding {
    pub const fn new(name: &'static str, default: &'static str) -> Self {
        Self {
            name,
            default,
            sequence: false,
        }
    }

    pub const fn new_sequence(name: &'static str, default: &'static str) -> Self {
        Self {
            name,
            default,
            sequence: true,
        }
    }

    pub fn hotkey(self) -> String {
        self.resolved().raw.to_string()
    }

    pub fn label(self) -> String {
        let resolved = self.resolved();
        resolved
            .spec
            .map(|spec| spec.label())
            .unwrap_or(resolved.raw)
    }

    pub fn key_spec(self) -> KeySpec {
        self.resolved()
            .spec
            .unwrap_or_else(|| panic!("app key `{}` is a sequence, not a key", self.name))
    }

    pub fn matches(self, event: &TuiEvent) -> bool {
        let TuiEvent::Key(key) = event else {
            return false;
        };

        self.resolved().spec.is_some_and(|spec| spec.matches(*key))
    }

    fn spec(self, raw: &str) -> Result<Option<KeySpec>, AppKeymapError> {
        if self.sequence {
            if raw.trim().is_empty() {
                return Err(AppKeymapError::invalid(self.name, raw));
            }
            return Ok(None);
        }
        parse_key(raw)
            .map(Some)
            .ok_or_else(|| AppKeymapError::invalid(self.name, raw))
    }

    fn resolved(self) -> ResolvedBinding {
        if let Some(keymap) = KEYMAP.get() {
            return keymap
                .binding(self.name)
                .unwrap_or_else(|| panic!("missing app key `{}`", self.name));
        }

        ResolvedBinding {
            raw: self.default.into(),
            event: parse_key_event(self.default),
            spec: self
                .spec(self.default)
                .unwrap_or_else(|error| panic!("invalid app key `{}`: {error}", self.name)),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AppKeymapError {
    message: String,
}

impl AppKeymapError {
    fn invalid(name: &str, key: &str) -> Self {
        Self {
            message: format!("unsupported app key `{name}` value `{key}`"),
        }
    }

    fn unknown(name: &str) -> Self {
        Self {
            message: format!("unknown app key `{name}`"),
        }
    }

    fn malformed(entry: &str) -> Self {
        Self {
            message: format!("malformed app key override `{entry}`; expected NAME=key"),
        }
    }

    fn conflict(context: &str, first: AppBinding, second: AppBinding) -> Self {
        Self {
            message: format!(
                "ambiguous app keys `{}` and `{}` in {context} context",
                first.name, second.name
            ),
        }
    }

    fn runtime_conflict(binding: AppBinding) -> Self {
        Self {
            message: format!(
                "app key `{}` conflicts with runtime quit binding",
                binding.name
            ),
        }
    }

    fn already_initialized() -> Self {
        Self {
            message: "app keymap already initialized".into(),
        }
    }
}

impl fmt::Display for AppKeymapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for AppKeymapError {}

#[derive(Debug, Clone)]
pub struct AppKeymap {
    bindings: HashMap<&'static str, ResolvedBinding>,
}

impl AppKeymap {
    fn from_overrides(
        overrides: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Self, AppKeymapError> {
        let mut overrides: HashMap<String, String> = overrides.into_iter().collect();
        let mut bindings = HashMap::with_capacity(keys::ALL.len());

        for binding in keys::ALL {
            let raw = overrides
                .remove(binding.name)
                .unwrap_or_else(|| binding.default.into());
            let spec = binding.spec(&raw)?;
            let event = (!binding.sequence).then(|| parse_key_event(&raw)).flatten();
            bindings.insert(binding.name, ResolvedBinding { raw, spec, event });
        }

        if let Some(name) = overrides.keys().next() {
            return Err(AppKeymapError::unknown(name));
        }

        validate_contexts(&bindings)?;

        Ok(Self { bindings })
    }

    fn from_env() -> Result<Self, AppKeymapError> {
        Self::from_overrides(env_overrides()?)
    }

    fn validate_runtime_quit(
        &self,
        runtime: &tuicore::RuntimeKeyBindings,
    ) -> Result<(), AppKeymapError> {
        for binding in [
            keys::TASK_COMPLETE,
            keys::COMPLETE_DONE,
            keys::COMPLETE_REJECT,
            keys::DIALOG_CANCEL,
            keys::DIALOG_CLOSE,
        ] {
            let resolved = &self.bindings[binding.name];
            if resolved
                .event
                .is_some_and(|event| runtime.quit_matches(event))
            {
                return Err(AppKeymapError::runtime_conflict(binding));
            }
        }
        Ok(())
    }

    fn binding(&self, name: &'static str) -> Option<ResolvedBinding> {
        self.bindings.get(name).cloned()
    }
}

#[derive(Debug, Clone)]
struct ResolvedBinding {
    raw: String,
    spec: Option<KeySpec>,
    event: Option<KeyEvent>,
}

fn validate_contexts(
    bindings: &HashMap<&'static str, ResolvedBinding>,
) -> Result<(), AppKeymapError> {
    for context in keys::CONTEXTS {
        for (index, first) in context.bindings.iter().enumerate() {
            for second in &context.bindings[index + 1..] {
                let first_pattern = binding_pattern(*first, &bindings[first.name]);
                let second_pattern = binding_pattern(*second, &bindings[second.name]);
                if is_prefix(&first_pattern, &second_pattern)
                    || is_prefix(&second_pattern, &first_pattern)
                {
                    return Err(AppKeymapError::conflict(context.name, *first, *second));
                }
            }
        }
    }
    Ok(())
}

fn binding_pattern(binding: AppBinding, resolved: &ResolvedBinding) -> Vec<String> {
    let normalized = resolved.raw.trim().to_ascii_lowercase();
    if binding.sequence {
        normalized
            .chars()
            .map(|character| character.to_string())
            .collect()
    } else {
        vec![normalized]
    }
}

fn is_prefix(first: &[String], second: &[String]) -> bool {
    first.len() <= second.len() && first.iter().zip(second).all(|(left, right)| left == right)
}

pub fn try_init() -> Result<(), AppKeymapError> {
    let keymap = AppKeymap::from_env()?;
    keymap.validate_runtime_quit(tuicore::keybindings().runtime())?;
    KEYMAP
        .set(keymap)
        .map_err(|_| AppKeymapError::already_initialized())?;
    Ok(())
}

fn env_overrides() -> Result<Vec<(String, String)>, AppKeymapError> {
    let mut overrides = Vec::new();

    if let Ok(value) = env::var("TUIDO_KEYMAP") {
        overrides.extend(parse_overrides(&value)?);
    }

    for (name, value) in env::vars() {
        if let Some(binding_name) = name.strip_prefix("TUIDO_KEY_") {
            overrides.push((binding_name.to_string(), value));
        }
    }

    Ok(overrides)
}

fn parse_overrides(value: &str) -> Result<Vec<(String, String)>, AppKeymapError> {
    let mut overrides = Vec::new();

    for entry in value.split([',', ';', '\n']) {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let Some((name, key)) = entry.split_once('=') else {
            return Err(AppKeymapError::malformed(entry));
        };
        overrides.push((name.trim().to_string(), key.trim().to_string()));
    }

    Ok(overrides)
}

pub fn validate_defaults() -> Result<(), AppKeymapError> {
    AppKeymap::from_overrides(std::iter::empty::<(String, String)>())?;
    Ok(())
}

pub fn matches_any(event: &TuiEvent, bindings: &[AppBinding]) -> bool {
    bindings.iter().any(|binding| binding.matches(event))
}

fn parse_key(value: &str) -> Option<KeySpec> {
    parse_key_event(value).map(KeySpec::from)
}

fn parse_key_event(value: &str) -> Option<KeyEvent> {
    let value = value.trim().to_ascii_lowercase();

    if let Some(rest) = value.strip_prefix("ctrl+") {
        return modified_key_event(rest, KeyModifiers::CONTROL);
    }

    let code = match value.as_str() {
        "esc" => Key::Esc,
        "enter" => Key::Enter,
        "tab" => Key::Tab,
        "backspace" => Key::Backspace,
        "delete" => Key::Delete,
        "space" => Key::Char(' '),
        text => return single_char(text).map(|character| KeyEvent::from(Key::Char(character))),
    };

    Some(KeyEvent::from(code))
}

fn modified_key_event(value: &str, modifiers: KeyModifiers) -> Option<KeyEvent> {
    let code = match value {
        "enter" => Key::Enter,
        "backspace" => Key::Backspace,
        "delete" => Key::Delete,
        "space" => Key::Char(' '),
        text => Key::Char(single_char(text)?),
    };
    Some(KeyEvent { code, modifiers })
}

fn single_char(value: &str) -> Option<char> {
    let mut chars = value.chars();
    let key = chars.next()?;
    chars.next().is_none().then_some(key)
}

pub mod keys {
    use super::AppBinding;

    pub(super) struct BindingContext {
        pub(super) name: &'static str,
        pub(super) bindings: &'static [AppBinding],
    }

    pub const APP_TASKS_TAB: AppBinding = AppBinding::new("APP_TASKS_TAB", "t");
    pub const APP_CALENDAR_TAB: AppBinding = AppBinding::new("APP_CALENDAR_TAB", "c");
    pub const APP_PROJECTS_TAB: AppBinding = AppBinding::new_sequence("APP_PROJECTS_TAB", "pr");
    pub const APP_PEOPLE_TAB: AppBinding = AppBinding::new_sequence("APP_PEOPLE_TAB", "pe");
    pub const TASK_QUICK_CREATE: AppBinding = AppBinding::new("TASK_QUICK_CREATE", "n");
    pub const TASK_VIEW_MENU: AppBinding = AppBinding::new("TASK_VIEW_MENU", "f");
    pub const TASK_LABEL_FILTER: AppBinding = AppBinding::new("TASK_LABEL_FILTER", "a");
    pub const TASK_DELETE: AppBinding = AppBinding::new("TASK_DELETE", "delete");
    pub const TASK_DELETE_BACKSPACE: AppBinding = AppBinding::new("TASK_DELETE_X", "backspace");
    pub const TASK_DELETE_CTRL_X: AppBinding = AppBinding::new("TASK_DELETE_CTRL_X", "ctrl+x");
    pub const TASK_QUICK_MENU: AppBinding = AppBinding::new("TASK_QUICK_MENU", ".");
    pub const TASK_MOVE_MODE: AppBinding = AppBinding::new("TASK_MOVE_MODE", "ctrl+m");
    pub const TASK_SNOOZE: AppBinding = AppBinding::new("TASK_SNOOZE", "ctrl+z");
    pub const TASK_COMPLETE: AppBinding = AppBinding::new("TASK_COMPLETE", "ctrl+c");
    pub const TASK_AGENT_YANK: AppBinding = AppBinding::new_sequence("TASK_AGENT_YANK", "ya");
    pub const MANAGEMENT_CREATE: AppBinding = AppBinding::new("MANAGEMENT_CREATE", "n");
    pub const MANAGEMENT_DELETE: AppBinding = AppBinding::new("MANAGEMENT_DELETE", "delete");
    pub const MANAGEMENT_DELETE_BACKSPACE: AppBinding =
        AppBinding::new("MANAGEMENT_DELETE_ALT", "backspace");
    pub const MANAGEMENT_DELETE_X: AppBinding = AppBinding::new("MANAGEMENT_DELETE_X", "ctrl+x");
    pub const PERSON_NAME_FIELD: AppBinding = AppBinding::new_sequence("PERSON_NAME_FIELD", "am");
    pub const PERSON_EMAIL_FIELD: AppBinding = AppBinding::new_sequence("PERSON_EMAIL_FIELD", "em");
    pub const PERSON_ABOUT_FIELD: AppBinding = AppBinding::new_sequence("PERSON_ABOUT_FIELD", "ab");
    pub const PERSON_ABOUT_EDITOR: AppBinding =
        AppBinding::new_sequence("PERSON_ABOUT_EDITOR", "ao");
    pub const PERSON_ACTIVE_FIELD: AppBinding =
        AppBinding::new_sequence("PERSON_ACTIVE_FIELD", "ac");
    pub const PROJECT_KEY_FIELD: AppBinding = AppBinding::new_sequence("PROJECT_KEY_FIELD", "ke");
    pub const PROJECT_NAME_FIELD: AppBinding = AppBinding::new_sequence("PROJECT_NAME_FIELD", "am");
    pub const PROJECT_DESCRIPTION_FIELD: AppBinding =
        AppBinding::new_sequence("PROJECT_DESCRIPTION_FIELD", "dd");
    pub const PROJECT_DESCRIPTION_EDITOR: AppBinding =
        AppBinding::new_sequence("PROJECT_DESCRIPTION_EDITOR", "do");
    pub const PROJECT_LEAD_FIELD: AppBinding = AppBinding::new_sequence("PROJECT_LEAD_FIELD", "ea");
    pub const TAG_LABEL_FIELD: AppBinding = AppBinding::new_sequence("TAG_LABEL_FIELD", "ab");
    pub const TASK_TITLE_FIELD: AppBinding = AppBinding::new_sequence("TASK_TITLE_FIELD", "ti");
    pub const TASK_DESCRIPTION_FIELD: AppBinding =
        AppBinding::new_sequence("TASK_DESCRIPTION_FIELD", "dd");
    pub const TASK_DESCRIPTION_EDITOR: AppBinding =
        AppBinding::new_sequence("TASK_DESCRIPTION_EDITOR", "do");
    pub const TASK_STATE_FIELD: AppBinding = AppBinding::new_sequence("TASK_STATE_FIELD", "st");
    pub const TASK_SIZE_FIELD: AppBinding = AppBinding::new_sequence("TASK_SIZE_FIELD", "si");
    pub const TASK_PRIORITY_FIELD: AppBinding =
        AppBinding::new_sequence("TASK_PRIORITY_FIELD", "pri");
    pub const TASK_PEOPLE_FIELD: AppBinding = AppBinding::new_sequence("TASK_PEOPLE_FIELD", "pe");
    pub const TASK_PROJECTS_FIELD: AppBinding =
        AppBinding::new_sequence("TASK_PROJECTS_FIELD", "pro");
    pub const TASK_TAGS_FIELD: AppBinding = AppBinding::new_sequence("TASK_TAGS_FIELD", "ta");
    pub const TASK_LINKS_FIELD: AppBinding = AppBinding::new_sequence("TASK_LINKS_FIELD", "ur");
    pub const TASK_LINK_DELETE: AppBinding = AppBinding::new("TASK_LINK_DELETE", "ctrl+x");
    pub const TASK_START_DATE_FIELD: AppBinding =
        AppBinding::new_sequence("TASK_START_DATE_FIELD", "sd");
    pub const TASK_END_DATE_FIELD: AppBinding =
        AppBinding::new_sequence("TASK_END_DATE_FIELD", "ed");
    pub const TASK_SNOOZED_UNTIL_FIELD: AppBinding =
        AppBinding::new_sequence("TASK_SNOOZED_UNTIL_FIELD", "su");
    pub const DETAIL_CLOSE: AppBinding = AppBinding::new("DETAIL_CLOSE", "esc");
    pub const DETAIL_CLOSE_ALT: AppBinding = AppBinding::new("DETAIL_CLOSE_ALT", "ctrl+[");

    pub const CAPTURE_RAW_LEAD: AppBinding = AppBinding::new("CAPTURE_RAW_LEAD", "i");
    pub const ACCEPT_SPLIT: AppBinding = AppBinding::new("ACCEPT_SPLIT", "a");
    pub const MERGE_SELECTED: AppBinding = AppBinding::new("MERGE_SELECTED", "m");
    pub const DISCARD_SUGGESTION: AppBinding = AppBinding::new("DISCARD_SUGGESTION", "d");
    pub const PULL_TO_BOARD: AppBinding = AppBinding::new("PULL_TO_BOARD", "p");
    pub const SNOOZE_ACTION: AppBinding = AppBinding::new("SNOOZE_ACTION", "z");
    pub const COMMAND_PALETTE: AppBinding = AppBinding::new("COMMAND_PALETTE", "?");
    pub const TRIAGE_QUEUES_PANEL: AppBinding = AppBinding::new("TRIAGE_QUEUES_PANEL", "q");
    pub const RAW_INBOX_TAB: AppBinding = AppBinding::new("RAW_INBOX_TAB", "r");
    pub const RETURNED_TAB: AppBinding = AppBinding::new("RETURNED_TAB", "t");
    pub const ACTIONS_TAB: AppBinding = AppBinding::new("ACTIONS_TAB", "a");
    pub const NOTES_TAB: AppBinding = AppBinding::new("NOTES_TAB", "n");
    pub const CLARIFY_TAB: AppBinding = AppBinding::new("CLARIFY_TAB", "c");
    pub const CONTEXT_TAB: AppBinding = AppBinding::new("CONTEXT_TAB", "x");
    pub const DATES_TAB: AppBinding = AppBinding::new("DATES_TAB", "d");
    pub const AI_RATIONALE_TAB: AppBinding = AppBinding::new("AI_RATIONALE_TAB", "i");
    pub const HISTORY_TAB: AppBinding = AppBinding::new("HISTORY_TAB", "h");
    pub const AI_SUGGESTIONS_TABLE: AppBinding = AppBinding::new("AI_SUGGESTIONS_TABLE", "s");
    pub const RAW_BODY_FIELD: AppBinding = AppBinding::new("RAW_BODY_FIELD", "b");
    pub const ACTION_TITLE_FIELD: AppBinding = AppBinding::new("ACTION_TITLE_FIELD", "t");
    pub const AI_REVIEWED_TOGGLE: AppBinding = AppBinding::new("AI_REVIEWED_TOGGLE", "v");
    pub const RETURNED_ACK_TOGGLE: AppBinding = AppBinding::new("RETURNED_ACK_TOGGLE", "g");
    pub const CONTEXT_NOTE_FIELD: AppBinding = AppBinding::new("CONTEXT_NOTE_FIELD", "n");

    pub const COMMAND_BAR: AppBinding = AppBinding::new("COMMAND_BAR", ":");
    pub const FILTER_PREFIX: AppBinding = AppBinding::new("FILTER_PREFIX", "/");
    pub const ACTION_PALETTE_BUTTON: AppBinding = AppBinding::new("ACTION_PALETTE_BUTTON", "a");
    pub const ARCHIVE_CONFIRM_BUTTON: AppBinding = AppBinding::new("ARCHIVE_CONFIRM_BUTTON", "d");
    pub const BULK_SNOOZE_BUTTON: AppBinding = AppBinding::new("BULK_SNOOZE_BUTTON", "z");
    pub const PULL_FOCUS_BUTTON: AppBinding = AppBinding::new("PULL_FOCUS_BUTTON", "p");
    pub const SHOW_FUTURE_TOGGLE: AppBinding = AppBinding::new("SHOW_FUTURE_TOGGLE", "f");
    pub const RETURNED_FILTER_TOGGLE: AppBinding = AppBinding::new("RETURNED_FILTER_TOGGLE", "/");
    pub const CONTEXTS_PANEL: AppBinding = AppBinding::new("CONTEXTS_PANEL", "c");
    pub const DETAIL_TAB: AppBinding = AppBinding::new("DETAIL_TAB", "d");
    pub const AI_EVIDENCE_TAB: AppBinding = AppBinding::new("AI_EVIDENCE_TAB", "e");
    pub const RELATIONSHIPS_TAB: AppBinding = AppBinding::new("RELATIONSHIPS_TAB", "r");
    pub const OPERATION_PLAN_TAB: AppBinding = AppBinding::new("OPERATION_PLAN_TAB", "o");
    pub const PALETTE_TAB: AppBinding = AppBinding::new("PALETTE_TAB", "a");
    pub const CONFIRM_TAB: AppBinding = AppBinding::new("CONFIRM_TAB", "d");
    pub const SNOOZE_TAB: AppBinding = AppBinding::new("SNOOZE_TAB", "z");
    pub const ACTION_QUERY_FIELD: AppBinding = AppBinding::new("ACTION_QUERY_FIELD", ":");
    pub const ARCHIVE_CONFIRM_TEXT: AppBinding = AppBinding::new("ARCHIVE_CONFIRM_TEXT", "d");
    pub const SNOOZE_REASON_FIELD: AppBinding = AppBinding::new("SNOOZE_REASON_FIELD", "r");

    pub const CANDIDATE_PICKER_BUTTON: AppBinding = AppBinding::new("CANDIDATE_PICKER_BUTTON", "p");
    pub const PICK_FROG_BUTTON: AppBinding = AppBinding::new("PICK_FROG_BUTTON", "f");
    pub const MIDDAY_SWAP_BUTTON: AppBinding = AppBinding::new("MIDDAY_SWAP_BUTTON", "s");
    pub const DONE_ARCHIVE_BUTTON: AppBinding = AppBinding::new("DONE_ARCHIVE_BUTTON", "d");
    pub const INCLUDE_RETURNED_TOGGLE: AppBinding = AppBinding::new("INCLUDE_RETURNED_TOGGLE", "r");
    pub const DUE_SOON_TOGGLE: AppBinding = AppBinding::new("DUE_SOON_TOGGLE", "u");
    pub const FUTURE_START_TOGGLE: AppBinding = AppBinding::new("FUTURE_START_TOGGLE", "g");
    pub const BIG_CANDIDATES_TAB: AppBinding = AppBinding::new("BIG_CANDIDATES_TAB", "b");
    pub const MEDIUM_CANDIDATES_TAB: AppBinding = AppBinding::new("MEDIUM_CANDIDATES_TAB", "m");
    pub const SMALL_CANDIDATES_TAB: AppBinding = AppBinding::new("SMALL_CANDIDATES_TAB", "s");
    pub const PLAN_TAB: AppBinding = AppBinding::new("PLAN_TAB", "1");
    pub const METER_TAB: AppBinding = AppBinding::new("METER_TAB", "2");
    pub const RULES_TAB: AppBinding = AppBinding::new("RULES_TAB", "3");
    pub const RATIONALE_TAB: AppBinding = AppBinding::new("RATIONALE_TAB", "r");
    pub const SWAP_IMPACT_TAB: AppBinding = AppBinding::new("SWAP_IMPACT_TAB", "s");
    pub const BOARD_STATE_TAB: AppBinding = AppBinding::new("BOARD_STATE_TAB", "b");
    pub const CANDIDATES_TAB: AppBinding = AppBinding::new("CANDIDATES_TAB", "p");
    pub const FROG_TAB: AppBinding = AppBinding::new("FROG_TAB", "f");
    pub const SWAP_TAB: AppBinding = AppBinding::new("SWAP_TAB", "s");
    pub const FOCUS_CONFIRM_TAB: AppBinding = AppBinding::new("FOCUS_CONFIRM_TAB", "c");
    pub const CANDIDATE_SEARCH_FIELD: AppBinding = AppBinding::new("CANDIDATE_SEARCH_FIELD", "/");
    pub const FROG_SEARCH_FIELD: AppBinding = AppBinding::new("FROG_SEARCH_FIELD", "f");
    pub const FOCUS_CONFIRM_TEXT: AppBinding = AppBinding::new("FOCUS_CONFIRM_TEXT", "c");

    pub const DIALOG_CLOSE: AppBinding = AppBinding::new("DIALOG_CLOSE", "esc");
    pub const DIALOG_OK: AppBinding = AppBinding::new("DIALOG_OK", "o");
    pub const DIALOG_CANCEL: AppBinding = AppBinding::new("DIALOG_CANCEL", "c");
    pub const DIALOG_SUBMIT: AppBinding = AppBinding::new("DIALOG_SUBMIT", "ctrl+enter");
    pub const DELETE_CONFIRM: AppBinding = AppBinding::new("DELETE_CONFIRM", "d");
    pub const COMPLETE_DONE: AppBinding = AppBinding::new("COMPLETE_DONE", "d");
    pub const COMPLETE_REJECT: AppBinding = AppBinding::new("COMPLETE_REJECT", "r");

    pub const ALL: &[AppBinding] = &[
        APP_TASKS_TAB,
        APP_CALENDAR_TAB,
        APP_PROJECTS_TAB,
        APP_PEOPLE_TAB,
        TASK_QUICK_CREATE,
        TASK_VIEW_MENU,
        TASK_LABEL_FILTER,
        TASK_DELETE,
        TASK_DELETE_BACKSPACE,
        TASK_DELETE_CTRL_X,
        TASK_QUICK_MENU,
        TASK_MOVE_MODE,
        TASK_SNOOZE,
        TASK_COMPLETE,
        TASK_AGENT_YANK,
        MANAGEMENT_CREATE,
        MANAGEMENT_DELETE,
        MANAGEMENT_DELETE_BACKSPACE,
        MANAGEMENT_DELETE_X,
        PERSON_NAME_FIELD,
        PERSON_EMAIL_FIELD,
        PERSON_ABOUT_FIELD,
        PERSON_ABOUT_EDITOR,
        PERSON_ACTIVE_FIELD,
        PROJECT_KEY_FIELD,
        PROJECT_NAME_FIELD,
        PROJECT_DESCRIPTION_FIELD,
        PROJECT_DESCRIPTION_EDITOR,
        PROJECT_LEAD_FIELD,
        TAG_LABEL_FIELD,
        TASK_TITLE_FIELD,
        TASK_DESCRIPTION_FIELD,
        TASK_DESCRIPTION_EDITOR,
        TASK_STATE_FIELD,
        TASK_SIZE_FIELD,
        TASK_PRIORITY_FIELD,
        TASK_PEOPLE_FIELD,
        TASK_PROJECTS_FIELD,
        TASK_TAGS_FIELD,
        TASK_LINKS_FIELD,
        TASK_LINK_DELETE,
        TASK_START_DATE_FIELD,
        TASK_END_DATE_FIELD,
        TASK_SNOOZED_UNTIL_FIELD,
        DETAIL_CLOSE,
        DETAIL_CLOSE_ALT,
        CAPTURE_RAW_LEAD,
        ACCEPT_SPLIT,
        MERGE_SELECTED,
        DISCARD_SUGGESTION,
        PULL_TO_BOARD,
        SNOOZE_ACTION,
        COMMAND_PALETTE,
        TRIAGE_QUEUES_PANEL,
        RAW_INBOX_TAB,
        RETURNED_TAB,
        ACTIONS_TAB,
        NOTES_TAB,
        CLARIFY_TAB,
        CONTEXT_TAB,
        DATES_TAB,
        AI_RATIONALE_TAB,
        HISTORY_TAB,
        AI_SUGGESTIONS_TABLE,
        RAW_BODY_FIELD,
        ACTION_TITLE_FIELD,
        AI_REVIEWED_TOGGLE,
        RETURNED_ACK_TOGGLE,
        CONTEXT_NOTE_FIELD,
        COMMAND_BAR,
        FILTER_PREFIX,
        ACTION_PALETTE_BUTTON,
        ARCHIVE_CONFIRM_BUTTON,
        BULK_SNOOZE_BUTTON,
        PULL_FOCUS_BUTTON,
        SHOW_FUTURE_TOGGLE,
        RETURNED_FILTER_TOGGLE,
        CONTEXTS_PANEL,
        DETAIL_TAB,
        AI_EVIDENCE_TAB,
        RELATIONSHIPS_TAB,
        OPERATION_PLAN_TAB,
        PALETTE_TAB,
        CONFIRM_TAB,
        SNOOZE_TAB,
        ACTION_QUERY_FIELD,
        ARCHIVE_CONFIRM_TEXT,
        SNOOZE_REASON_FIELD,
        CANDIDATE_PICKER_BUTTON,
        PICK_FROG_BUTTON,
        MIDDAY_SWAP_BUTTON,
        DONE_ARCHIVE_BUTTON,
        INCLUDE_RETURNED_TOGGLE,
        DUE_SOON_TOGGLE,
        FUTURE_START_TOGGLE,
        BIG_CANDIDATES_TAB,
        MEDIUM_CANDIDATES_TAB,
        SMALL_CANDIDATES_TAB,
        PLAN_TAB,
        METER_TAB,
        RULES_TAB,
        RATIONALE_TAB,
        SWAP_IMPACT_TAB,
        BOARD_STATE_TAB,
        CANDIDATES_TAB,
        FROG_TAB,
        SWAP_TAB,
        FOCUS_CONFIRM_TAB,
        CANDIDATE_SEARCH_FIELD,
        FROG_SEARCH_FIELD,
        FOCUS_CONFIRM_TEXT,
        DIALOG_CLOSE,
        DIALOG_OK,
        DIALOG_CANCEL,
        DIALOG_SUBMIT,
        DELETE_CONFIRM,
        COMPLETE_DONE,
        COMPLETE_REJECT,
    ];

    pub(super) const CONTEXTS: &[BindingContext] = &[
        BindingContext {
            name: "app tabs",
            bindings: &[APP_TASKS_TAB, APP_CALENDAR_TAB],
        },
        BindingContext {
            name: "task workspace",
            bindings: &[
                TASK_QUICK_CREATE,
                TASK_VIEW_MENU,
                TASK_LABEL_FILTER,
                TASK_DELETE,
                TASK_DELETE_BACKSPACE,
                TASK_DELETE_CTRL_X,
                TASK_QUICK_MENU,
                TASK_MOVE_MODE,
                TASK_SNOOZE,
                TASK_COMPLETE,
                TASK_AGENT_YANK,
            ],
        },
        BindingContext {
            name: "task detail",
            bindings: &[
                TASK_COMPLETE,
                TASK_LABEL_FILTER,
                TASK_TITLE_FIELD,
                TASK_DESCRIPTION_FIELD,
                TASK_DESCRIPTION_EDITOR,
                TASK_STATE_FIELD,
                TASK_SIZE_FIELD,
                TASK_PRIORITY_FIELD,
                TASK_PEOPLE_FIELD,
                TASK_PROJECTS_FIELD,
                TASK_TAGS_FIELD,
                TASK_LINKS_FIELD,
                TASK_LINK_DELETE,
                TASK_START_DATE_FIELD,
                TASK_END_DATE_FIELD,
                TASK_SNOOZED_UNTIL_FIELD,
                DETAIL_CLOSE,
                DETAIL_CLOSE_ALT,
            ],
        },
        BindingContext {
            name: "people management",
            bindings: &[
                MANAGEMENT_CREATE,
                MANAGEMENT_DELETE,
                MANAGEMENT_DELETE_BACKSPACE,
                MANAGEMENT_DELETE_X,
                PERSON_NAME_FIELD,
                PERSON_EMAIL_FIELD,
                PERSON_ABOUT_FIELD,
                PERSON_ABOUT_EDITOR,
                PERSON_ACTIVE_FIELD,
                DETAIL_CLOSE,
                DETAIL_CLOSE_ALT,
            ],
        },
        BindingContext {
            name: "project management",
            bindings: &[
                MANAGEMENT_CREATE,
                MANAGEMENT_DELETE,
                MANAGEMENT_DELETE_BACKSPACE,
                MANAGEMENT_DELETE_X,
                PROJECT_KEY_FIELD,
                PROJECT_NAME_FIELD,
                PROJECT_DESCRIPTION_FIELD,
                PROJECT_DESCRIPTION_EDITOR,
                PROJECT_LEAD_FIELD,
                DETAIL_CLOSE,
                DETAIL_CLOSE_ALT,
            ],
        },
        BindingContext {
            name: "tag management",
            bindings: &[
                MANAGEMENT_CREATE,
                MANAGEMENT_DELETE,
                MANAGEMENT_DELETE_BACKSPACE,
                MANAGEMENT_DELETE_X,
                TAG_LABEL_FIELD,
                DETAIL_CLOSE,
                DETAIL_CLOSE_ALT,
            ],
        },
        BindingContext {
            name: "create dialog",
            bindings: &[DIALOG_OK, DIALOG_CANCEL, DIALOG_SUBMIT],
        },
        BindingContext {
            name: "delete confirmation",
            bindings: &[DELETE_CONFIRM, DIALOG_CLOSE],
        },
        BindingContext {
            name: "complete task dialog",
            bindings: &[COMPLETE_DONE, COMPLETE_REJECT, DIALOG_CANCEL, DIALOG_CLOSE],
        },
    ];
}

#[cfg(test)]
mod tests {
    use super::*;
    use tuicore::KeyEvent;

    #[test]
    fn valid_keys_parse_to_tuicore_labels() {
        assert_eq!(parse_key("a").unwrap().label(), "a");
        assert_eq!(parse_key("space").unwrap().label(), "Space");
        assert_eq!(parse_key("ctrl+space").unwrap().label(), "⌃Space");
        assert_eq!(parse_key("esc").unwrap().label(), "Esc");
    }

    #[test]
    fn invalid_keys_are_rejected() {
        assert!(parse_key("").is_none());
        assert!(parse_key("shift+a").is_none());
        assert!(parse_key("ctrl+").is_none());
        assert!(parse_key("enter now").is_none());
    }

    #[test]
    fn labels_use_configured_override_specs() {
        let keymap =
            AppKeymap::from_overrides([("APP_TASKS_TAB".into(), "ctrl+space".into())]).unwrap();
        assert_eq!(
            keymap
                .binding("APP_TASKS_TAB")
                .unwrap()
                .spec
                .unwrap()
                .label(),
            "⌃Space"
        );
        assert_eq!(keymap.binding("APP_TASKS_TAB").unwrap().raw, "ctrl+space");
    }

    #[test]
    fn task_detail_hotkeys_use_requested_sequence_defaults() {
        let keymap = AppKeymap::from_overrides(std::iter::empty::<(String, String)>()).unwrap();
        let expected = [
            ("TASK_TITLE_FIELD", "ti"),
            ("TASK_DESCRIPTION_FIELD", "dd"),
            ("TASK_DESCRIPTION_EDITOR", "do"),
            ("TASK_STATE_FIELD", "st"),
            ("TASK_SIZE_FIELD", "si"),
            ("TASK_PRIORITY_FIELD", "pri"),
            ("TASK_PEOPLE_FIELD", "pe"),
            ("TASK_PROJECTS_FIELD", "pro"),
            ("TASK_TAGS_FIELD", "ta"),
            ("TASK_LINKS_FIELD", "ur"),
            ("TASK_SNOOZED_UNTIL_FIELD", "su"),
        ];

        for (name, default) in expected {
            let binding = keymap.binding(name).unwrap();
            assert_eq!(binding.raw, default);
            assert!(
                binding.spec.is_none(),
                "{name} should be a sequence binding"
            );
        }
    }

    #[test]
    fn management_detail_hotkeys_use_requested_sequence_defaults() {
        let keymap = AppKeymap::from_overrides(std::iter::empty::<(String, String)>()).unwrap();
        let expected = [
            ("PERSON_NAME_FIELD", "am"),
            ("PERSON_EMAIL_FIELD", "em"),
            ("PERSON_ABOUT_FIELD", "ab"),
            ("PERSON_ABOUT_EDITOR", "ao"),
            ("PERSON_ACTIVE_FIELD", "ac"),
            ("PROJECT_KEY_FIELD", "ke"),
            ("PROJECT_NAME_FIELD", "am"),
            ("PROJECT_DESCRIPTION_FIELD", "dd"),
            ("PROJECT_DESCRIPTION_EDITOR", "do"),
            ("PROJECT_LEAD_FIELD", "ea"),
            ("TAG_LABEL_FIELD", "ab"),
        ];

        for (name, default) in expected {
            let binding = keymap.binding(name).unwrap();
            assert_eq!(binding.raw, default);
            assert!(
                binding.spec.is_none(),
                "{name} should be a sequence binding"
            );
        }
    }

    #[test]
    fn detail_hotkeys_leave_data_view_scroll_right_key_unclaimed() {
        let keymap = AppKeymap::from_overrides(std::iter::empty::<(String, String)>()).unwrap();

        for context in keys::CONTEXTS
            .iter()
            .filter(|context| matches!(context.name, "task detail" | "tag management"))
        {
            assert!(context.bindings.iter().all(|binding| {
                !binding_pattern(*binding, &keymap.bindings[binding.name])
                    .first()
                    .is_some_and(|key| key == "l")
            }));
        }
    }

    #[test]
    fn task_and_management_shortcuts_use_requested_defaults() {
        let keymap = AppKeymap::from_overrides(std::iter::empty::<(String, String)>()).unwrap();
        for (name, expected) in [
            ("TASK_LABEL_FILTER", "a"),
            ("TASK_SNOOZE", "ctrl+z"),
            ("TASK_COMPLETE", "ctrl+c"),
            ("TASK_DELETE_CTRL_X", "ctrl+x"),
            ("TASK_DELETE", "delete"),
            ("TASK_DELETE_X", "backspace"),
            ("MANAGEMENT_DELETE_X", "ctrl+x"),
            ("MANAGEMENT_DELETE", "delete"),
            ("MANAGEMENT_DELETE_ALT", "backspace"),
        ] {
            assert_eq!(keymap.binding(name).unwrap().raw, expected);
        }
        assert_eq!(
            keymap.binding("TASK_SNOOZE").unwrap().spec.unwrap().label(),
            "⌃z"
        );
    }

    #[test]
    fn event_matching_uses_tuicore_keyspec_semantics() {
        let binding = AppBinding::new("TEST", "ctrl+a");
        let event = TuiEvent::Key(KeyEvent {
            code: Key::Char('a'),
            modifiers: KeyModifiers::CONTROL,
        });
        let non_key = TuiEvent::Paste("a".into());

        assert!(binding.matches(&event));
        assert!(!binding.matches(&non_key));
    }

    #[test]
    fn override_config_rejects_unknown_and_invalid_keys() {
        assert!(AppKeymap::from_overrides([("NOPE".into(), "a".into())]).is_err());
        assert!(
            AppKeymap::from_overrides([("APP_TASKS_TAB".into(), "ctrl+escape".into())]).is_err()
        );
    }

    #[test]
    fn active_context_rejects_duplicate_bindings() {
        let error = AppKeymap::from_overrides([("TASK_VIEW_MENU".into(), "n".into())]).unwrap_err();

        assert!(error.to_string().contains("task workspace context"));
    }

    #[test]
    fn active_context_rejects_prefix_ambiguous_bindings() {
        let error =
            AppKeymap::from_overrides([("TASK_STATE_FIELD".into(), "t".into())]).unwrap_err();

        assert!(error.to_string().contains("task detail context"));
    }

    #[test]
    fn complete_dialog_rejects_duplicate_action_bindings() {
        let error =
            AppKeymap::from_overrides([("COMPLETE_REJECT".into(), "d".into())]).unwrap_err();

        assert!(error.to_string().contains("complete task dialog context"));
    }

    #[test]
    fn task_detail_rejects_complete_binding_collision() {
        let error =
            AppKeymap::from_overrides([("TASK_COMPLETE".into(), "esc".into())]).unwrap_err();

        assert!(error.to_string().contains("task detail context"));
    }

    #[test]
    fn bindings_in_separate_contexts_may_share_prefixes() {
        AppKeymap::from_overrides([
            ("APP_TASKS_TAB".into(), "t".into()),
            ("TASK_TITLE_FIELD".into(), "ti".into()),
        ])
        .unwrap();
    }

    #[test]
    fn dialog_overrides_supply_labels_and_matching_specs() {
        let keymap = AppKeymap::from_overrides([
            ("DIALOG_OK".into(), "y".into()),
            ("DIALOG_CANCEL".into(), "n".into()),
            ("DIALOG_SUBMIT".into(), "ctrl+s".into()),
            ("DELETE_CONFIRM".into(), "x".into()),
            ("COMPLETE_DONE".into(), "g".into()),
            ("COMPLETE_REJECT".into(), "j".into()),
        ])
        .unwrap();

        for (name, label, key) in [
            ("DIALOG_OK", "y", Key::Char('y')),
            ("DIALOG_CANCEL", "n", Key::Char('n')),
            ("DELETE_CONFIRM", "x", Key::Char('x')),
            ("COMPLETE_DONE", "g", Key::Char('g')),
            ("COMPLETE_REJECT", "j", Key::Char('j')),
        ] {
            let binding = keymap.binding(name).unwrap().spec.unwrap();
            assert_eq!(binding.label(), label);
            assert!(binding.matches(KeyEvent::from(key)));
        }
        let submit = keymap.binding("DIALOG_SUBMIT").unwrap().spec.unwrap();
        assert_eq!(submit.label(), "⌃s");
        assert!(submit.matches(KeyEvent {
            code: Key::Char('s'),
            modifiers: KeyModifiers::CONTROL,
        }));
    }

    #[test]
    fn complete_flow_bindings_cannot_shadow_runtime_quit() {
        for (name, key) in [
            (
                "TASK_COMPLETE",
                KeyEvent {
                    code: Key::Char('c'),
                    modifiers: KeyModifiers::CONTROL,
                },
            ),
            ("COMPLETE_DONE", KeyEvent::from(Key::Char('d'))),
            ("DIALOG_CANCEL", KeyEvent::from(Key::Char('c'))),
        ] {
            let keymap = AppKeymap::from_overrides(std::iter::empty::<(String, String)>()).unwrap();
            let runtime = tuicore::RuntimeKeyBindings::new().with_quit([KeySpec::from(key)]);

            let error = keymap.validate_runtime_quit(&runtime).unwrap_err();

            assert!(error.to_string().contains(name));
            assert!(error.to_string().contains("runtime quit"));
        }
    }

    #[test]
    fn default_runtime_quit_does_not_conflict_with_task_complete() {
        let keymap = AppKeymap::from_overrides(std::iter::empty::<(String, String)>()).unwrap();

        keymap
            .validate_runtime_quit(&tuicore::RuntimeKeyBindings::default())
            .unwrap();
    }
}
