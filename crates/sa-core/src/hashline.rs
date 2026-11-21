//! hashline editing: addressing a line by a hash of its own content (spec 9).
//!
//! a model that edits by line number edits a line it has not seen the moment anything above
//! it shifts, and a model that edits by quoting text gets the quoting slightly wrong. a hash
//! is neither: it names the text itself, so a line that has moved is still found, and a line
//! that has changed is refused instead of being overwritten with a guess.
//!
//! the format is the one a model can copy back verbatim: `hhhhhh|content`, six hex
//! characters of the line's sha256. everything here is fault tolerant on purpose, because
//! getting this wrong is the common case rather than the exception:
//!
//! - a hash that no line carries is refused, naming the hash, rather than editing nothing.
//! - a hash that several identical lines carry is refused, naming the lines, rather than
//!   picking one.
//! - a line number that disagrees with the hash loses to the hash, which is the one that
//!   describes the text.
//! - text copied back with its `hhhhhh|` prefix is accepted, and the prefix stripped.

use std::fmt::Write as _;

use sha2::{Digest, Sha256};

/// the separator between a line's hash and its text.
const SEPARATOR: char = '|';

/// how many hex characters a line's hash carries. six puts the first collision at roughly
/// five thousand identical-by-hash lines in one file, and a line is only ever matched
/// within its own file.
const HASH_CHARS: usize = 6;

/// the hash of one line's text, as the model copies it back.
#[must_use]
pub fn hash_line(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    let mut out = String::with_capacity(HASH_CHARS);
    for byte in digest {
        if out.len() == HASH_CHARS {
            break;
        }
        let _ = write!(out, "{byte:02x}");
    }
    out.truncate(HASH_CHARS);
    out
}

/// a file as the model reads it: every line prefixed with its own hash.
#[must_use]
pub fn render(source: &str) -> String {
    let trailing = source.ends_with('\n');
    let lines: Vec<&str> = source.lines().collect();
    let mut out = String::with_capacity(source.len() + lines.len() * (HASH_CHARS + 1));
    for (index, line) in lines.iter().enumerate() {
        out.push_str(&hash_line(line));
        out.push(SEPARATOR);
        out.push_str(line);
        if index + 1 < lines.len() || trailing {
            out.push('\n');
        }
    }
    out
}

/// the text of a line, with its `hhhhhh|` prefix removed when it has one.
///
/// this is what makes a model that copies a hashed line into `old` still succeed: the hash
/// is addressing, not content.
#[must_use]
pub fn strip_hashed(text: &str) -> String {
    let trailing = text.ends_with('\n');
    let lines: Vec<&str> = text.lines().collect();
    let mut out = String::with_capacity(text.len());
    for (index, line) in lines.iter().enumerate() {
        out.push_str(content_of(line).unwrap_or(line));
        if index + 1 < lines.len() || trailing {
            out.push('\n');
        }
    }
    out
}

/// the text after a `hhhhhh|` prefix, when the line carries one.
fn content_of(line: &str) -> Option<&str> {
    let (prefix, rest) = line.split_at_checked(HASH_CHARS)?;
    if !prefix.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    rest.strip_prefix(SEPARATOR)
}

/// the lines a hash matched, as the error names them.
fn line_list(matches: &[usize]) -> String {
    matches
        .iter()
        .map(|index| (index + 1).to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// one line to replace, named by the hash of the text it must currently hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub hash: String,
    /// the line the model thinks it is on. a hint only: the hash decides.
    pub line: Option<usize>,
    pub content: String,
}

/// replaces the lines the edits name, or refuses and says why.
///
/// # Errors
///
/// Returns a message naming what went wrong and what to do about it: the model reads it and
/// tries again, so it says which hash, which lines, and that a re-read is the fix.
pub fn apply(source: &str, edits: &[Edit]) -> Result<String, String> {
    if edits.is_empty() {
        return Err("no edits: nothing to replace".to_owned());
    }
    let trailing = source.ends_with('\n');
    let lines: Vec<&str> = source.lines().collect();

    let mut replacements: Vec<(usize, &str)> = Vec::new();
    for edit in edits {
        let matches: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, text)| hash_line(text) == edit.hash)
            .map(|(index, _)| index)
            .collect();
        let target = match edit.line {
            // the hash describes the text, so it wins over the number: a number goes stale
            // the moment anything above it shifts, which is the case this exists for.
            Some(hint) => {
                let index = hint.saturating_sub(1);
                if lines
                    .get(index)
                    .is_some_and(|text| hash_line(text) == edit.hash)
                {
                    index
                } else if matches.len() == 1 {
                    matches[0]
                } else if matches.is_empty() {
                    return Err(format!(
                        "line {hint} does not carry hash {} — re-read the file",
                        edit.hash
                    ));
                } else {
                    return Err(format!(
                        "line {hint} does not carry hash {}, which is on lines {} — re-read the file",
                        edit.hash,
                        line_list(&matches)
                    ));
                }
            }
            None => match matches.as_slice() {
                [only] => *only,
                [] => {
                    return Err(format!(
                        "no line carries hash {} — re-read the file",
                        edit.hash
                    ))
                }
                many => {
                    return Err(format!(
                        "hash {} is on lines {}; pass line to choose — re-read the file",
                        edit.hash,
                        line_list(many)
                    ));
                }
            },
        };
        if replacements.iter().any(|(at, _)| *at == target) {
            return Err(format!("line {} is edited twice", target + 1));
        }
        replacements.push((target, edit.content.as_str()));
    }

    let mut out = String::with_capacity(source.len());
    for (index, text) in lines.iter().enumerate() {
        match replacements.iter().find(|(at, _)| *at == index) {
            Some((_, replacement)) => out.push_str(replacement),
            None => out.push_str(text),
        }
        if index + 1 < lines.len() || trailing {
            out.push('\n');
        }
    }
    Ok(out)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    const SOURCE: &str = "fn one() {}\nfn two() {}\nfn three() {}\n";

    #[test]
    fn a_rendered_file_names_every_line_by_its_own_text() {
        let rendered = render(SOURCE);
        let lines: Vec<&str> = rendered.lines().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(
            lines[0],
            format!("{}|fn one() {{}}", hash_line("fn one() {}"))
        );
        assert!(rendered.ends_with('\n'), "the file's own ending is kept");
    }

    #[test]
    fn an_edit_replaces_the_line_its_hash_names() {
        let hash = hash_line("fn two() {}");
        let edited = apply(
            SOURCE,
            &[Edit {
                hash,
                line: None,
                content: "fn two() { run(); }".to_owned(),
            }],
        )
        .expect("edit");
        assert_eq!(edited, "fn one() {}\nfn two() { run(); }\nfn three() {}\n");
    }

    #[test]
    fn a_line_that_moved_is_still_found() {
        // the hash describes the text, so inserting above it does not lose it.
        let moved = "// a comment\nfn one() {}\nfn two() {}\nfn three() {}\n";
        let edited = apply(
            moved,
            &[Edit {
                hash: hash_line("fn two() {}"),
                line: Some(2),
                content: "fn two() { run(); }".to_owned(),
            }],
        )
        .expect("the hash wins over the stale line number");
        assert!(edited.contains("fn two() { run(); }"), "{edited}");
    }

    #[test]
    fn a_hash_that_no_line_carries_is_refused() {
        let error = apply(
            SOURCE,
            &[Edit {
                hash: "abcdef".to_owned(),
                line: None,
                content: "x".to_owned(),
            }],
        )
        .expect_err("refused");
        assert!(error.contains("abcdef"), "{error}");
        assert!(error.contains("re-read"), "it says what to do: {error}");
    }

    #[test]
    fn a_hash_several_lines_carry_is_refused_rather_than_guessed() {
        let source = "same\nsame\n";
        let error = apply(
            source,
            &[Edit {
                hash: hash_line("same"),
                line: None,
                content: "other".to_owned(),
            }],
        )
        .expect_err("refused");
        assert!(error.contains("lines 1, 2"), "{error}");
        assert!(error.contains("pass line"), "{error}");

        // and the line hint resolves it.
        let edited = apply(
            source,
            &[Edit {
                hash: hash_line("same"),
                line: Some(2),
                content: "other".to_owned(),
            }],
        )
        .expect("edit");
        assert_eq!(edited, "same\nother\n");
    }

    #[test]
    fn an_ambiguous_hash_with_a_stale_line_is_refused() {
        // two lines carry the hash, and the hint points at neither: only a re-read can say
        // which one was meant.
        let source = "same\nother\nsame\n";
        let error = apply(
            source,
            &[Edit {
                hash: hash_line("same"),
                line: Some(2),
                content: "x".to_owned(),
            }],
        )
        .expect_err("refused");
        assert!(error.contains("line 2"), "{error}");
        assert!(error.contains("lines 1, 3"), "{error}");
        assert!(error.contains("re-read"), "{error}");
    }

    #[test]
    fn a_line_number_that_no_longer_holds_the_hash_is_not_an_error() {
        // the number went stale, the text did not: the hash is what describes the text.
        let error = apply(
            SOURCE,
            &[Edit {
                hash: hash_line("fn three() {}"),
                line: Some(1),
                content: "x".to_owned(),
            }],
        )
        .expect("the hash wins over the stale number");
        assert!(error.contains('x'), "{error}");
    }

    #[test]
    fn hashed_text_copied_back_into_old_still_matches() {
        let rendered = render(SOURCE);
        assert_eq!(strip_hashed(&rendered), SOURCE, "the prefixes come off");
        // text that was never hashed is left alone.
        assert_eq!(strip_hashed("plain text"), "plain text");
        // a `|` in the content is not a separator unless six hex characters precede it.
        assert_eq!(strip_hashed("a | b"), "a | b");
    }

    #[test]
    fn two_edits_on_one_line_are_refused() {
        let error = apply(
            SOURCE,
            &[
                Edit {
                    hash: hash_line("fn two() {}"),
                    line: None,
                    content: "a".to_owned(),
                },
                Edit {
                    hash: hash_line("fn two() {}"),
                    line: None,
                    content: "b".to_owned(),
                },
            ],
        )
        .expect_err("refused");
        assert!(error.contains("twice"), "{error}");
    }

    #[test]
    fn a_file_without_a_trailing_newline_keeps_not_having_one() {
        let source = "one\ntwo";
        let edited = apply(
            source,
            &[Edit {
                hash: hash_line("two"),
                line: None,
                content: "three".to_owned(),
            }],
        )
        .expect("edit");
        assert_eq!(edited, "one\nthree");
    }
}
