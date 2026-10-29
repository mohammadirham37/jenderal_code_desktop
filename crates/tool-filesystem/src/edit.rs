//! String-replacement edits that tolerate the small mismatches language
//! models routinely make (line endings, trailing whitespace, indentation,
//! typographic quotes) while never guessing between several candidates.
//!
//! Matching runs in tiers; the first tier that finds a match decides. Every
//! relaxed tier requires a *unique* match, so a fuzzy edit can't land in the
//! wrong place.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

/// How `old_string` was located in the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchStrategy {
    Exact,
    /// Ignoring trailing whitespace on each line.
    TrailingWhitespace,
    /// Ignoring leading indentation; `new_string` is re-indented to match.
    Indentation,
    /// Treating typographic quotes / non-breaking spaces as ASCII.
    Normalized,
}

impl MatchStrategy {
    pub fn describe(self) -> &'static str {
        match self {
            MatchStrategy::Exact => "exact match",
            MatchStrategy::TrailingWhitespace => "matched ignoring trailing whitespace",
            MatchStrategy::Indentation => "matched ignoring indentation (new text re-indented)",
            MatchStrategy::Normalized => "matched after normalizing quotes/whitespace",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PlannedEdit {
    pub after: String,
    pub occurrences: usize,
    pub strategy: MatchStrategy,
}

/// Compute the result of replacing `old` with `new` in `content`.
/// `path` is only used in error messages.
pub fn plan_edit(content: &str, old: &str, new: &str, replace_all: bool, path: &str) -> Result<PlannedEdit> {
    if old.is_empty() {
        bail!("old_string must not be empty (use fs_write to create or overwrite a file)");
    }
    if old == new {
        bail!("old_string and new_string are identical");
    }

    // Work on LF text when the file is consistently CRLF, then convert back,
    // so a model that emits "\n" still matches a Windows-style file.
    let crlf = content.contains("\r\n") && content.matches("\r\n").count() == content.matches('\n').count();
    let (text, old, new) = if crlf {
        (content.replace("\r\n", "\n"), old.replace("\r\n", "\n"), new.replace("\r\n", "\n"))
    } else {
        (content.to_string(), old.to_string(), new.to_string())
    };
    let restore = |s: String| if crlf { s.replace('\n', "\r\n") } else { s };

    // Tier 1: exact.
    let occurrences = text.matches(old.as_str()).count();
    if occurrences == 1 || (occurrences > 1 && replace_all) {
        let after = if replace_all { text.replace(old.as_str(), &new) } else { text.replacen(old.as_str(), &new, 1) };
        return Ok(PlannedEdit { after: restore(after), occurrences, strategy: MatchStrategy::Exact });
    }
    if occurrences > 1 {
        bail!(
            "old_string occurs {occurrences} times in {path}. Include more surrounding lines to make it unique, or set replace_all=true."
        );
    }

    // Tiers 2-4: line-based relaxed matching.
    let lines = split_lines(&text);
    let old_lines = pattern_lines(&old);
    if !old_lines.is_empty() && old_lines.iter().any(|l| !l.trim().is_empty()) {
        let tiers: [(MatchStrategy, LineEq); 3] = [
            (MatchStrategy::TrailingWhitespace, |a, b| a.trim_end() == b.trim_end()),
            (MatchStrategy::Indentation, |a, b| a.trim() == b.trim()),
            (MatchStrategy::Normalized, |a, b| normalize(a) == normalize(b)),
        ];
        for (strategy, eq) in tiers {
            let starts = find_windows(&lines, &old_lines, eq);
            match starts.len() {
                0 => continue,
                1 => {
                    let start = starts[0];
                    let replacement = if strategy == MatchStrategy::Indentation {
                        reindent(&new, &old_lines, &lines[start..start + old_lines.len()])
                    } else {
                        new.clone()
                    };
                    let after = splice(&lines, start, old_lines.len(), old.ends_with('\n'), &replacement);
                    return Ok(PlannedEdit { after: restore(after), occurrences: 1, strategy });
                }
                n => bail!(
                    "old_string does not match {path} exactly, and {} it matches {n} places. Re-read the file with fs_read and copy old_string exactly, with enough surrounding lines to be unique.",
                    strategy.describe().replace("matched ", "")
                ),
            }
        }
    }

    bail!("{}", not_found_message(&lines, &old_lines, path))
}

/// Line comparison used by one matching tier.
type LineEq = fn(&str, &str) -> bool;

/// Lines of `s`, each including its trailing '\n' (if any).
fn split_lines(s: &str) -> Vec<&str> {
    s.split_inclusive('\n').collect()
}

/// Lines of the search pattern without newlines; a trailing newline does not
/// produce an extra empty line.
fn pattern_lines(s: &str) -> Vec<&str> {
    let body = s.strip_suffix('\n').unwrap_or(s);
    body.split('\n').collect()
}

fn strip_nl(l: &str) -> &str {
    l.strip_suffix('\n').unwrap_or(l)
}

fn find_windows(lines: &[&str], pat: &[&str], eq: LineEq) -> Vec<usize> {
    if pat.len() > lines.len() {
        return Vec::new();
    }
    (0..=lines.len() - pat.len())
        .filter(|&i| pat.iter().enumerate().all(|(j, p)| eq(strip_nl(lines[i + j]), p)))
        .collect()
}

/// Replace `count` lines starting at `start` with `replacement`. When the
/// pattern didn't end with a newline, the last matched line's newline is
/// kept (the pattern described the line content only).
fn splice(lines: &[&str], start: usize, count: usize, pattern_had_nl: bool, replacement: &str) -> String {
    let mut out = String::new();
    for l in &lines[..start] {
        out.push_str(l);
    }
    out.push_str(replacement);
    let last = lines[start + count - 1];
    if !pattern_had_nl && last.ends_with('\n') && !replacement.ends_with('\n') {
        out.push('\n');
    }
    for l in &lines[start + count..] {
        out.push_str(l);
    }
    out
}

fn leading_ws(s: &str) -> &str {
    &s[..s.len() - s.trim_start().len()]
}

/// Shift `new`'s indentation by the difference between the pattern's first
/// non-blank line and the file line it matched.
fn reindent(new: &str, pat: &[&str], matched: &[&str]) -> String {
    let Some(idx) = pat.iter().position(|l| !l.trim().is_empty()) else {
        return new.to_string();
    };
    let from = leading_ws(pat[idx]);
    let to = leading_ws(strip_nl(matched[idx]));
    if from == to {
        return new.to_string();
    }
    new.split_inclusive('\n')
        .map(|line| {
            if line.trim().is_empty() {
                line.to_string()
            } else if let Some(rest) = line.strip_prefix(from) {
                format!("{to}{rest}")
            } else if from.is_empty() {
                format!("{to}{line}")
            } else {
                line.to_string()
            }
        })
        .collect()
}

fn normalize(s: &str) -> String {
    s.trim()
        .chars()
        .map(|c| match c {
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' | '\u{2032}' => '\'',
            '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' | '\u{2033}' => '"',
            '\u{2013}' | '\u{2014}' | '\u{2212}' => '-',
            '\u{00A0}' | '\u{2007}' | '\u{202F}' | '\t' => ' ',
            c => c,
        })
        .collect::<String>()
        .split(' ')
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Error text that helps the model retry: the most similar region of the
/// file with line numbers, plus the instruction to re-read.
fn not_found_message(lines: &[&str], pat: &[&str], path: &str) -> String {
    let mut msg = format!(
        "old_string not found in {path}. The file does not contain that text (it may have changed, or old_string was written from memory). Call fs_read on {path} and copy old_string exactly from the current content."
    );
    let want: Vec<String> = pat.iter().map(|l| normalize(l)).filter(|l| !l.is_empty()).collect();
    if want.is_empty() || lines.is_empty() {
        return msg;
    }
    let window = pat.len().max(1).min(lines.len());
    let mut best = (0usize, 0usize);
    for i in 0..=lines.len() - window {
        let score = lines[i..i + window]
            .iter()
            .filter(|l| {
                let n = normalize(strip_nl(l));
                !n.is_empty() && want.iter().any(|w| *w == n || (w.len() > 8 && n.contains(w.as_str())))
            })
            .count();
        if score > best.1 {
            best = (i, score);
        }
    }
    if best.1 == 0 {
        msg.push_str(&format!(" ({path} has {} lines; none resemble old_string.)", lines.len()));
        return msg;
    }
    let from = best.0.saturating_sub(2);
    let to = (best.0 + window + 2).min(lines.len()).min(from + 40);
    msg.push_str(&format!("\nMost similar region (lines {}-{}):\n", from + 1, to));
    for (n, l) in lines[from..to].iter().enumerate() {
        msg.push_str(&format!("{:>5} | {}\n", from + n + 1, strip_nl(l)));
    }
    msg
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(content: &str, old: &str, new: &str) -> Result<PlannedEdit> {
        plan_edit(content, old, new, false, "f.php")
    }

    const PHP: &str = "<?php\n\nreturn [\n    App\\Providers\\AppServiceProvider::class,\n];\n";

    #[test]
    fn exact_match() {
        let e = apply(PHP, "AppServiceProvider::class,", "AppServiceProvider::class,\n    X::class,").unwrap();
        assert_eq!(e.strategy, MatchStrategy::Exact);
        assert!(e.after.contains("    X::class,\n];"));
    }

    #[test]
    fn tolerates_wrong_indentation_and_reindents() {
        // Model used tabs / no indentation.
        let old = "return [\nApp\\Providers\\AppServiceProvider::class,\n];";
        let new = "return [\nApp\\Providers\\AppServiceProvider::class,\nApp\\Providers\\Filament\\AdminPanelProvider::class,\n];";
        let e = apply(PHP, old, new).unwrap();
        assert_eq!(e.strategy, MatchStrategy::Indentation);
        assert_eq!(
            e.after,
            "<?php\n\nreturn [\nApp\\Providers\\AppServiceProvider::class,\nApp\\Providers\\Filament\\AdminPanelProvider::class,\n];\n"
        );
        // Indented pattern vs. differently indented file: new text follows the file.
        let file = "fn a() {\n        let x = 1;\n        let y = 2;\n}\n";
        let e = apply(file, "    let x = 1;\n    let y = 2;", "    let x = 10;\n    let y = 2;").unwrap();
        assert_eq!(e.after, "fn a() {\n        let x = 10;\n        let y = 2;\n}\n");
    }

    #[test]
    fn tolerates_trailing_whitespace() {
        let file = "a = 1;   \nb = 2;\n";
        let e = apply(file, "a = 1;\nb = 2;", "a = 3;\nb = 2;").unwrap();
        assert_eq!(e.strategy, MatchStrategy::TrailingWhitespace);
        assert_eq!(e.after, "a = 3;\nb = 2;\n");
    }

    #[test]
    fn handles_crlf_files() {
        let file = "line1\r\nline2\r\nline3\r\n";
        let e = apply(file, "line1\nline2", "line1\nLINE2").unwrap();
        assert_eq!(e.strategy, MatchStrategy::Exact);
        assert_eq!(e.after, "line1\r\nLINE2\r\nline3\r\n");
    }

    #[test]
    fn normalizes_smart_quotes() {
        let file = "echo 'hello';\n";
        let e = apply(file, "echo \u{2018}hello\u{2019};", "echo 'bye';").unwrap();
        assert_eq!(e.strategy, MatchStrategy::Normalized);
        assert_eq!(e.after, "echo 'bye';\n");
    }

    #[test]
    fn relaxed_match_must_be_unique() {
        // No exact match, but two lines match once spacing is normalized.
        let file = "  x  =  1;\n    x  =  1;\n";
        let err = apply(file, "x = 1;", "x = 2;").unwrap_err().to_string();
        assert!(err.contains("2 places"), "{err}");
    }

    #[test]
    fn exact_duplicates_need_replace_all() {
        let err = apply("a\na\n", "a", "b").unwrap_err().to_string();
        assert!(err.contains("occurs 2 times"), "{err}");
        let e = plan_edit("a\na\n", "a", "b", true, "f").unwrap();
        assert_eq!((e.after.as_str(), e.occurrences), ("b\nb\n", 2));
    }

    #[test]
    fn not_found_shows_closest_region() {
        let err = apply(PHP, "return [\n    App\\Providers\\RouteServiceProvider::class,\n];", "x")
            .unwrap_err()
            .to_string();
        assert!(err.contains("Call fs_read"), "{err}");
        assert!(err.contains("Most similar region"), "{err}");
        assert!(err.contains("    3 | return ["), "{err}");
    }

    #[test]
    fn rejects_empty_old_string() {
        assert!(apply(PHP, "", "x").is_err());
    }

    #[test]
    fn keeps_trailing_newline_semantics() {
        // Pattern without trailing newline replaces line content only.
        let e = apply("a\nb\nc\n", "  b", "B").unwrap();
        assert_eq!(e.after, "a\nB\nc\n");
        // Pattern with trailing newline can delete the whole line.
        let e = apply("a\n  b\nc\n", "  b  \n", "").unwrap();
        assert_eq!(e.after, "a\nc\n");
    }
}
