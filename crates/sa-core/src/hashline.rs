//! hashline editing: addressing a line by a hash of its own content (spec 9).
//!
//! a model that edits by line number edits a line it has not seen the moment anything above
//! it shifts, and a model that edits by quoting text gets the quoting slightly wrong. a hash
//! is neither: it names the text itself, so a line that has moved is still found, and a line
//! that has changed is refused instead of being overwritten with a guess.
//!
//! the format is the one a model can copy back verbatim: `hhhhhh|content`, six hex
//! characters of the line's sha256.
//!
//! everything here is fault tolerant on purpose, because getting this wrong is the common
//! case rather than the exception. a model that gets it wrong is told what the file holds
//! now and what to send instead, rather than being left to work it out:
//!
//! - a hash no line carries is refused, and the lines around the one it meant are returned
//!   hashed, so the retry needs no separate read.
//! - a hash several identical lines carry is refused, naming each of them.
//! - a line number that disagrees with the hash loses to the hash, which describes the text.
//! - the text itself is accepted where a hash belongs, for a model that copied the wrong
//!   half of the line.
//! - a successful edit returns the hashes of what it wrote, so the next edit has them.

use std::fmt::Write as _;

use sha2::{Digest, Sha256};

/// the separator between a line's hash and its text.
const SEPARATOR: char = '|';

/// how many hex characters a line's hash carries. six puts the first collision at roughly
/// five thousand identical-by-hash lines in one file, and a line is only ever matched
/// within its own file.
const HASH_CHARS: usize = 6;

/// how much of a file a refusal shows around the line the model meant.
const WINDOW: usize = 4;

/// a file short enough to hand back whole when there is nothing to aim at. past this, the
/// model is better off re-reading than being shown the top of a long file.
const WHOLE_FILE: usize = 40;

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

/// the lines around `line`, hashed, under a header naming the range.
///
/// this is what a refusal hands back: enough of the file to retry against, without the cost
/// of reading it again.
#[must_use]
pub fn window(source: &str, line: usize, radius: usize) -> String {
    let lines: Vec<&str> = source.lines().collect();
    if lines.is_empty() {
        return String::new();
    }
    let centre = line.saturating_sub(1).min(lines.len() - 1);
    let start = centre.saturating_sub(radius);
    let end = (centre + radius + 1).min(lines.len());
    let mut out = String::new();
    let _ = writeln!(out, "@@ lines {}-{} @@", start + 1, end);
    for (index, text) in lines.iter().enumerate().take(end).skip(start) {
        let _ = writeln!(out, "{}", hashed(index + 1, text));
    }
    out
}

/// one line as the model reads it, numbered by the caller when it needs the number.
fn hashed(_number: usize, text: &str) -> String {
    format!("{}{SEPARATOR}{text}", hash_line(text))
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

/// one line to replace, named by the hash of the text it must currently hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub hash: String,
    /// the line the model thinks it is on. a hint only: the hash decides.
    pub line: Option<usize>,
    pub content: String,
}

/// what an edit wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    pub source: String,
    /// the lines it replaced, 1-based, in the file as it was written.
    pub changed: Vec<usize>,
}

/// why an edit was refused, and where to look.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub message: String,
    /// the line the model was aiming at, when it said, so the caller can show it what is
    /// there now.
    pub around: Option<usize>,
}

impl Failure {
    const fn at(message: String, around: Option<usize>) -> Self {
        Self { message, around }
    }
}

/// whether this line is the one an edit names.
///
/// the hash is the address, but a model that copied the text instead of the hash is not
/// wrong about which line it means, so the text is accepted where a hash belongs.
fn names(text: &str, name: &str) -> bool {
    let looks_like_hash =
        name.len() == HASH_CHARS && name.bytes().all(|byte| byte.is_ascii_hexdigit());
    if looks_like_hash {
        hash_line(text) == name
    } else {
        text.trim_end() == name.trim_end()
    }
}

/// replaces the lines the edits name, or refuses and says why.
///
/// # Errors
///
/// Returns a [`Failure`] carrying what went wrong and the line it was aiming at. the caller
/// turns that into the file's current lines, so the retry needs no separate read.
pub fn apply(source: &str, edits: &[Edit]) -> Result<Applied, Failure> {
    if edits.is_empty() {
        return Err(Failure::at("no edits: nothing to replace".to_owned(), None));
    }
    let trailing = source.ends_with('\n');
    let lines: Vec<&str> = source.lines().collect();

    let mut replacements: Vec<(usize, &str)> = Vec::new();
    for edit in edits {
        let matches: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, text)| names(text, &edit.hash))
            .map(|(index, _)| index)
            .collect();
        let target = match edit.line {
            // the hash describes the text, so it wins over the number: a number goes stale
            // the moment anything above it shifts, which is the case this exists for.
            Some(hint) => {
                let index = hint.saturating_sub(1);
                if lines.get(index).is_some_and(|text| names(text, &edit.hash)) {
                    index
                } else if matches.len() == 1 {
                    matches[0]
                } else if matches.is_empty() {
                    return Err(Failure::at(
                        format!(
                            "line {hint} does not carry hash {}, and no line does — the file \
                             has changed since it was read",
                            edit.hash
                        ),
                        Some(hint),
                    ));
                } else {
                    return Err(Failure::at(
                        format!(
                            "line {hint} does not carry hash {}, which is on lines {} — add \
                             `line` to say which",
                            edit.hash,
                            line_list(&matches)
                        ),
                        Some(hint),
                    ));
                }
            }
            None => match matches.as_slice() {
                [only] => *only,
                [] => {
                    return Err(Failure::at(
                        format!(
                            "no line carries hash {} — the file has changed since it was read",
                            edit.hash
                        ),
                        None,
                    ))
                }
                many => {
                    return Err(Failure::at(
                        format!(
                            "hash {} is on lines {}; add `line` to say which",
                            edit.hash,
                            line_list(many)
                        ),
                        Some(many[0] + 1),
                    ));
                }
            },
        };
        if replacements.iter().any(|(at, _)| *at == target) {
            return Err(Failure::at(
                format!("line {} is edited twice", target + 1),
                Some(target + 1),
            ));
        }
        replacements.push((target, edit.content.as_str()));
    }

    let mut changed: Vec<usize> = replacements.iter().map(|(at, _)| at + 1).collect();
    changed.sort_unstable();

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
    Ok(Applied {
        source: out,
        changed,
    })
}

/// the guidance a refusal hands back: what the file holds around the line that was meant.
#[must_use]
pub fn guidance(source: &str, failure: &Failure) -> String {
    let shown = match failure.around {
        Some(around) => window(source, around, WINDOW),
        // with no line to aim at, a short file is its own guidance; a long one is not.
        None if source.lines().count() <= WHOLE_FILE => render(source),
        None => String::new(),
    };
    if shown.is_empty() {
        return format!(
            "{}\n\nre-read the file, then edit by the hashes it returns",
            failure.message
        );
    }
    format!(
        "{}\n\nwhat the file holds now — send one of these hashes:\n{shown}",
        failure.message
    )
}

/// the lines a hash matched, as a refusal names them.
fn line_list(matches: &[usize]) -> String {
    matches
        .iter()
        .map(|index| (index + 1).to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    const SOURCE: &str = "fn one() {}\nfn two() {}\nfn three() {}\n";

    fn edit(hash: &str, content: &str) -> Edit {
        Edit {
            hash: hash.to_owned(),
            line: None,
            content: content.to_owned(),
        }
    }

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
        let applied = apply(
            SOURCE,
            &[edit(&hash_line("fn two() {}"), "fn two() { run(); }")],
        )
        .expect("edit");
        assert_eq!(
            applied.source,
            "fn one() {}\nfn two() { run(); }\nfn three() {}\n"
        );
        assert_eq!(applied.changed, [2], "it says which line it wrote");
    }

    #[test]
    fn a_line_that_moved_is_still_found() {
        // the hash describes the text, so inserting above it does not lose it.
        let moved = "// a comment\nfn one() {}\nfn two() {}\nfn three() {}\n";
        let applied = apply(
            moved,
            &[Edit {
                hash: hash_line("fn two() {}"),
                line: Some(2),
                content: "fn two() { run(); }".to_owned(),
            }],
        )
        .expect("the hash wins over the stale line number");
        assert!(
            applied.source.contains("fn two() { run(); }"),
            "{applied:?}"
        );
    }

    #[test]
    fn the_text_itself_is_accepted_where_a_hash_belongs() {
        // a model that copied the wrong half of the line is not wrong about the line.
        let applied = apply(SOURCE, &[edit("fn two() {}", "fn two() { run(); }")])
            .expect("matched by its text");
        assert!(
            applied.source.contains("fn two() { run(); }"),
            "{applied:?}"
        );
    }

    #[test]
    fn a_refusal_hands_back_the_lines_to_retry_with() {
        let failure = apply(SOURCE, &[edit("abcdef", "x")]).expect_err("refused");
        assert!(failure.message.contains("abcdef"), "{failure:?}");

        let guidance = guidance(SOURCE, &failure);
        assert!(guidance.contains("send one of these hashes"), "{guidance}");
        assert!(
            guidance.contains(&format!("{}|fn one() {{}}", hash_line("fn one() {}"))),
            "the current hashes are returned: {guidance}"
        );
    }

    #[test]
    fn a_refusal_around_a_line_shows_the_lines_near_it() {
        let mut stale = edit("abcdef", "x");
        stale.line = Some(2);
        let failure = apply(SOURCE, &[stale]).expect_err("refused");
        assert_eq!(failure.around, Some(2), "it knows where the model aimed");

        let guidance = guidance(SOURCE, &failure);
        assert!(guidance.contains("lines 1-3"), "{guidance}");
    }

    #[test]
    fn a_hash_several_lines_carry_is_refused_rather_than_guessed() {
        let source = "same\nsame\n";
        let failure = apply(source, &[edit(&hash_line("same"), "other")]).expect_err("refused");
        assert!(failure.message.contains("lines 1, 2"), "{failure:?}");
        assert!(failure.message.contains("add `line`"), "{failure:?}");

        // and the line hint resolves it.
        let applied = apply(
            source,
            &[Edit {
                hash: hash_line("same"),
                line: Some(2),
                content: "other".to_owned(),
            }],
        )
        .expect("edit");
        assert_eq!(applied.source, "same\nother\n");
    }

    #[test]
    fn an_ambiguous_hash_with_a_stale_line_is_refused() {
        // two lines carry the hash, and the hint points at neither: only a re-read can say
        // which one was meant.
        let source = "same\nother\nsame\n";
        let failure = apply(
            source,
            &[Edit {
                hash: hash_line("same"),
                line: Some(2),
                content: "x".to_owned(),
            }],
        )
        .expect_err("refused");
        assert!(failure.message.contains("line 2"), "{failure:?}");
        assert!(failure.message.contains("lines 1, 3"), "{failure:?}");
    }

    #[test]
    fn a_line_number_that_no_longer_holds_the_hash_is_not_an_error() {
        // the number went stale, the text did not: the hash is what describes the text.
        let applied = apply(
            SOURCE,
            &[Edit {
                hash: hash_line("fn three() {}"),
                line: Some(1),
                content: "x".to_owned(),
            }],
        )
        .expect("the hash wins over the stale number");
        assert!(applied.source.contains('x'), "{applied:?}");
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
        let failure = apply(
            SOURCE,
            &[
                edit(&hash_line("fn two() {}"), "a"),
                edit(&hash_line("fn two() {}"), "b"),
            ],
        )
        .expect_err("refused");
        assert!(failure.message.contains("twice"), "{failure:?}");
    }

    #[test]
    fn a_file_without_a_trailing_newline_keeps_not_having_one() {
        let applied = apply("one\ntwo", &[edit(&hash_line("two"), "three")]).expect("edit");
        assert_eq!(applied.source, "one\nthree");
    }
}
