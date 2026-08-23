//! Mapping between spec-tree node text and test identifiers in source code.
//!
//! Packs configure how a condition like "when the key is present" becomes a
//! block name (`when_the_key_is_present`, `describe("when the key is present")`)
//! and how an action like "it returns none" becomes a test name.

use serde::Deserialize;

/// Case transform applied to node text when deriving an identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Case {
    /// Keep the text as-is (trimmed). Used by block-style runners (vitest).
    #[default]
    Verbatim,
    /// `snake_case`, punctuation dropped.
    Snake,
    /// `camelCase`, punctuation dropped.
    Camel,
    /// `PascalCase`, punctuation dropped.
    Pascal,
}

/// How one category of spec node maps to an identifier.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct NameRule {
    /// Prefix to strip from the node text before transforming, e.g. "it ".
    #[serde(default)]
    pub strip_prefix: Option<String>,
    /// Prefix to prepend to the transformed name, e.g. "test_".
    #[serde(default)]
    pub add_prefix: Option<String>,
    /// Case transform for the remaining text.
    #[serde(default)]
    pub case: Case,
}

impl NameRule {
    /// Apply this rule to spec node text, producing the expected identifier.
    #[must_use]
    pub fn apply(&self, text: &str) -> String {
        let mut t = text.trim();
        if let Some(p) = &self.strip_prefix
            && let Some(stripped) = strip_prefix_ci(t, p)
        {
            t = stripped;
        }
        let out = match self.case {
            Case::Verbatim => t.trim().to_string(),
            Case::Snake => words(t).collect::<Vec<_>>().join("_").to_lowercase(),
            Case::Camel => {
                let mut s = String::new();
                for (i, w) in words(t).enumerate() {
                    if i == 0 {
                        s.push_str(&w.to_lowercase());
                    } else {
                        s.push_str(&capitalize(w));
                    }
                }
                s
            }
            Case::Pascal => words(t).map(capitalize).collect(),
        };
        match &self.add_prefix {
            Some(p) => format!("{p}{out}"),
            None => out,
        }
    }
}

/// Case-insensitive `strip_prefix` (ASCII case folding, boundary-safe).
fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let n = prefix.len();
    if s.len() >= n && s.is_char_boundary(n) && s[..n].eq_ignore_ascii_case(prefix) {
        Some(&s[n..])
    } else {
        None
    }
}

/// Split node text into identifier-safe words, dropping punctuation.
fn words(t: &str) -> impl Iterator<Item = &str> {
    t.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty())
}

fn capitalize(w: &str) -> String {
    let mut cs = w.chars();
    match cs.next() {
        Some(c) => c.to_uppercase().collect::<String>() + &cs.as_str().to_lowercase(),
        None => String::new(),
    }
}

/// How the spec root line maps onto the test file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RootMapping {
    /// The root is the file itself; top-level spec nodes map to top-level
    /// blocks/tests in the file (Rust integration tests).
    #[default]
    File,
    /// The root must appear as a top-level block (a `describe` in vitest).
    Block,
}

/// A pack's complete node-text-to-identifier configuration.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Mapping {
    /// How the root line maps onto the file.
    #[serde(default)]
    pub root: RootMapping,
    /// Rule for condition nodes -> block names.
    #[serde(default)]
    pub block: NameRule,
    /// Rule for action nodes -> test names.
    #[serde(default)]
    pub test: NameRule,
    /// Block names that are structurally transparent: their children are
    /// treated as if they sat at the wrapper's level. Lets Rust's
    /// `#[cfg(test)] mod tests { ... }` wrapper exist without appearing in
    /// the spec tree.
    #[serde(default)]
    pub wrappers: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    mod when_applying_snake_case {
        use super::*;

        #[test]
        fn joins_words_with_underscores() {
            let rule = NameRule { case: Case::Snake, ..Default::default() };
            assert_eq!(rule.apply("when the key is present"), "when_the_key_is_present");
        }

        #[test]
        fn drops_punctuation() {
            let rule = NameRule { case: Case::Snake, ..Default::default() };
            assert_eq!(rule.apply("it returns `None`, always"), "it_returns_none_always");
        }
    }

    mod when_a_strip_prefix_is_configured {
        use super::*;

        #[test]
        fn removes_it_case_insensitively() {
            let rule = NameRule {
                strip_prefix: Some("it ".into()),
                case: Case::Snake,
                ..Default::default()
            };
            assert_eq!(rule.apply("It returns none"), "returns_none");
        }

        #[test]
        fn leaves_other_text_untouched() {
            let rule = NameRule {
                strip_prefix: Some("it ".into()),
                case: Case::Verbatim,
                ..Default::default()
            };
            assert_eq!(rule.apply("when iterating"), "when iterating");
        }

        #[test]
        fn survives_multibyte_text_at_the_boundary() {
            let rule = NameRule {
                strip_prefix: Some("it ".into()),
                case: Case::Snake,
                ..Default::default()
            };
            // "it—does" has a multi-byte char straddling the prefix length;
            // must not panic, and the prefix must not match.
            assert_eq!(rule.apply("it—does thing"), "it_does_thing");
        }
    }

    mod when_applying_pascal_case {
        use super::*;

        #[test]
        fn capitalizes_each_word() {
            let rule = NameRule {
                strip_prefix: Some("when ".into()),
                add_prefix: Some("test_When".into()),
                case: Case::Pascal,
            };
            assert_eq!(rule.apply("when the caller is the owner"), "test_WhenTheCallerIsTheOwner");
        }
    }
}
