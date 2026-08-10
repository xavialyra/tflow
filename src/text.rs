pub(crate) fn matches_query(text: &str, query: &str) -> bool {
    let folded = text.to_lowercase();
    query
        .split_whitespace()
        .all(|token| folded.contains(&token.to_lowercase()))
}

pub(crate) fn sanitize_text(text: &str) -> String {
    sanitize_terminal_text(text).trim().to_string()
}

pub(crate) fn sanitize_terminal_text(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut escape = false;
    let mut csi = false;
    let mut osc = false;
    let mut osc_escape = false;

    for character in text.chars() {
        if osc {
            if osc_escape {
                osc_escape = false;
                if character == '\\' {
                    osc = false;
                }
            } else if character == '\u{7}' {
                osc = false;
            } else if character == '\u{1b}' {
                osc_escape = true;
            }
            continue;
        }
        if csi {
            if ('@'..='~').contains(&character) {
                csi = false;
            }
            continue;
        }
        if escape {
            escape = false;
            match character {
                '[' => csi = true,
                ']' => osc = true,
                _ => {}
            }
            continue;
        }
        if character == '\u{1b}' {
            escape = true;
            continue;
        }
        if character.is_control() {
            if character == '\t' {
                output.push(' ');
            }
            continue;
        }
        output.push(character);
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_all_query_tokens() {
        assert!(matches_query("Restart API service", "api start"));
        assert!(!matches_query("Restart API service", "api database"));
    }

    #[test]
    fn terminal_sanitizer_preserves_printable_spacing() {
        assert_eq!(
            sanitize_terminal_text(" > \u{1b}[31mred\u{1b}[0m "),
            " > red "
        );
    }

    #[test]
    fn strips_terminal_controls() {
        assert_eq!(sanitize_text("\u{1b}[31mred\u{1b}[0m\n"), "red");
    }
}
