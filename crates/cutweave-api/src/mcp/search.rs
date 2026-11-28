// SPDX-License-Identifier: GPL-3.0-or-later
//! The search behind `cutweave_search_commands` (WP-1J.01; the words a user would say:
//! WP-2I.07).
//!
//! A query is split into words. Each word counts once per entry, by the best of:
//!
//! - **3:** the word is in the entry's name (`split` in `clip.split`), or is one of the entry's
//!   keywords in the core table (`mcp_core.toml`, `[keywords]`: `cut` for `clip.split`,
//!   `faster` for `clip.set_speed`). Keywords are what a person says when the command's name is
//!   an editor's term; no command is renamed for search.
//! - **2:** the word without a plural `s` is in the name.
//! - **1:** the word (or that stem) is in the summary, or, for a word of 4 letters or more that
//!   matches nothing else, its letters occur in the name in order (typos).
//!
//! Words that say nothing about a command ("the", "make", "please") are skipped when the query
//! has other words. A word of one or two letters must be a whole part of a name (`in` matches
//! `timeline.set_in_out`, not `timeline`). Ties are in name order, so results are the same from
//! run to run.

//!
//! # A name that is no command (#258)
//!
//! `cutweave_describe_command`, `cutweave_execute` and `cutweave_query` answer a name they do
//! not know with `not_found`; its `hint` names up to [`MAX_SUGGESTIONS`] commands close to the
//! guess ([`nearest`]), so `clip.speed` is answered with `clip.set_speed`. A candidate's
//! closeness is half the likeness of the two spellings and half the overlap of their parts,
//! where a part of the guess also matches a keyword of the command; a guess that shares only
//! the namespace is not close. Only commands an agent can call are named.

use cutweave_commands::{Entry, Scope};

use crate::mcp::{
    catalog::{self, Catalog},
    core::CoreTable,
};

/// Default number of hits returned.
pub const DEFAULT_LIMIT: usize = 20;
/// Most hits returned.
pub const MAX_LIMIT: usize = 50;
/// Most hits returned with [`Detail::Schema`]: input schemas are big.
pub const MAX_SCHEMA_HITS: usize = 5;
/// Most commands `cutweave_describe_command` describes in one call (`names`).
pub const MAX_DESCRIBED: usize = 8;

/// What `cutweave_search_commands` returns per hit (`detail`; WP-2I.08, ADR-0040 question 5).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Detail {
    /// The name only: `matches` is a list of strings.
    Names,
    /// Name, summary, kind and how to call it: the default, and the result before `detail`
    /// existed.
    #[default]
    Summary,
    /// The summary plus the hit's `input_schema`, for at most [`MAX_SCHEMA_HITS`] hits.
    Schema,
}

impl Detail {
    /// The values of `detail`, in listing order.
    pub const NAMES: [&'static str; 3] = ["names", "summary", "schema"];

    /// Parses a `detail` value.
    #[must_use]
    pub fn parse(value: &str) -> Option<Detail> {
        match value {
            "names" => Some(Detail::Names),
            "summary" => Some(Detail::Summary),
            "schema" => Some(Detail::Schema),
            _ => None,
        }
    }

    /// The hits a search returns at this level when `limit` are asked for.
    #[must_use]
    pub fn limit(self, limit: usize) -> usize {
        match self {
            Detail::Schema => limit.min(MAX_SCHEMA_HITS),
            Detail::Names | Detail::Summary => limit,
        }
    }
}

/// Words that carry no meaning for finding a command.
const STOP_WORDS: &[&str] = &[
    "a", "an", "and", "are", "at", "be", "can", "do", "for", "here", "how", "i", "is", "it",
    "make", "me", "my", "of", "on", "please", "so", "some", "that", "the", "them", "there",
    "these", "this", "those", "to", "want", "with", "you",
];

/// The result of a search.
#[derive(Debug)]
pub struct Found<'a> {
    /// The best hits, best first, at most the limit.
    pub entries: Vec<&'a Entry>,
    /// How many entries matched.
    pub total: usize,
    /// The query as searched: lower case.
    pub query: String,
}

/// Finds the entries reachable through `catalog` that match `query`; an empty query matches
/// all of them, in name order.
#[must_use]
pub fn find<'a>(catalog: &'a Catalog, query: &str, limit: usize) -> Found<'a> {
    let query = query.to_lowercase();
    let words = words(&query);
    let table = CoreTable::get();
    let mut hits: Vec<(usize, &Entry)> = catalog
        .reachable_entries()
        .filter_map(|e| {
            let score = score(
                e.meta.name,
                e.meta.summary,
                table.keywords_of(e.meta.name),
                &words,
            );
            (words.is_empty() || score > 0).then_some((score, e))
        })
        .collect();
    hits.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.meta.name.cmp(b.1.meta.name)));
    let total = hits.len();
    let entries = hits
        .into_iter()
        .take(limit.clamp(1, MAX_LIMIT))
        .map(|(_, e)| e)
        .collect();
    Found {
        entries,
        total,
        query,
    }
}

/// The words of a lower-case query that are searched for.
fn words(query: &str) -> Vec<&str> {
    let all: Vec<&str> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let meaningful: Vec<&str> = all
        .iter()
        .copied()
        .filter(|w| !STOP_WORDS.contains(w))
        .collect();
    if meaningful.is_empty() {
        all
    } else {
        meaningful
    }
}

fn in_name(name: &str, word: &str) -> bool {
    if word.len() <= 2 {
        name.split(['.', '_']).any(|part| part == word)
    } else {
        name.contains(word)
    }
}

/// Keyword relevance of one entry (see the module docs).
fn score(name: &str, summary: &str, keywords: &[String], words: &[&str]) -> usize {
    let summary = summary.to_lowercase();
    let mut total = 0;
    for word in words {
        let stem = word
            .strip_suffix('s')
            .filter(|s| s.len() >= 3)
            .unwrap_or(word);
        total += if in_name(name, word) || keywords.iter().any(|k| k == word || k == stem) {
            3
        } else if in_name(name, stem) {
            2
        } else if summary.contains(word) || summary.contains(stem) {
            1
        } else {
            usize::from(word.len() >= 4 && is_subsequence(word, name))
        };
    }
    total
}

fn is_subsequence(needle: &str, haystack: &str) -> bool {
    let mut chars = haystack.chars();
    needle.chars().all(|c| chars.any(|h| h == c))
}

/// Most names a `not_found` suggests (#258).
pub const MAX_SUGGESTIONS: usize = 3;
/// The lowest [`closeness`] a suggested name has.
const NEAR: f64 = 0.55;
/// The likeness of two spellings from which a guess is near without sharing a word.
const CLOSE_SPELLING: f64 = 0.8;
/// A suggestion is at most this far below the best one.
const NEAR_BEST: f64 = 0.15;
/// Longer guesses than this are not compared: no command is that long.
const MAX_COMPARED: usize = 96;

/// The names reachable through `catalog` that are closest to `unknown`, a name that is no
/// command: best first, at most [`MAX_SUGGESTIONS`], none when nothing is near. Only what an
/// agent can call is suggested: never a `hidden` entry, never a UI-only one.
#[must_use]
pub fn nearest<'a>(catalog: &'a Catalog, unknown: &str) -> Vec<&'a str> {
    let table = CoreTable::get();
    nearest_of(
        catalog
            .reachable_entries()
            .filter(|e| e.meta.scope == Scope::Any)
            .map(|e| (e.meta.name, table.keywords_of(e.meta.name))),
        unknown,
    )
}

/// The hint of a `not_found` for `unknown`, a name that is no command (#258): the closest
/// commands when some are near, or that the name is one of the generic tools. `None` when
/// there is nothing better to say than the error kind's own hint.
#[must_use]
pub fn unknown_command_hint(catalog: &Catalog, unknown: &str) -> Option<String> {
    let rest = "cutweave_search_commands lists every command and query";
    if let Some(tool) = catalog::GENERIC_TOOLS
        .iter()
        .find(|tool| unknown.trim().eq_ignore_ascii_case(tool))
    {
        return Some(format!(
            "`{tool}` is a tool, not a command: call it as a tool; {rest}"
        ));
    }
    let near: Vec<String> = nearest(catalog, unknown)
        .iter()
        .map(|name| format!("`{name}`"))
        .collect();
    let (last, first) = near.split_last()?;
    Some(if first.is_empty() {
        format!("Did you mean {last}? {rest}")
    } else {
        format!("Did you mean {} or {last}? {rest}", first.join(", "))
    })
}

/// [`nearest`] over `(name, keywords)` pairs.
fn nearest_of<'a>(
    names: impl Iterator<Item = (&'a str, &'a [String])>,
    unknown: &str,
) -> Vec<&'a str> {
    let unknown = command_form(unknown);
    if unknown.is_empty() || unknown.len() > MAX_COMPARED {
        return Vec::new();
    }
    let mut near: Vec<(f64, &str)> = names
        .map(|(name, keywords)| (closeness(&unknown, name, keywords), name))
        .filter(|(score, _)| *score >= NEAR)
        .collect();
    near.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(b.1)));
    let best = near.first().map_or(0.0, |(score, _)| *score);
    near.into_iter()
        .take_while(|(score, _)| *score >= best - NEAR_BEST)
        .take(MAX_SUGGESTIONS)
        .map(|(_, name)| name)
        .collect()
}

/// A guessed name in the form of a command name: lower case, and a tool-style name
/// (`clip_speed`) with its first `_` as the dot.
fn command_form(name: &str) -> String {
    let name = name.trim().to_lowercase();
    if name.contains('.') {
        name
    } else {
        name.replacen('_', ".", 1)
    }
}

fn ratio(part: usize, whole: usize) -> f64 {
    let small = |n: usize| u32::try_from(n).map_or(f64::MAX, f64::from);
    small(part) / small(whole.max(1))
}

/// How close the guess `unknown` is to the command `name`, 0 to 1: half the likeness of the
/// two spellings (1 minus the edit distance over the longer length), half the overlap of their
/// parts ([`part_overlap`]). A guess that shares no word of what the command does (the parts
/// after the dot, the keywords) is as close as its spelling when that is near
/// ([`CLOSE_SPELLING`]: a typo in a short word), else 0: the same namespace alone is not a
/// suggestion.
fn closeness(unknown: &str, name: &str, keywords: &[String]) -> f64 {
    let longer = unknown.chars().count().max(name.chars().count());
    let spelling = 1.0 - ratio(edit_distance(unknown, name), longer);
    match part_overlap(unknown, name, keywords) {
        Some(overlap) => 0.5 * spelling + 0.5 * overlap,
        None if spelling >= CLOSE_SPELLING => spelling,
        None => 0.0,
    }
}

/// Whether two parts of a name are the same word: equal, or one typo apart when both have at
/// least 4 letters.
fn same_part(a: &str, b: &str) -> bool {
    a == b || (a.len().min(b.len()) >= 4 && edit_distance(a, b) <= 1)
}

/// The parts of a command name with their weights: the namespace (before the dot) counts 1,
/// each word of what the command does (after it, or a name without a dot) counts 2.
fn weighted_parts(name: &str) -> Vec<(&str, usize)> {
    let (namespace, verb) = name.split_once('.').unwrap_or(("", name));
    let words = |text, weight| {
        str::split(text, ['.', '_'])
            .filter(|part| !part.is_empty())
            .map(move |part| (part, weight))
    };
    words(namespace, 1).chain(words(verb, 2)).collect()
}

/// The overlap of the parts of a guess and of a command name, 0 to 1: mostly how much of the
/// guess the command has (as a part of its name, as a keyword, or one typo away), and for the
/// rest how much of the name the guess has. `None` when the guess has none of the words of
/// what the command does.
fn part_overlap(unknown: &str, name: &str, keywords: &[String]) -> Option<f64> {
    let (guessed, parts) = (weighted_parts(unknown), weighted_parts(name));
    let total = |parts: &[(&str, usize)]| parts.iter().map(|(_, weight)| weight).sum::<usize>();
    let keyword = |g: &str| keywords.iter().any(|k| k == g);
    let found: usize = guessed
        .iter()
        .filter(|(g, _)| parts.iter().any(|(p, _)| same_part(g, p)) || keyword(g))
        .map(|(_, weight)| weight)
        .sum();
    let covered: usize = parts
        .iter()
        .filter(|(p, _)| guessed.iter().any(|(g, _)| same_part(g, p)))
        .map(|(_, weight)| weight)
        .sum();
    let does = parts
        .iter()
        .filter(|(_, weight)| *weight == 2)
        .any(|(p, _)| guessed.iter().any(|(g, _)| same_part(g, p)));
    (does || guessed.iter().any(|(g, _)| keyword(g)))
        .then(|| 0.7 * ratio(found, total(&guessed)) + 0.3 * ratio(covered, total(&parts)))
}

/// The Levenshtein distance of two strings, in characters.
fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let substituted = diagonal + usize::from(ca != *cb);
            diagonal = row[j + 1];
            row[j + 1] = substituted.min(diagonal + 1).min(row[j] + 1);
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_score_above_summaries_and_plurals_and_typos_are_tolerated() {
        assert_eq!(score("clip.split", "Split a clip", &[], &["split"]), 3);
        assert_eq!(
            score("track.add", "Add an empty track", &[], &["tracks"]),
            2
        );
        assert_eq!(
            score("edit.undo", "Undo your last change", &[], &["change"]),
            1
        );
        assert_eq!(score("timeline.insert", "Insert", &[], &["tmlnins"]), 1);
        assert_eq!(score("clip.split", "Split", &[], &["zebra"]), 0);
    }

    #[test]
    fn a_keyword_counts_like_the_name_and_once_per_word() {
        let keywords = ["cut".to_owned(), "photo".to_owned()];
        assert_eq!(score("clip.split", "Split a clip", &keywords, &["cut"]), 3);
        assert_eq!(
            score("clip.split", "Split a clip", &keywords, &["photos"]),
            3
        );
        // `split` is in the name and could be a keyword too: still 3.
        let both = ["split".to_owned()];
        assert_eq!(score("clip.split", "Split a clip", &both, &["split"]), 3);
        // A keyword is a whole word, not a part of one.
        assert_eq!(score("clip.split", "Split a clip", &keywords, &["cuts"]), 3);
        assert_eq!(score("clip.split", "x", &keywords, &["cu"]), 0);
    }

    fn near(unknown: &str) -> Vec<&'static str> {
        const NAMES: [&str; 12] = [
            "clip.reset_speed",
            "clip.set_speed",
            "clip.split",
            "clip.trim",
            "edit.undo",
            "item.delete",
            "item.move",
            "item.set",
            "timeline.add_marker",
            "timeline.get",
            "timeline.insert",
            "track.add",
        ];
        let faster: &'static [String] = Vec::leak(vec!["faster".to_owned()]);
        nearest_of(
            NAMES.iter().map(|name| {
                let words = if *name == "clip.set_speed" {
                    faster
                } else {
                    &[]
                };
                (*name, words)
            }),
            unknown,
        )
    }

    #[test]
    fn an_unknown_name_finds_the_closest_commands() {
        // The first evaluation's guess (#258), and the same guess as a tool name.
        assert_eq!(near("clip.speed")[0], "clip.set_speed");
        assert_eq!(near("clip_speed")[0], "clip.set_speed");
        assert_eq!(near("Clip.Set_Speed "), ["clip.set_speed"]);
        // Typos, a missing part, a keyword in place of the name's word.
        assert_eq!(near("clip.splt"), ["clip.split"]);
        assert_eq!(near("timeline.insrt"), ["timeline.insert"]);
        assert_eq!(near("timeline.marker")[0], "timeline.add_marker");
        assert_eq!(near("clip.faster")[0], "clip.set_speed");
        // At most three, best first.
        assert!(near("item.sett").len() <= MAX_SUGGESTIONS);
        assert_eq!(near("item.sett")[0], "item.set");
        // Nothing near: no suggestion.
        // The namespace alone says nothing about which of its commands is meant.
        assert_eq!(near("clip.move"), ["item.move"]);
        for far in [
            "",
            "zebra.frobnicate",
            "x",
            "clip",
            "timeline.zoom",
            "foo_bar_baz",
        ] {
            assert_eq!(near(far), [""; 0], "{far}");
        }
        assert_eq!(near(&"clip.".repeat(40)), [""; 0], "too long to compare");
    }

    #[test]
    fn edit_distance_counts_characters() {
        assert_eq!(edit_distance("", "abc"), 3);
        assert_eq!(edit_distance("clip.speed", "clip.set_speed"), 4);
        assert_eq!(edit_distance("kitten", "sitting"), 3);
        assert_eq!(edit_distance("\u{e9}", "e"), 1);
        assert_eq!(command_form("item_set_props"), "item.set_props");
        assert_eq!(command_form("a.b_c"), "a.b_c");
    }

    #[test]
    fn detail_levels_parse_and_only_schema_caps_the_hits() {
        for name in Detail::NAMES {
            assert!(Detail::parse(name).is_some(), "{name}");
        }
        assert_eq!(Detail::parse("full"), None);
        assert_eq!(Detail::default(), Detail::Summary);
        assert_eq!(Detail::Schema.limit(DEFAULT_LIMIT), MAX_SCHEMA_HITS);
        assert_eq!(Detail::Schema.limit(2), 2);
        assert_eq!(Detail::Names.limit(MAX_LIMIT), MAX_LIMIT);
        assert_eq!(Detail::Summary.limit(DEFAULT_LIMIT), DEFAULT_LIMIT);
    }

    #[test]
    fn filler_words_are_skipped_and_short_words_match_whole_name_parts() {
        assert_eq!(words("make the clip faster, please"), ["clip", "faster"]);
        assert_eq!(words("the"), ["the"], "a query of filler words is kept");
        assert_eq!(words(""), [""; 0]);
        assert!(in_name("timeline.set_in_out", "in"));
        assert!(!in_name("timeline.get", "in"));
        assert!(in_name("timeline.get", "line"));
    }
}
