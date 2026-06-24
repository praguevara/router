//! Tokenization for the semantic-introspection index.
//!
//! Splits schema text (coordinate names + descriptions) and search queries into
//! lowercase tokens, breaking on non-alphanumeric characters and on
//! camelCase / digit boundaries so that `availableTaxis` and `available_taxis`
//! both yield `["available", "taxis"]`.

/// Tokenizes `text` into lowercase alphanumeric tokens.
pub fn tokenize(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    // Whether the previous emitted char was a lowercase letter or a digit, used
    // to detect camelCase boundaries (a transition into an uppercase letter).
    let mut prev_lower_or_digit = false;

    for ch in text.chars() {
        if ch.is_alphanumeric() {
            if ch.is_uppercase() && prev_lower_or_digit && !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            for lc in ch.to_lowercase() {
                cur.push(lc);
            }
            prev_lower_or_digit = ch.is_lowercase() || ch.is_numeric();
        } else {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            prev_lower_or_digit = false;
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::tokenize;

    #[test]
    fn splits_camel_case() {
        assert_eq!(tokenize("availableTaxis"), vec!["available", "taxis"]);
    }

    #[test]
    fn splits_snake_case_and_punctuation() {
        assert_eq!(
            tokenize("nearest_station name"),
            vec!["nearest", "station", "name"]
        );
    }

    #[test]
    fn lowercases_and_splits_digits() {
        assert_eq!(tokenize("v2Engine"), vec!["v2", "engine"]);
    }

    #[test]
    fn handles_descriptions() {
        assert_eq!(
            tokenize("Returns the number of available taxis."),
            vec!["returns", "the", "number", "of", "available", "taxis"]
        );
    }

    #[test]
    fn empty_input() {
        assert!(tokenize("   ").is_empty());
    }
}
