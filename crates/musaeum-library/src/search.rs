//! Finds works as someone types.
//!
//! Each keystroke runs up to three queries, stopping as soon as there are enough results:
//!
//! 1. the title starts with what was typed;
//! 2. every word typed starts a word somewhere in the title, the authors or the series;
//! 3. every word typed appears inside one, which needs at least three letters per word.
//!
//! None of them asks SQLite to rank all the matches. Sorting is the one thing that scales with how
//! many works match, and that is what takes a first letter past any budget. Instead the tiers are
//! the ranking: an exact start comes before a word start, which comes before the middle of a word.

use std::collections::HashSet;

use rusqlite::{Connection, params};
use serde::Serialize;

use crate::error::Result;
use crate::model::WorkId;

/// How many results a search returns unless told otherwise.
pub const DEFAULT_SEARCH_LIMIT: usize = 50;

const MAX_TERMS: usize = 16;
const MAX_TERM_CHARS: usize = 128;
/// The trigram index can only look for something at least this long.
const MIN_SUBSTRING_CHARS: usize = 3;

/// How a work matched.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchKind {
    /// Its title starts with what was typed.
    TitleStart,
    /// Each word typed starts a word somewhere in it.
    WordStart,
    /// Each word typed appears somewhere in it.
    Substring,
}

/// One work found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SearchHit {
    /// Its ID.
    pub work_id: WorkId,
    /// Its title.
    pub title: String,
    /// Its authors, in order, separated by `; `.
    pub creators: Option<String>,
    /// Its series.
    pub series: Option<String>,
    /// How it matched.
    pub matched: MatchKind,
    /// Whether any of its files can be seen right now, here or as a placeholder.
    pub available: bool,
}

/// A term as an FTS5 string, which leaves nothing in it that could be read as query syntax.
fn quoted(term: &str) -> String {
    format!("\"{}\"", term.replace('"', "\"\""))
}

fn has_word_characters(term: &str) -> bool {
    term.chars().any(char::is_alphanumeric)
}

/// `title : ^"dom casm"*`: the title starts with these words, the last one possibly unfinished.
fn title_start_query(terms: &[&str]) -> Option<String> {
    let words: Vec<&str> = terms
        .iter()
        .copied()
        .filter(|t| has_word_characters(t))
        .collect();
    (!words.is_empty()).then(|| format!("title : ^{}*", quoted(&words.join(" "))))
}

/// `"dom"* "casm"*`: each word starts a word somewhere.
fn word_start_query(terms: &[&str]) -> Option<String> {
    let words: Vec<String> = terms
        .iter()
        .filter(|term| has_word_characters(term))
        .map(|term| format!("{}*", quoted(term)))
        .collect();
    (!words.is_empty()).then(|| words.join(" "))
}

/// `"dom" "asm"`: each term is somewhere, as a substring. A shorter term could not be looked up, and
/// leaving it out would return works without it, so then this tier does not run at all.
fn substring_query(terms: &[&str]) -> Option<String> {
    let long_enough = terms
        .iter()
        .all(|term| term.chars().count() >= MIN_SUBSTRING_CHARS);
    long_enough.then(|| {
        terms
            .iter()
            .map(|term| quoted(term))
            .collect::<Vec<_>>()
            .join(" ")
    })
}

/// Finds works matching what was typed.
pub(crate) fn search(connection: &Connection, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
    // A control character is not something to look for, and a NUL would end the query for SQLite.
    let visible: String = query
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect();
    let cut: Vec<String> = visible
        .split_whitespace()
        .take(MAX_TERMS)
        .map(|term| term.chars().take(MAX_TERM_CHARS).collect())
        .collect();
    let terms: Vec<&str> = cut.iter().map(String::as_str).collect();
    if terms.is_empty() || limit == 0 {
        return Ok(Vec::new());
    }

    let mut hits = Vec::with_capacity(limit);
    let mut seen = HashSet::with_capacity(limit);
    let tiers = [
        (
            "SELECT rowid FROM search_words WHERE search_words MATCH ?1 LIMIT ?2",
            title_start_query(&terms),
            MatchKind::TitleStart,
        ),
        (
            "SELECT rowid FROM search_words WHERE search_words MATCH ?1 LIMIT ?2",
            word_start_query(&terms),
            MatchKind::WordStart,
        ),
        (
            "SELECT rowid FROM search_trigrams WHERE search_trigrams MATCH ?1 LIMIT ?2",
            substring_query(&terms),
            MatchKind::Substring,
        ),
    ];
    for (sql, expression, matched) in tiers {
        let Some(expression) = expression else {
            continue;
        };
        let ids = {
            let mut statement = connection.prepare_cached(sql)?;
            statement
                .query_map(params![expression, limit as i64], |row| {
                    row.get::<_, WorkId>(0)
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut tier = Vec::new();
        for id in ids {
            if hits.len() + tier.len() == limit {
                break;
            }
            if seen.insert(id) {
                tier.push(load_hit(connection, id, matched)?);
            }
        }
        // Shortest first is the closest thing to the best match without looking at the rest.
        tier.sort_by(|a, b| {
            a.title
                .chars()
                .count()
                .cmp(&b.title.chars().count())
                .then_with(|| a.title.cmp(&b.title))
        });
        hits.append(&mut tier);
        if hits.len() == limit {
            break;
        }
    }
    Ok(hits)
}

fn load_hit(connection: &Connection, id: WorkId, matched: MatchKind) -> Result<SearchHit> {
    Ok(connection
        .prepare_cached(
            "SELECT w.title, w.series,
                 (SELECT group_concat(name, '; ') FROM (
                      SELECT c.name FROM work_creator wc JOIN creator c ON c.id = wc.creator_id
                      WHERE wc.work_id = w.id ORDER BY wc.position)),
                 EXISTS (SELECT 1 FROM document d
                         JOIN revision r ON r.document_id = d.id
                         JOIN file_instance i ON i.revision_id = r.id
                         WHERE d.work_id = w.id AND i.state <> 'absent')
             FROM work w WHERE w.id = ?1",
        )?
        .query_row([id], |row| {
            Ok(SearchHit {
                work_id: id,
                title: row.get(0)?,
                series: row.get(1)?,
                creators: row.get(2)?,
                matched,
                available: row.get(3)?,
            })
        })?)
}

/// Brings one work's entries up to date, for what the trigger on `work` does not see: its creators.
pub(crate) fn refresh_entry(transaction: &rusqlite::Transaction<'_>, work: WorkId) -> Result<()> {
    for table in ["search_words", "search_trigrams"] {
        transaction
            .prepare_cached(&format!("DELETE FROM {table} WHERE rowid = ?1"))?
            .execute([work])?;
        transaction
            .prepare_cached(&format!(
                "INSERT INTO {table} (rowid, title, creators, series)
                 SELECT w.id, w.title,
                     (SELECT coalesce(group_concat(c.name, ' '), '') FROM work_creator wc
                         JOIN creator c ON c.id = wc.creator_id WHERE wc.work_id = w.id),
                     coalesce(w.series, '')
                 FROM work w WHERE w.id = ?1"
            ))?
            .execute([work])?;
    }
    Ok(())
}

/// Writes the search indexes again from what is in `work`. They hold nothing that is not there,
/// which is what makes it possible to throw them away if they ever seem wrong.
pub(crate) fn rebuild(transaction: &rusqlite::Transaction<'_>) -> Result<()> {
    transaction.execute_batch(
        "DELETE FROM search_words;
         DELETE FROM search_trigrams;
         CREATE TEMP TABLE rebuilt AS
             SELECT w.id, w.title,
                 (SELECT coalesce(group_concat(c.name, ' '), '') FROM work_creator wc
                     JOIN creator c ON c.id = wc.creator_id WHERE wc.work_id = w.id) AS creators,
                 coalesce(w.series, '') AS series
             FROM work w;
         INSERT INTO search_words (rowid, title, creators, series)
             SELECT id, title, creators, series FROM rebuilt;
         INSERT INTO search_trigrams (rowid, title, creators, series)
             SELECT id, title, creators, series FROM rebuilt;
         DROP TABLE rebuilt;",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_syntax_stays_inert() {
        assert_eq!(quoted(r#"a"b"#), r#""a""b""#);
        assert_eq!(
            word_start_query(&["NEAR(x", "AND"]).unwrap(),
            r#""NEAR(x"* "AND"*"#
        );
    }

    #[test]
    fn terms_with_no_word_characters_do_not_reach_the_word_index() {
        assert_eq!(word_start_query(&["--", "***"]), None);
        assert_eq!(title_start_query(&["--"]), None);
        assert_eq!(word_start_query(&["--", "dom"]).unwrap(), r#""dom"*"#);
    }

    #[test]
    fn a_short_term_turns_the_substring_tier_off() {
        assert_eq!(substring_query(&["dom", "qu"]), None);
        assert_eq!(substring_query(&["dom", "qui"]).unwrap(), r#""dom" "qui""#);
        assert_eq!(substring_query(&["ção"]).unwrap(), r#""ção""#);
    }
}
