use std::time::Duration;

use tuicore::SpeedReader;

pub(crate) const SPEED_READER_WPM_SETTING: &str = "speed_reader.wpm";
pub(crate) const SPEED_READER_MARKDOWN_BLOCK_PAUSE_SETTING: &str =
    "speed_reader.markdown_block_pause_ms";
pub(crate) const MIN_SPEED_READER_WPM: u16 = 100;
pub(crate) const MAX_SPEED_READER_WPM: u16 = 1000;
pub(crate) const MAX_MARKDOWN_BLOCK_PAUSE_MS: u64 = 60_000;
const DEFAULT_SPEED_READER_WPM: u16 = 600;
const DEFAULT_MARKDOWN_BLOCK_PAUSE_MS: u64 = 250;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SpeedReaderSettings {
    pub wpm: u16,
    pub markdown_block_pause: Duration,
}

impl Default for SpeedReaderSettings {
    fn default() -> Self {
        Self {
            wpm: DEFAULT_SPEED_READER_WPM,
            markdown_block_pause: Duration::from_millis(DEFAULT_MARKDOWN_BLOCK_PAUSE_MS),
        }
    }
}

impl SpeedReaderSettings {
    pub(crate) fn apply(self, reader: SpeedReader) -> SpeedReader {
        reader
            .wpm(self.wpm)
            .markdown_block_pause(self.markdown_block_pause)
    }
}

pub(crate) fn parse_speed_reader_wpm(value: Option<&str>) -> Result<u16, String> {
    let Some(value) = value else {
        return Ok(DEFAULT_SPEED_READER_WPM);
    };
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid_wpm(value));
    }
    let wpm = value.parse::<u16>().map_err(|_| invalid_wpm(value))?;
    if !(MIN_SPEED_READER_WPM..=MAX_SPEED_READER_WPM).contains(&wpm) {
        return Err(invalid_wpm(value));
    }
    Ok(wpm)
}

pub(crate) fn parse_markdown_block_pause(value: Option<&str>) -> Result<Duration, String> {
    let Some(value) = value else {
        return Ok(Duration::from_millis(DEFAULT_MARKDOWN_BLOCK_PAUSE_MS));
    };
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid_block_pause(value));
    }
    let milliseconds = value
        .parse::<u64>()
        .map_err(|_| invalid_block_pause(value))?;
    if milliseconds > MAX_MARKDOWN_BLOCK_PAUSE_MS {
        return Err(invalid_block_pause(value));
    }
    Ok(Duration::from_millis(milliseconds))
}

pub(crate) fn format_speed_reader_wpm(value: u16) -> String {
    value.to_string()
}

pub(crate) fn format_markdown_block_pause(value: Duration) -> String {
    value.as_millis().to_string()
}

fn invalid_wpm(value: &str) -> String {
    format!(
        "invalid value for {SPEED_READER_WPM_SETTING}: {value} (expected {MIN_SPEED_READER_WPM}..={MAX_SPEED_READER_WPM})"
    )
}

fn invalid_block_pause(value: &str) -> String {
    format!(
        "invalid value for {SPEED_READER_MARKDOWN_BLOCK_PAUSE_SETTING}: {value} (expected 0..={MAX_MARKDOWN_BLOCK_PAUSE_MS})"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_values_use_speed_reader_defaults() {
        assert_eq!(parse_speed_reader_wpm(None), Ok(600));
        assert_eq!(
            parse_markdown_block_pause(None),
            Ok(Duration::from_millis(250))
        );
    }

    #[test]
    fn speed_reader_wpm_accepts_only_tuicore_range() {
        for value in ["100", "300", "1000"] {
            assert!(parse_speed_reader_wpm(Some(value)).is_ok());
        }
        for value in ["", "99", "1001", "300.0", "+300", " 300"] {
            assert!(parse_speed_reader_wpm(Some(value)).is_err(), "{value}");
        }
    }

    #[test]
    fn markdown_block_pause_accepts_only_millisecond_range() {
        for value in ["0", "250", "60000"] {
            assert!(parse_markdown_block_pause(Some(value)).is_ok());
        }
        for value in ["", "60001", "250.0", "+250", " 250"] {
            assert!(parse_markdown_block_pause(Some(value)).is_err(), "{value}");
        }
    }

    #[test]
    fn typed_values_format_as_numeric_strings() {
        assert_eq!(format_speed_reader_wpm(425), "425");
        assert_eq!(
            format_markdown_block_pause(Duration::from_millis(1_250)),
            "1250"
        );
    }
}
