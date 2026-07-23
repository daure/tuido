use std::{collections::HashMap, env, error::Error, fmt, sync::OnceLock};

use tuicore::{Key, KeyModifiers, KeySpec, TuiEvent};

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
            bindings.insert(binding.name, ResolvedBinding { raw, spec });
        }

        if let Some(name) = overrides.keys().next() {
            return Err(AppKeymapError::unknown(name));
        }

        Ok(Self { bindings })
    }

    fn from_env() -> Result<Self, AppKeymapError> {
        Self::from_overrides(env_overrides()?)
    }

    fn binding(&self, name: &'static str) -> Option<ResolvedBinding> {
        self.bindings.get(name).cloned()
    }
}

#[derive(Debug, Clone)]
struct ResolvedBinding {
    raw: String,
    spec: Option<KeySpec>,
}

pub fn try_init() -> Result<(), AppKeymapError> {
    let keymap = AppKeymap::from_env()?;
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
    let value = value.trim().to_ascii_lowercase();

    if let Some(rest) = value.strip_prefix("ctrl+") {
        return modified_key(rest, KeyModifiers::CONTROL);
    }

    let code = match value.as_str() {
        "esc" => Key::Esc,
        "enter" => Key::Enter,
        "tab" => Key::Tab,
        "space" => Key::Char(' '),
        text => return single_char(text).map(KeySpec::plain),
    };

    Some(KeySpec::key(code))
}

fn modified_key(value: &str, modifiers: KeyModifiers) -> Option<KeySpec> {
    if value == "space" {
        return Some(KeySpec::key_with_modifiers(Key::Char(' '), modifiers));
    }
    single_char(value).map(|key| KeySpec::key_with_modifiers(Key::Char(key), modifiers))
}

fn single_char(value: &str) -> Option<char> {
    let mut chars = value.chars();
    let key = chars.next()?;
    chars.next().is_none().then_some(key)
}

pub mod keys {
    use super::AppBinding;

    pub const APP_INBOX_TAB: AppBinding = AppBinding::new("APP_INBOX_TAB", "i");
    pub const APP_TASKS_TAB: AppBinding = AppBinding::new("APP_TASKS_TAB", "t");
    pub const APP_NOTES_TAB: AppBinding = AppBinding::new("APP_NOTES_TAB", "n");
    pub const APP_CALENDAR_TAB: AppBinding = AppBinding::new("APP_CALENDAR_TAB", "c");
    pub const APP_PROJECTS_TAB: AppBinding = AppBinding::new_sequence("APP_PROJECTS_TAB", "pr");
    pub const APP_PEOPLE_TAB: AppBinding = AppBinding::new_sequence("APP_PEOPLE_TAB", "pe");
    pub const INBOX_CAPTURE: AppBinding = AppBinding::new("INBOX_CAPTURE", "i");
    pub const TASK_QUICK_CREATE: AppBinding = AppBinding::new("TASK_QUICK_CREATE", "+");
    pub const TASK_TITLE_FIELD: AppBinding = AppBinding::new("TASK_TITLE_FIELD", "e");
    pub const TASK_DESCRIPTION_FIELD: AppBinding = AppBinding::new("TASK_DESCRIPTION_FIELD", "d");
    pub const TASK_TYPE_FIELD: AppBinding = AppBinding::new("TASK_TYPE_FIELD", "p");
    pub const TASK_STATE_FIELD: AppBinding = AppBinding::new("TASK_STATE_FIELD", "s");
    pub const TASK_SIZE_FIELD: AppBinding = AppBinding::new("TASK_SIZE_FIELD", "x");
    pub const TASK_PEOPLE_FIELD: AppBinding = AppBinding::new("TASK_PEOPLE_FIELD", "z");
    pub const TASK_PROJECTS_FIELD: AppBinding =
        AppBinding::new_sequence("TASK_PROJECTS_FIELD", "ts");
    pub const TASK_START_DATE_FIELD: AppBinding =
        AppBinding::new_sequence("TASK_START_DATE_FIELD", "sd");
    pub const TASK_END_DATE_FIELD: AppBinding =
        AppBinding::new_sequence("TASK_END_DATE_FIELD", "ed");
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
    pub const SNOOZED_FILTER_TOGGLE: AppBinding = AppBinding::new("SNOOZED_FILTER_TOGGLE", "s");
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

    pub const ALL: &[AppBinding] = &[
        APP_INBOX_TAB,
        APP_TASKS_TAB,
        APP_NOTES_TAB,
        APP_CALENDAR_TAB,
        APP_PROJECTS_TAB,
        APP_PEOPLE_TAB,
        INBOX_CAPTURE,
        TASK_QUICK_CREATE,
        TASK_TITLE_FIELD,
        TASK_DESCRIPTION_FIELD,
        TASK_TYPE_FIELD,
        TASK_STATE_FIELD,
        TASK_SIZE_FIELD,
        TASK_PEOPLE_FIELD,
        TASK_PROJECTS_FIELD,
        TASK_START_DATE_FIELD,
        TASK_END_DATE_FIELD,
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
        SNOOZED_FILTER_TOGGLE,
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
            AppKeymap::from_overrides([("APP_INBOX_TAB".into(), "ctrl+space".into())]).unwrap();
        assert_eq!(
            keymap
                .binding("APP_INBOX_TAB")
                .unwrap()
                .spec
                .unwrap()
                .label(),
            "⌃Space"
        );
        assert_eq!(keymap.binding("APP_INBOX_TAB").unwrap().raw, "ctrl+space");
    }

    #[test]
    fn sequence_hotkeys_allow_multi_character_defaults() {
        let keymap = AppKeymap::from_overrides(std::iter::empty::<(String, String)>()).unwrap();
        assert_eq!(keymap.binding("TASK_START_DATE_FIELD").unwrap().raw, "sd");
        assert!(
            keymap
                .binding("TASK_START_DATE_FIELD")
                .unwrap()
                .spec
                .is_none()
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
            AppKeymap::from_overrides([("APP_INBOX_TAB".into(), "ctrl+enter".into())]).is_err()
        );
    }
}
