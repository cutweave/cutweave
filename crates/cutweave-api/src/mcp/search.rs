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

use cutweave_commands::Entry;

use crate::mcp::{catalog::Catalog, core::CoreTable};

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
