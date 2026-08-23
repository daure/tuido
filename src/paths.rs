use std::{env, ffi::OsString, path::PathBuf};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum UiConfigSource {
    Legacy,
    Directory(PathBuf),
    Defaults,
}

pub(crate) fn data_dir() -> Result<PathBuf, Box<dyn std::error::Error>> {
    #[cfg(target_os = "linux")]
    if let Some(path) = optional_path_env("XDG_DATA_HOME")? {
        return absolute_override("XDG_DATA_HOME", path);
    }

    dirs::data_local_dir().ok_or_else(|| "could not determine platform data directory".into())
}

pub(crate) fn ui_config_source() -> Result<UiConfigSource, Box<dyn std::error::Error>> {
    let home = env::var_os("HOME").map(PathBuf::from);
    let legacy_dir = home.as_ref().map(|home| home.join(".tuicore"));
    let legacy_files_exist = legacy_dir.as_ref().is_some_and(|dir| {
        dir.join("tui.toml").is_file() || dir.join("keybindings.toml").is_file()
    });
    Ok(select_ui_config_source(
        absolute_path_env("TUIDO_CONFIG_DIR")?,
        absolute_path_env("TUICORE_CONFIG_DIR")?,
        legacy_files_exist,
        dirs::config_dir().map(|path| path.join("tuido")),
    ))
}

pub(crate) fn app_config_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
    app_config_path_for(ui_config_source()?, dirs::home_dir(), dirs::config_dir())
}

fn app_config_path_for(
    source: UiConfigSource,
    home: Option<PathBuf>,
    config: Option<PathBuf>,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let directory = match source {
        UiConfigSource::Legacy => home
            .map(|home| home.join(".tuicore"))
            .ok_or("could not determine home directory")?,
        UiConfigSource::Directory(directory) => directory,
        UiConfigSource::Defaults => config
            .map(|directory| directory.join("tuido"))
            .ok_or("could not determine platform config directory")?,
    };
    Ok(directory.join("config.toml"))
}

fn select_ui_config_source(
    tuido_dir: Option<PathBuf>,
    tuicore_dir: Option<PathBuf>,
    legacy_files_exist: bool,
    native_dir: Option<PathBuf>,
) -> UiConfigSource {
    if let Some(path) = tuido_dir {
        UiConfigSource::Directory(path)
    } else if let Some(path) = tuicore_dir {
        UiConfigSource::Directory(path)
    } else if legacy_files_exist {
        UiConfigSource::Legacy
    } else if let Some(path) = native_dir {
        UiConfigSource::Directory(path)
    } else {
        UiConfigSource::Defaults
    }
}

pub(crate) fn absolute_path_env(name: &str) -> Result<Option<PathBuf>, Box<dyn std::error::Error>> {
    optional_path_env(name)?
        .map(|path| absolute_override(name, path))
        .transpose()
}

pub(crate) fn optional_path_env(name: &str) -> Result<Option<PathBuf>, Box<dyn std::error::Error>> {
    match env::var_os(name) {
        Some(value) => path_override(name, value).map(Some),
        None => Ok(None),
    }
}

fn path_override(name: &str, value: OsString) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if value.is_empty() {
        return Err(format!("{name} must not be empty").into());
    }
    Ok(PathBuf::from(value))
}

fn absolute_override(name: &str, path: PathBuf) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if !path.is_absolute() {
        return Err(format!("{name} must be an absolute path: {}", path.display()).into());
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_overrides_must_be_absolute() {
        for name in [
            "XDG_DATA_HOME",
            "TUIDO_CONFIG_DIR",
            "TUICORE_CONFIG_DIR",
            "TUIDO_MIGRATIONS_DIR",
        ] {
            assert_eq!(
                absolute_override(name, PathBuf::from("relative/data"))
                    .unwrap_err()
                    .to_string(),
                format!("{name} must be an absolute path: relative/data")
            );
        }
        assert_eq!(
            absolute_override("XDG_DATA_HOME", PathBuf::from("/var/lib/tuido")).unwrap(),
            PathBuf::from("/var/lib/tuido")
        );
    }

    #[test]
    fn empty_path_overrides_are_invalid() {
        assert_eq!(
            path_override("TUIDO_CONFIG_DIR", OsString::new())
                .unwrap_err()
                .to_string(),
            "TUIDO_CONFIG_DIR must not be empty"
        );
    }

    #[test]
    fn config_source_precedence() {
        let native = PathBuf::from("/home/test/.config/tuido");
        for (explicit, tuicore, legacy_present, native, expected) in [
            (
                None,
                Some(PathBuf::from("/home/test/tuicore")),
                false,
                Some(native.clone()),
                UiConfigSource::Directory(PathBuf::from("/home/test/tuicore")),
            ),
            (
                None,
                None,
                true,
                Some(native.clone()),
                UiConfigSource::Legacy,
            ),
            (
                None,
                None,
                false,
                Some(native.clone()),
                UiConfigSource::Directory(native),
            ),
            (None, None, false, None, UiConfigSource::Defaults),
            (
                Some(PathBuf::from("/etc/tuido")),
                Some(PathBuf::from("/home/test/tuicore")),
                true,
                None,
                UiConfigSource::Directory(PathBuf::from("/etc/tuido")),
            ),
        ] {
            assert_eq!(
                select_ui_config_source(explicit, tuicore, legacy_present, native),
                expected
            );
        }
    }

    #[test]
    fn app_config_path_uses_concrete_ui_config_directory() {
        assert_eq!(
            app_config_path_for(
                select_ui_config_source(
                    None,
                    Some(PathBuf::from("/tmp/tuicore-override")),
                    false,
                    Some(PathBuf::from("/home/test/.config/tuido")),
                ),
                Some(PathBuf::from("/home/test")),
                Some(PathBuf::from("/home/test/.config")),
            )
            .unwrap(),
            PathBuf::from("/tmp/tuicore-override/config.toml")
        );
    }
}
