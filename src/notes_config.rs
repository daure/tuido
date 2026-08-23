use std::{
    fs, io,
    path::Path,
    sync::{Mutex, OnceLock},
};

use toml::{Table, Value};

const NOTES_TABLE: &str = "notes";
const LEGACY_ZOOM_KEY: &str = "zoom";
const DESKTOP_ZOOM_KEY: &str = "desktop_zoom";
const NARROW_ZOOM_KEY: &str = "narrow_zoom";
static SAVE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
pub(crate) const MAX_NOTES_ZOOM: usize = 2;
pub(crate) const MAX_NARROW_NOTES_ZOOM: usize = 1;
pub(crate) const DEFAULT_NOTE_EDITING_SETTING: &str = "notes.default_editing";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum NoteEditingMode {
    #[default]
    Inline,
    External,
}

impl NoteEditingMode {
    pub(crate) const ALL: [Self; 2] = [Self::Inline, Self::External];

    pub(crate) fn setting_value(self) -> &'static str {
        match self {
            Self::Inline => "inline",
            Self::External => "external",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Inline => "Inline",
            Self::External => "External",
        }
    }
}

pub(crate) fn parse_note_editing_mode(
    value: Option<&str>,
) -> Result<NoteEditingMode, &'static str> {
    match value.unwrap_or(NoteEditingMode::Inline.setting_value()) {
        "inline" => Ok(NoteEditingMode::Inline),
        "external" => Ok(NoteEditingMode::External),
        _ => Err("default note editing must be inline or external"),
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct NotesZoomLevels {
    pub(crate) desktop: usize,
    pub(crate) narrow: usize,
}

pub(crate) fn load_notes_zoom() -> Result<NotesZoomLevels, Box<dyn std::error::Error>> {
    load_notes_zoom_from(&crate::paths::app_config_path()?)
}

pub(crate) fn save_notes_zoom(zoom: NotesZoomLevels) -> Result<(), Box<dyn std::error::Error>> {
    save_notes_zoom_to(&crate::paths::app_config_path()?, zoom)
}

fn load_notes_zoom_from(path: &Path) -> Result<NotesZoomLevels, Box<dyn std::error::Error>> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(NotesZoomLevels::default());
        }
        Err(error) => return Err(error.into()),
    };
    let table = contents.parse::<Table>()?;
    let notes = table.get(NOTES_TABLE).and_then(Value::as_table);
    let zoom = |key| {
        notes
            .and_then(|notes| notes.get(key))
            .and_then(Value::as_integer)
            .and_then(|zoom| usize::try_from(zoom).ok())
    };
    let legacy_zoom = zoom(LEGACY_ZOOM_KEY).unwrap_or(0);
    Ok(NotesZoomLevels {
        desktop: zoom(DESKTOP_ZOOM_KEY)
            .unwrap_or(legacy_zoom)
            .min(MAX_NOTES_ZOOM),
        narrow: zoom(NARROW_ZOOM_KEY)
            .unwrap_or(legacy_zoom)
            .min(MAX_NARROW_NOTES_ZOOM),
    })
}

fn save_notes_zoom_to(
    path: &Path,
    zoom: NotesZoomLevels,
) -> Result<(), Box<dyn std::error::Error>> {
    let _lock = SAVE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| io::Error::other("notes config save lock poisoned"))?;
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.into()),
    };
    let mut table = contents.parse::<Table>()?;
    let notes = table
        .entry(NOTES_TABLE)
        .or_insert_with(|| Value::Table(Table::new()))
        .as_table_mut()
        .ok_or("notes config must be a TOML table")?;
    notes.remove(LEGACY_ZOOM_KEY);
    notes.insert(
        DESKTOP_ZOOM_KEY.to_string(),
        Value::Integer(zoom.desktop.min(MAX_NOTES_ZOOM) as i64),
    );
    notes.insert(
        NARROW_ZOOM_KEY.to_string(),
        Value::Integer(zoom.narrow.min(MAX_NARROW_NOTES_ZOOM) as i64),
    );
    let serialized = toml::to_string(&table)?;
    let parent = path.parent().ok_or("config path has no parent directory")?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("tuido"),
        uuid::Uuid::new_v4()
    ));
    let result = (|| -> io::Result<()> {
        let mut file = fs::File::create(&temporary)?;
        use std::io::Write;
        file.write_all(serialized.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_notes_zoom_without_discarding_other_config() {
        let path =
            std::env::temp_dir().join(format!("tuido-notes-config-{}.toml", uuid::Uuid::new_v4()));
        fs::write(&path, "[other]\nvalue = 1\n").unwrap();

        save_notes_zoom_to(
            &path,
            NotesZoomLevels {
                desktop: 2,
                narrow: 1,
            },
        )
        .unwrap();

        assert_eq!(
            load_notes_zoom_from(&path).unwrap(),
            NotesZoomLevels {
                desktop: 2,
                narrow: 1,
            }
        );
        let table = fs::read_to_string(&path).unwrap().parse::<Table>().unwrap();
        assert_eq!(table["other"]["value"].as_integer(), Some(1));
        assert_eq!(table[NOTES_TABLE][DESKTOP_ZOOM_KEY].as_integer(), Some(2));
        assert_eq!(table[NOTES_TABLE][NARROW_ZOOM_KEY].as_integer(), Some(1));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn loads_legacy_zoom_into_each_layout() {
        let path =
            std::env::temp_dir().join(format!("tuido-notes-config-{}.toml", uuid::Uuid::new_v4()));
        fs::write(&path, "[notes]\nzoom = 2\n").unwrap();

        assert_eq!(
            load_notes_zoom_from(&path).unwrap(),
            NotesZoomLevels {
                desktop: 2,
                narrow: 1,
            }
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn invalid_config_is_preserved_when_zoom_save_fails() {
        let path =
            std::env::temp_dir().join(format!("tuido-notes-config-{}.toml", uuid::Uuid::new_v4()));
        let contents = "[notes\ninvalid";
        fs::write(&path, contents).unwrap();

        assert!(save_notes_zoom_to(&path, NotesZoomLevels::default()).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), contents);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn default_note_editing_is_inline_and_validates_stored_values() {
        assert_eq!(parse_note_editing_mode(None), Ok(NoteEditingMode::Inline));
        assert_eq!(NoteEditingMode::Inline.label(), "Inline");
        assert_eq!(NoteEditingMode::External.label(), "External");
        assert_eq!(
            parse_note_editing_mode(Some("external")),
            Ok(NoteEditingMode::External)
        );
        assert!(parse_note_editing_mode(Some("other")).is_err());
    }
}
