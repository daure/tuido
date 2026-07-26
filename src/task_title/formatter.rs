use super::is_action_verb;

#[derive(Debug)]
struct Token {
    value: String,
    protected: bool,
    corrected: bool,
}

pub(crate) fn format_title(value: &str) -> String {
    let sanitized = sanitize(value);
    let mut tokens = sanitized
        .split_whitespace()
        .map(|value| Token {
            value: value.to_string(),
            protected: is_protected(value),
            corrected: false,
        })
        .collect::<Vec<_>>();

    protect_delimited_spans(&mut tokens);
    correct_unambiguous_contractions(&mut tokens);
    correct_contextual_contractions(&mut tokens);
    capitalize_first_plain_token(&mut tokens);
    trim_terminal_prose_punctuation(&mut tokens);

    tokens
        .into_iter()
        .map(|token| token.value)
        .collect::<Vec<_>>()
        .join(" ")
}

fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_whitespace() || is_c0_or_c1_control(character) {
                ' '
            } else {
                character
            }
        })
        .collect()
}

fn is_c0_or_c1_control(character: char) -> bool {
    matches!(character as u32, 0x00..=0x1f | 0x7f..=0x9f)
}

fn is_protected(token: &str) -> bool {
    has_technical_marker(token)
        || is_issue_reference(token)
        || is_version(token)
        || is_dotted_technical_token(token)
        || is_quoted_literal(token)
        || has_internal_mixed_case(token)
        || is_all_uppercase_word(token)
}

fn has_technical_marker(token: &str) -> bool {
    token.contains("://")
        || token.to_ascii_lowercase().contains("www.")
        || token.contains('@')
        || token.contains('/')
        || token.contains('\\')
        || token.contains("::")
        || token.contains(['_', '`', '{', '}', '[', ']'])
        || token.starts_with("--")
        || token.contains('=')
}

fn is_issue_reference(token: &str) -> bool {
    let token = token.trim_end_matches(['.', ',', ';', '!', '?', ':']);
    if let Some(number) = token.strip_prefix('#') {
        return !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit());
    }

    let Some((prefix, number)) = token.rsplit_once('-') else {
        return false;
    };
    prefix.len() >= 2
        && prefix.bytes().all(|byte| byte.is_ascii_uppercase())
        && !number.is_empty()
        && number.bytes().all(|byte| byte.is_ascii_digit())
}

fn is_version(token: &str) -> bool {
    let token = token.trim_end_matches(['.', ',', ';', '!', '?', ':']);
    let token = token
        .strip_prefix('v')
        .or_else(|| token.strip_prefix('V'))
        .unwrap_or(token);
    token.contains('.')
        && token
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

fn is_dotted_technical_token(token: &str) -> bool {
    let token = token.trim_end_matches(['.', ',', ';', '!', '?', ':']);
    let mut segments = token.split('.');
    let segment_count = segments
        .by_ref()
        .take_while(|segment| {
            !segment.is_empty() && segment.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
        .count();
    segment_count >= 2 && segment_count == token.split('.').count()
}

fn is_quoted_literal(token: &str) -> bool {
    let token = token.trim_end_matches(['.', ',', ';']);
    (token.starts_with('"') && token.ends_with('"'))
        || (token.starts_with('\'') && token.ends_with('\''))
}

fn prose_word(token: &str) -> Option<(&str, &str)> {
    let word_end = token.trim_end_matches(['.', ',', '!', '?', ';', ':']).len();
    let word = &token[..word_end];
    word.bytes()
        .all(|byte| byte.is_ascii_alphabetic())
        .then_some((word, &token[word_end..]))
}

fn has_internal_mixed_case(token: &str) -> bool {
    let letters = token
        .bytes()
        .filter(|byte| byte.is_ascii_alphabetic())
        .collect::<Vec<_>>();
    let Some((&first, rest)) = letters.split_first() else {
        return false;
    };
    let titlecase =
        first.is_ascii_uppercase() && rest.iter().all(|character| character.is_ascii_lowercase());
    let lowercase =
        first.is_ascii_lowercase() && rest.iter().all(|character| character.is_ascii_lowercase());
    let uppercase =
        first.is_ascii_uppercase() && rest.iter().all(|character| character.is_ascii_uppercase());
    !(lowercase || uppercase || titlecase)
}

fn is_all_uppercase_word(token: &str) -> bool {
    let letters = token.bytes().filter(|byte| byte.is_ascii_alphabetic());
    letters.clone().count() >= 2 && letters.into_iter().all(|byte| byte.is_ascii_uppercase())
}

fn protect_delimited_spans(tokens: &mut [Token]) {
    let mut open_delimiter = None;

    for token in tokens {
        if let Some(delimiter) = open_delimiter {
            token.protected = true;
            if ends_with_delimiter(&token.value, delimiter) {
                open_delimiter = None;
            }
            continue;
        }

        let Some(delimiter) = token
            .value
            .chars()
            .next()
            .filter(|character| matches!(character, '"' | '\'' | '`'))
        else {
            continue;
        };
        token.protected = true;
        if !ends_with_delimiter(&token.value[delimiter.len_utf8()..], delimiter) {
            open_delimiter = Some(delimiter);
        }
    }
}

fn ends_with_delimiter(token: &str, delimiter: char) -> bool {
    token
        .trim_end_matches(['.', ',', ';', '!', '?', ':'])
        .ends_with(delimiter)
}

fn correct_unambiguous_contractions(tokens: &mut [Token]) {
    for token in tokens.iter_mut().filter(|token| !token.protected) {
        replace_prose_word(token, |word| match word.to_ascii_lowercase().as_str() {
            "youre" => Some("you're"),
            "dont" => Some("don't"),
            "isnt" => Some("isn't"),
            "theres" => Some("there's"),
            _ => None,
        });
    }
}

fn correct_contextual_contractions(tokens: &mut [Token]) {
    for index in 0..tokens.len() {
        if tokens[index].protected {
            continue;
        }

        let next_is_action = tokens
            .get(index + 1)
            .is_some_and(|token| !token.protected && is_action_verb(&token.value));
        if next_is_action {
            replace_prose_word(&mut tokens[index], |word| {
                match word.to_ascii_lowercase().as_str() {
                    "cant" => Some("can't"),
                    "wont" => Some("won't"),
                    "lets" if index == 0 => Some("let's"),
                    _ => None,
                }
            });
        }

        if next_is_action && is_clause_start(tokens, index) {
            replace_prose_word(&mut tokens[index], |word| {
                word.eq_ignore_ascii_case("ill").then_some("I'll")
            });
        }
    }
}

fn is_clause_start(tokens: &[Token], index: usize) -> bool {
    if index == 0 {
        return true;
    }
    let previous = &tokens[index - 1].value;
    matches!(
        previous.to_ascii_lowercase().as_str(),
        "and" | "then" | "," | ";"
    ) || previous.ends_with([',', ';'])
}

fn replace_prose_word(token: &mut Token, replacement: impl FnOnce(&str) -> Option<&'static str>) {
    let Some((word, suffix)) = prose_word(&token.value) else {
        return;
    };
    let Some(replacement) = replacement(word) else {
        return;
    };
    let replacement = if word
        .bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_uppercase())
    {
        capitalize(replacement)
    } else {
        replacement.to_string()
    };
    token.value = format!("{replacement}{suffix}");
    token.corrected = true;
}

fn capitalize_first_plain_token(tokens: &mut [Token]) {
    let Some(first) = tokens.first_mut().filter(|token| !token.protected) else {
        return;
    };
    if first.corrected {
        if first.value.as_bytes()[0].is_ascii_lowercase() {
            first.value = capitalize(&first.value);
        }
        return;
    }
    let Some((word, suffix)) = prose_word(&first.value) else {
        return;
    };
    if word.bytes().all(|byte| byte.is_ascii_lowercase()) {
        first.value = format!("{}{suffix}", capitalize(word));
    }
}

fn capitalize(value: &str) -> String {
    let mut bytes = value.as_bytes().to_vec();
    if let Some(first) = bytes.first_mut() {
        first.make_ascii_uppercase();
    }
    String::from_utf8(bytes).expect("ASCII prose word remains valid UTF-8")
}

fn trim_terminal_prose_punctuation(tokens: &mut Vec<Token>) {
    while let Some(last) = tokens.last_mut().filter(|token| !token.protected) {
        last.value = last.value.trim_end_matches(['.', ';', ',']).to_string();
        if !last.value.is_empty() {
            break;
        }
        tokens.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::format_title;

    #[test]
    fn formats_reported_enter_case() {
        assert_eq!(
            format_title("Theres five and ill do it"),
            "There's five and I'll do it"
        );
    }

    #[test]
    fn corrects_lowercase_and_titlecase_unambiguous_contractions() {
        assert_eq!(
            format_title("youre ready, Dont panic, it Isnt broken and theres time"),
            "You're ready, Don't panic, it Isn't broken and there's time"
        );
    }

    #[test]
    fn corrects_ambiguous_contractions_only_in_action_context() {
        assert_eq!(
            format_title("cant fix and wont deploy"),
            "Can't fix and won't deploy"
        );
        assert_eq!(format_title("lets fix login"), "Let's fix login");
        assert_eq!(format_title("ill do it"), "I'll do it");
        assert_eq!(format_title("ill bake a cake"), "I'll bake a cake");
        assert_eq!(format_title("ill fix login"), "I'll fix login");
        assert_eq!(
            format_title("work, ill do cleanup"),
            "Work, I'll do cleanup"
        );
        assert_eq!(format_title("wait and ill do it"), "Wait and I'll do it");
        assert_eq!(format_title("then ill do it"), "Then I'll do it");
    }

    #[test]
    fn preserves_ambiguous_words_and_uppercase_tokens() {
        for (input, expected) in [
            ("patient is ill", "Patient is ill"),
            ("ill effects", "Ill effects"),
            ("were well", "Were well"),
            ("shell", "Shell"),
            ("canter", "Canter"),
            ("cantilever", "Cantilever"),
            ("legal cant", "Legal cant"),
            ("DONT change", "DONT change"),
            ("CANT fix", "CANT fix"),
            ("WONT deploy", "WONT deploy"),
            ("lets users sign in", "Lets users sign in"),
            ("wont wait", "Wont wait"),
        ] {
            assert_eq!(format_title(input), expected, "{input}");
        }
    }

    #[test]
    fn preserves_every_protected_token_class() {
        for input in [
            "iOS release",
            "https://example.com/dont.",
            "www.example.com",
            "name@example.com",
            "src/dont/file.rs",
            "C:\\Temp\\dont",
            "std::io",
            "API_ID",
            "`dont.`",
            "{dont}",
            "[dont]",
            "#42...",
            "ABC-42,",
            "--force",
            "FOO=bar",
            "v1.2.",
            "1.2.3",
            "eBay listing",
            "camelCase value",
            "API release",
            "API-V2.",
            "dont.rs",
            "\"dont.\"",
            "\"dont.\",",
        ] {
            assert_eq!(format_title(input), input, "{input}");
        }
    }

    #[test]
    fn preserves_every_token_in_quoted_and_backtick_spans() {
        for input in [
            "keep \"dont theres ill do\" unchanged",
            "keep 'dont theres ill do' unchanged",
            "keep `dont theres ill do` unchanged",
            "keep \"dont theres ill do through end",
        ] {
            assert_eq!(
                format_title(input),
                format!("Keep {}", &input[5..]),
                "{input}"
            );
        }
        assert_eq!(
            format_title("dont quote don't here"),
            "Don't quote don't here"
        );
    }

    #[test]
    fn preserves_multi_segment_dotted_tokens_with_terminal_punctuation() {
        assert_eq!(
            format_title("review docs.example.com."),
            "Review docs.example.com."
        );
        assert_eq!(
            format_title("review api.docs.example123.com,"),
            "Review api.docs.example123.com,"
        );
    }

    #[test]
    fn corrects_ill_before_titlecase_do_but_preserves_uppercase_do() {
        assert_eq!(format_title("ill Do it"), "I'll Do it");
        assert_eq!(format_title("ill DO it"), "Ill DO it");
    }

    #[test]
    fn capitalizes_only_plain_lowercase_first_tokens() {
        assert_eq!(format_title("implement feature x"), "Implement feature x");
        assert_eq!(format_title("patient is ill"), "Patient is ill");
        for protected in [
            "iOS release",
            "https://example.com",
            "eBay listing",
            "API work",
        ] {
            assert_eq!(format_title(protected), protected);
        }
    }

    #[test]
    fn removes_only_terminal_unprotected_prose_separators() {
        assert_eq!(format_title("fix it...;,"), "Fix it");
        assert_eq!(format_title("fix it?"), "Fix it?");
        assert_eq!(format_title("fix it!"), "Fix it!");
        for (input, expected) in [
            ("open https://example.com/a.", "Open https://example.com/a."),
            ("open src/file.", "Open src/file."),
            ("ship v1.2", "Ship v1.2"),
            ("fix #42...", "Fix #42..."),
            ("keep \"literal.\"", "Keep \"literal.\""),
            ("keep `literal.`", "Keep `literal.`"),
        ] {
            assert_eq!(format_title(input), expected);
        }
    }

    #[test]
    fn sanitizes_controls_without_joining_adjacent_words() {
        assert_eq!(format_title("fix\u{0}login\u{85}now"), "Fix login now");
    }

    #[test]
    fn normalizes_whitespace_runs_and_trims() {
        assert_eq!(
            format_title("  implement\t  feature x  "),
            "Implement feature x"
        );
    }

    #[test]
    fn formatting_is_idempotent() {
        for input in [
            "  fix   youre issue,  ",
            "Theres five and ill do it",
            "cant deploy release...",
            "patient is ill",
            "iOS release",
            "review https://example.com/dont.",
            "keep \"dont theres ill do\" unchanged",
            "review docs.example.com.",
            "ill Do it",
            "ill DO it",
            "fix\u{7f}API_ID",
        ] {
            let once = format_title(input);
            assert_eq!(format_title(&once), once, "{input}");
        }
    }
}
