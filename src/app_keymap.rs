use std::{collections::HashMap, env, error::Error, fmt, sync::OnceLock};

use tuicore::{Key, KeyModifiers, KeySpec, TuiEvent};

static KEYMAP: OnceLock<AppKeymap> = OnceLock::new();

#[derive(Debug, Clone, Copy)]
pub struct AppBinding {
    name: &'static str,
    default: &'static str,
}

impl AppBinding {
    pub const fn new(name: &'static str, default: &'static str) -> Self {
        Self { name, default }
    }

    pub fn hotkey(self) -> String {
        self.resolved().raw.to_string()
    }

    pub fn label(self) -> String {
        self.resolved().spec.label()
    }

    pub fn matches(self, event: &TuiEvent) -> bool {
        let TuiEvent::Key(key) = event else {
            return false;
        };

        self.resolved().spec.matches(*key)
    }

    fn spec(self) -> Result<KeySpec, AppKeymapError> {
        parse_key(self.default).ok_or_else(|| AppKeymapError::invalid(self.name, self.default))
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
                .spec()
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
            let spec =
                parse_key(&raw).ok_or_else(|| AppKeymapError::invalid(binding.name, &raw))?;
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
    spec: KeySpec,
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

    pub const A: AppBinding = AppBinding::new("A", "a");
    pub const B: AppBinding = AppBinding::new("B", "b");
    pub const C: AppBinding = AppBinding::new("C", "c");
    pub const D: AppBinding = AppBinding::new("D", "d");
    pub const E: AppBinding = AppBinding::new("E", "e");
    pub const F: AppBinding = AppBinding::new("F", "f");
    pub const G: AppBinding = AppBinding::new("G", "g");
    pub const H: AppBinding = AppBinding::new("H", "h");
    pub const I: AppBinding = AppBinding::new("I", "i");
    pub const M: AppBinding = AppBinding::new("M", "m");
    pub const N: AppBinding = AppBinding::new("N", "n");
    pub const O: AppBinding = AppBinding::new("O", "o");
    pub const P: AppBinding = AppBinding::new("P", "p");
    pub const Q: AppBinding = AppBinding::new("Q", "q");
    pub const R: AppBinding = AppBinding::new("R", "r");
    pub const S: AppBinding = AppBinding::new("S", "s");
    pub const T: AppBinding = AppBinding::new("T", "t");
    pub const U: AppBinding = AppBinding::new("U", "u");
    pub const V: AppBinding = AppBinding::new("V", "v");
    pub const X: AppBinding = AppBinding::new("X", "x");
    pub const Z: AppBinding = AppBinding::new("Z", "z");
    pub const ONE: AppBinding = AppBinding::new("ONE", "1");
    pub const TWO: AppBinding = AppBinding::new("TWO", "2");
    pub const THREE: AppBinding = AppBinding::new("THREE", "3");
    pub const COLON: AppBinding = AppBinding::new("COLON", ":");
    pub const SLASH: AppBinding = AppBinding::new("SLASH", "/");
    pub const QUESTION: AppBinding = AppBinding::new("QUESTION", "?");
    pub const ESC: AppBinding = AppBinding::new("ESC", "esc");
    pub const CTRL_LEFT_BRACKET: AppBinding = AppBinding::new("CTRL_LEFT_BRACKET", "ctrl+[");

    pub const ALL: &[AppBinding] = &[
        A,
        B,
        C,
        D,
        E,
        F,
        G,
        H,
        I,
        M,
        N,
        O,
        P,
        Q,
        R,
        S,
        T,
        U,
        V,
        X,
        Z,
        ONE,
        TWO,
        THREE,
        COLON,
        SLASH,
        QUESTION,
        ESC,
        CTRL_LEFT_BRACKET,
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
        let keymap = AppKeymap::from_overrides([("I".into(), "ctrl+space".into())]).unwrap();
        assert_eq!(keymap.binding("I").unwrap().spec.label(), "⌃Space");
        assert_eq!(keymap.binding("I").unwrap().raw, "ctrl+space");
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
        assert!(AppKeymap::from_overrides([("I".into(), "ctrl+enter".into())]).is_err());
    }
}
