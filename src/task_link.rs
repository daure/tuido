use std::sync::OnceLock;

use regex::Regex;

pub(crate) const LINK_PATTERN: &str = r"^(?:[A-Za-z][A-Za-z0-9+.-]*:[^\s]+|www\.[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)+(?:\:[0-9]+)?(?:[/?#][^\s]*)?)$";

pub(crate) fn is_valid(value: &str) -> bool {
    static LINK: OnceLock<Regex> = OnceLock::new();
    LINK.get_or_init(|| Regex::new(LINK_PATTERN).expect("task link regex must compile"))
        .is_match(value)
}

pub(crate) fn icon(value: &str) -> &'static str {
    crate::link_icons::icon(value)
}

pub(crate) fn browser_target(value: &str) -> String {
    if value.starts_with("www.") {
        format!("https://{value}")
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_any_well_formed_protocol_and_rejects_bare_locations() {
        assert!(is_valid("file:///tmp/report.pdf"));
        assert!(is_valid("content://media/external/images/1"));
        assert!(is_valid("mailto:someone@example.com"));
        assert!(is_valid("custom+app://item/42"));
        assert!(is_valid("www.example.com/item?q=1"));
        assert!(!is_valid("example.com/item"));
        assert!(!is_valid("://missing-scheme"));
        assert!(!is_valid("https://contains whitespace"));
    }

    #[test]
    fn chooses_specific_icons_before_protocol_fallbacks() {
        assert_eq!(icon("https://www.airbnb.com/rooms/1"), "");
        assert_eq!(icon("https://team.atlassian.net/jira/software/ABC-1"), "");
        assert_eq!(icon("file:///tmp/report.pdf"), "󰈔");
        assert_eq!(icon("content://media/1"), "󰄡");
        assert_eq!(icon("https://example.com"), "󰖟");
    }

    #[test]
    fn browser_targets_add_https_to_www_links() {
        assert_eq!(
            browser_target("www.example.com/item"),
            "https://www.example.com/item"
        );
        assert_eq!(
            browser_target("https://example.com/item"),
            "https://example.com/item"
        );
    }
}
