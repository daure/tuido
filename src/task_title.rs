#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TitleLevel {
    Bad,
    Okay,
    Good,
    Perfect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TitleCheck {
    pub(crate) label: &'static str,
    pub(crate) level: TitleLevel,
}

pub(crate) fn evaluate_title(value: &str) -> [TitleCheck; 3] {
    let title = value.trim();
    [
        TitleCheck {
            label: "Starts with a verb",
            level: starts_with_verb_level(title),
        },
        TitleCheck {
            label: "No second action detected",
            level: one_action_level(title),
        },
        TitleCheck {
            label: "3-8 words for quick scanning",
            level: word_count_level(title),
        },
    ]
}

fn normalized_word(word: &str) -> String {
    word.trim_matches(|character: char| !character.is_alphabetic())
        .to_ascii_lowercase()
}

fn is_action_verb(word: &str) -> bool {
    ACTION_VERBS
        .binary_search(&normalized_word(word).as_str())
        .is_ok()
}

fn is_unambiguous_second_action(word: &str) -> bool {
    let word = normalized_word(word);
    is_action_verb(&word)
        && !matches!(
            word.as_str(),
            "account" | "design" | "plan" | "report" | "research" | "review" | "support"
        )
}

fn starts_with_action(title: &str) -> bool {
    let mut words = title.split_whitespace();
    match words.next() {
        Some(first) if is_action_verb(first) => true,
        Some(first)
            if first.eq_ignore_ascii_case("let's") || first.eq_ignore_ascii_case("lets") =>
        {
            words.next().is_some_and(is_action_verb)
        }
        _ => false,
    }
}

fn starts_with_verb_level(title: &str) -> TitleLevel {
    if title.is_empty() {
        TitleLevel::Bad
    } else if starts_with_action(title) {
        TitleLevel::Perfect
    } else {
        TitleLevel::Okay
    }
}

fn one_action_level(title: &str) -> TitleLevel {
    if title.is_empty() {
        return TitleLevel::Bad;
    }
    if has_strong_multiple_action_evidence(title) {
        return TitleLevel::Bad;
    }
    if starts_with_action(title) {
        TitleLevel::Perfect
    } else {
        TitleLevel::Good
    }
}

fn has_strong_multiple_action_evidence(title: &str) -> bool {
    let words = title.split_whitespace().collect::<Vec<_>>();
    let conjunction_evidence = words.windows(2).any(|pair| {
        (matches!(normalized_word(pair[0]).as_str(), "and" | "then")
            || matches!(pair[0], "/" | "&" | "+"))
            && is_unambiguous_second_action(pair[1])
    });
    conjunction_evidence
        || title
            .split([',', ';'])
            .skip(1)
            .filter_map(|clause| clause.split_whitespace().next())
            .any(is_unambiguous_second_action)
}

fn word_count_level(title: &str) -> TitleLevel {
    if title.is_empty() {
        return TitleLevel::Bad;
    }
    match title.split_whitespace().count() {
        0 => TitleLevel::Bad,
        1 => TitleLevel::Okay,
        2 => TitleLevel::Good,
        3..=8 => TitleLevel::Perfect,
        9..=10 => TitleLevel::Good,
        11..=12 => TitleLevel::Okay,
        _ => TitleLevel::Bad,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluation_uses_clear_evidence_without_blocking_ambiguous_titles() {
        let strong = evaluate_title("Fix login redirect");
        assert_eq!(strong[0].level, TitleLevel::Perfect);
        assert_eq!(strong[1].level, TitleLevel::Perfect);
        assert_eq!(strong[2].level, TitleLevel::Perfect);

        let ambiguous = evaluate_title("Login redirect");
        assert_eq!(ambiguous[0].level, TitleLevel::Okay);
        assert_eq!(ambiguous[1].level, TitleLevel::Good);

        let multiple = evaluate_title("Fix login and update docs");
        assert_eq!(multiple[1].level, TitleLevel::Bad);

        let comma_multiple = evaluate_title("Fix login, update docs");
        assert_eq!(comma_multiple[1].level, TitleLevel::Bad);

        for separator in ["/", "&", "+"] {
            let multiple = evaluate_title(&format!("Fix login {separator} update docs"));
            assert_eq!(multiple[1].level, TitleLevel::Bad);
        }

        let technical = evaluate_title("Review src/fix and API+client names");
        assert_ne!(technical[1].level, TitleLevel::Bad);
    }

    #[test]
    fn evaluation_does_not_treat_ambiguous_nouns_as_second_actions() {
        for title in [
            "Schedule call and support team",
            "Review customer and account notes",
            "Schedule meeting and design review",
            "Draft roadmap and plan summary",
            "Email metrics and report summary",
            "Summarize findings and research notes",
            "Schedule launch and review meeting",
        ] {
            assert_ne!(evaluate_title(title)[1].level, TitleLevel::Bad, "{title}");
        }
    }

    #[test]
    fn evaluation_recognizes_action_verbs_across_major_domains() {
        for title in [
            "Bake birthday cake",
            "Email project update",
            "Phone insurance company",
            "Kill the internal services",
            "Bring meeting notes",
            "Choose dinner recipe",
            "Drink more water",
            "Eat healthy lunch",
            "Get train tickets",
            "Go grocery shopping",
            "Make dentist appointment",
            "Meet design team",
            "Put bins outside",
            "Take morning medication",
            "Text apartment manager",
            "Fetch remote branch",
            "Pull latest changes",
            "Reboot office router",
            "Restart web server",
            "Sync project files",
            "Debug login failure",
            "Containerize web service",
            "Prototype checkout flow",
            "Wireframe account settings",
            "Reconcile monthly accounts",
            "Diagnose patient symptoms",
            "Calibrate pressure sensor",
            "Weld support bracket",
            "Illustrate book cover",
            "Ferment garden vegetables",
            "Prune apple tree",
            "Knit winter scarf",
            "Hike coastal trail",
            "Meditate before work",
            "Schedule team meeting",
        ] {
            assert_eq!(
                evaluate_title(title)[0].level,
                TitleLevel::Perfect,
                "{title}"
            );
        }
    }

    #[test]
    fn evaluation_recognizes_tuido_recommended_verbs() {
        for title in [
            "Spike auth options",
            "Decide release scope",
            "Scope migration work",
            "QA checkout flow",
            "Break down launch plan",
            "Follow up with vendor",
        ] {
            assert_eq!(
                evaluate_title(title)[0].level,
                TitleLevel::Perfect,
                "{title}"
            );
        }
    }

    #[test]
    fn evaluation_labels_describe_measured_proxies() {
        assert_eq!(
            evaluate_title("Fix login").map(|check| check.label),
            [
                "Starts with a verb",
                "No second action detected",
                "3-8 words for quick scanning",
            ]
        );
    }

    #[test]
    fn evaluation_uses_requested_word_count_bands() {
        for (word_count, expected) in [
            (0, TitleLevel::Bad),
            (1, TitleLevel::Okay),
            (2, TitleLevel::Good),
            (3, TitleLevel::Perfect),
            (8, TitleLevel::Perfect),
            (9, TitleLevel::Good),
            (10, TitleLevel::Good),
            (11, TitleLevel::Okay),
            (12, TitleLevel::Okay),
            (13, TitleLevel::Bad),
        ] {
            let title = vec!["word"; word_count].join(" ");
            assert_eq!(evaluate_title(&title)[2].level, expected, "{word_count}");
        }
    }
}
mod action_verbs;
mod formatter;

use action_verbs::ACTION_VERBS;
pub(crate) use formatter::format_title;
