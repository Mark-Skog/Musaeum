//! Smart collections: a rule, kept as JSON, and the works that match it whenever it is asked. A
//! rule names fields and operators from a closed list, checked here before it is kept; it is
//! turned into SQL by this module alone, its values always passed as parameters, so a rule can
//! never be SQL.
//!
//! ```json
//! {"match": "all", "rules": [
//!   {"field": "creator", "op": "contains", "value": "gould"},
//!   {"field": "year", "op": "between", "value": [1980, 1999]},
//!   {"field": "category", "op": "in", "value": 4},
//!   {"field": "custom:2", "op": "is", "value": "emprestado"}
//! ]}
//! ```
//!
//! Text is compared folded (`fold`: lower case, without accents), so `"gould"` finds "Gould" and
//! `"historia"` finds "História". A category takes the works of its subcategories too, and a
//! folder those of the folders below it. A rule with nothing in it matches no work.

use std::collections::HashMap;

use rusqlite::types::Value;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::sql::{date_ms, fold};

/// The most rules one smart collection has.
pub const MAX_RULES: usize = 32;

/// The kind of each custom field, by id, as the library has them.
pub type CustomKinds = HashMap<i64, &'static str>;

/// Whether every rule must match, or any.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Match {
    #[default]
    All,
    Any,
}

/// One condition on a work.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    pub field: String,
    pub op: String,
    #[serde(default)]
    pub value: serde_json::Value,
}

/// A smart collection's rule.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rules {
    #[serde(rename = "match", default)]
    pub matching: Match,
    #[serde(default)]
    pub rules: Vec<Rule>,
}

/// What kind of value a field holds, which says which operators it takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Text,
    Number,
    /// Milliseconds since the Unix epoch.
    Moment,
    /// `YYYY-MM-DD`, a custom field's.
    Day,
    Status,
    Format,
    Set,
    Boolean,
}

fn bad(rule: &Rule, why: &str) -> Error {
    Error::Refused(format!(
        "a smart collection's rule ({} {}): {why}",
        rule.field, rule.op
    ))
}

/// Where a field's values are: on the work's own row (`w`), or in rows of other tables, which a
/// rule takes when any of them matches.
struct Source {
    kind: Kind,
    /// `FROM … WHERE …` naming `w`, for a field with several values; none for the work's own.
    rows: Option<String>,
    /// The value, in `w` or in those rows.
    value: String,
}

fn source_of(rule: &Rule, custom: &CustomKinds) -> Result<Source> {
    let own = |kind: Kind, column: &str| Source {
        kind,
        rows: None,
        value: format!("w.{column}"),
    };
    let rows = |kind: Kind, rows: &str, value: &str| Source {
        kind,
        rows: Some(rows.to_owned()),
        value: value.to_owned(),
    };
    Ok(match rule.field.as_str() {
        "title" => own(Kind::Text, "title"),
        "series" => own(Kind::Text, "series"),
        "language" => own(Kind::Text, "language"),
        "year" => own(Kind::Number, "year"),
        "rating" => own(Kind::Number, "rating"),
        "status" => own(Kind::Status, "status"),
        "added" => own(Kind::Moment, "created_at"),
        "creator" => rows(
            Kind::Text,
            "work_creator x JOIN creator c ON c.id = x.creator_id WHERE x.work_id = w.id",
            "c.name",
        ),
        "identifier" => rows(
            Kind::Text,
            "work_identifier x WHERE x.work_id = w.id",
            "x.value",
        ),
        "format" => rows(
            Kind::Format,
            "document d WHERE d.work_id = w.id",
            "d.format",
        ),
        "progress" => rows(
            Kind::Number,
            "document d JOIN reading_state s ON s.document_id = d.id WHERE d.work_id = w.id",
            "s.progress * 100",
        ),
        "last_read" => rows(
            Kind::Moment,
            "document d JOIN reading_state s ON s.document_id = d.id WHERE d.work_id = w.id",
            "s.updated_at",
        ),
        "tag" | "category" | "collection" | "folder" => Source {
            kind: Kind::Set,
            rows: None,
            value: String::new(),
        },
        other => {
            let id: i64 = other
                .strip_prefix("custom:")
                .and_then(|id| id.parse().ok())
                .ok_or_else(|| bad(rule, "no such field"))?;
            let kind = match custom.get(&id).copied() {
                Some("text" | "url" | "choice") => Kind::Text,
                Some("number") => Kind::Number,
                Some("date") => Kind::Day,
                Some("boolean") => Kind::Boolean,
                _ => return Err(bad(rule, "no such custom field")),
            };
            Source {
                kind,
                // The id is a number parsed above, not text from the rule.
                rows: Some(format!(
                    "work_custom_value x WHERE x.work_id = w.id AND x.field_id = {id}"
                )),
                value: "x.value".into(),
            }
        }
    })
}

fn text_value(rule: &Rule) -> Result<String> {
    rule.value
        .as_str()
        .map(str::to_owned)
        .filter(|text| !text.trim().is_empty() && text.chars().count() <= 400)
        .ok_or_else(|| bad(rule, "the value is a text of 1 to 400 characters"))
}

fn number_of(rule: &Rule, value: &serde_json::Value) -> Result<f64> {
    value
        .as_f64()
        .filter(|number| number.is_finite())
        .ok_or_else(|| bad(rule, "the value is a number"))
}

fn id_value(rule: &Rule) -> Result<i64> {
    rule.value
        .as_i64()
        .ok_or_else(|| bad(rule, "the value is an id"))
}

/// One rule as SQL on the work `w`, and its parameters in order.
fn compile_rule(rule: &Rule, custom: &CustomKinds, now_ms: i64) -> Result<(String, Vec<Value>)> {
    let source = source_of(rule, custom)?;
    let v = source.value.as_str();
    // Whether the work has a value for which `condition` (naming the value as `{v}`) holds.
    let some = |condition: &str| -> String {
        let condition = condition.replace("{v}", v);
        match &source.rows {
            None => format!("coalesce({condition}, 0)"),
            Some(rows) => format!("EXISTS (SELECT 1 FROM {rows} AND {condition})"),
        }
    };
    let op = rule.op.as_str();
    let mut params: Vec<Value> = Vec::new();
    let filled = "{v} IS NOT NULL AND {v} <> ''";
    let sql = match (source.kind, op) {
        (kind, "empty") if kind != Kind::Set => format!("NOT {}", some(filled)),
        (kind, "not_empty") if kind != Kind::Set => some(filled),
        (Kind::Text, "contains" | "not_contains" | "is" | "is_not" | "starts_with") => {
            params.push(Value::Text(fold(&text_value(rule)?)));
            let condition = match op {
                "contains" | "not_contains" => "instr(fold({v}), ?) > 0",
                "starts_with" => "instr(fold({v}), ?) = 1",
                _ => "fold({v}) = ?",
            };
            if matches!(op, "not_contains" | "is_not") {
                format!("NOT {}", some(condition))
            } else {
                some(condition)
            }
        }
        (Kind::Number, "eq" | "ne" | "lt" | "le" | "gt" | "ge") => {
            params.push(Value::Real(number_of(rule, &rule.value)?));
            some(match op {
                "eq" => "{v} = ?",
                "ne" => "{v} <> ?",
                "lt" => "{v} < ?",
                "le" => "{v} <= ?",
                "gt" => "{v} > ?",
                _ => "{v} >= ?",
            })
        }
        (Kind::Number, "between") => {
            let pair = rule
                .value
                .as_array()
                .filter(|pair| pair.len() == 2)
                .ok_or_else(|| bad(rule, "the value is two numbers"))?;
            let (low, high) = (number_of(rule, &pair[0])?, number_of(rule, &pair[1])?);
            params.push(Value::Real(low.min(high)));
            params.push(Value::Real(low.max(high)));
            some("{v} BETWEEN ? AND ?")
        }
        (Kind::Moment | Kind::Day, "in_last_days") => {
            let days = number_of(rule, &rule.value)?;
            if !(0.0..=36_500.0).contains(&days) {
                return Err(bad(rule, "the value is a number of days, up to 36500"));
            }
            let since = now_ms - (days * 86_400_000.0) as i64;
            if source.kind == Kind::Day {
                params.push(Value::Text(day_text(since)));
            } else {
                params.push(Value::Integer(since));
            }
            some("{v} >= ?")
        }
        (Kind::Moment | Kind::Day, "before" | "after") => {
            let text = rule
                .value
                .as_str()
                .filter(|text| date_ms(text).is_some())
                .ok_or_else(|| bad(rule, "the value is a date, YYYY-MM-DD"))?;
            if source.kind == Kind::Day {
                params.push(Value::Text(text.to_owned()));
                some(if op == "before" { "{v} < ?" } else { "{v} > ?" })
            } else {
                let day = date_ms(text).unwrap_or_default();
                // After a day is from the day after it on.
                params.push(Value::Integer(if op == "before" {
                    day
                } else {
                    day + 86_400_000
                }));
                some(if op == "before" {
                    "{v} < ?"
                } else {
                    "{v} >= ?"
                })
            }
        }
        (Kind::Status, "is" | "is_not") => {
            let status = text_value(rule)?;
            if !matches!(status.as_str(), "unread" | "reading" | "read" | "abandoned") {
                return Err(bad(rule, "the value is unread, reading, read or abandoned"));
            }
            params.push(Value::Text(status));
            some(if op == "is" { "{v} = ?" } else { "{v} <> ?" })
        }
        (Kind::Format, "is" | "is_not") => {
            params.push(Value::Text(text_value(rule)?.to_lowercase()));
            let condition = some("{v} = ?");
            if op == "is" {
                condition
            } else {
                format!("NOT {condition}")
            }
        }
        (Kind::Boolean, "is_true" | "is_false") => {
            params.push(Value::Integer(i64::from(op == "is_true")));
            some("{v} = ?")
        }
        (Kind::Set, "in" | "not_in") => {
            params.push(Value::Integer(id_value(rule)?));
            let inside = match rule.field.as_str() {
                "tag" => {
                    "EXISTS (SELECT 1 FROM work_tag t WHERE t.work_id = w.id AND t.tag_id = ?)"
                }
                "collection" => {
                    "EXISTS (SELECT 1 FROM collection_item i
                     WHERE i.work_id = w.id AND i.collection_id = ?)"
                }
                "category" => {
                    "EXISTS (SELECT 1 FROM category top JOIN category c
                         ON substr(c.path, 1, length(top.path)) = top.path
                     JOIN work_category wc ON wc.category_id = c.id
                     WHERE top.id = ? AND wc.work_id = w.id)"
                }
                _ => {
                    "EXISTS (WITH RECURSIVE scope(id) AS (
                         SELECT ? UNION ALL
                         SELECT f.id FROM folder f JOIN scope ON f.parent_id = scope.id)
                     SELECT 1 FROM document d JOIN revision r ON r.document_id = d.id
                     JOIN file_instance i ON i.revision_id = r.id
                     WHERE d.work_id = w.id AND i.state <> 'absent'
                         AND i.folder_id IN (SELECT id FROM scope))"
                }
            };
            if op == "in" {
                inside.to_owned()
            } else {
                format!("NOT {inside}")
            }
        }
        _ => return Err(bad(rule, "this operator does not go with this field")),
    };
    Ok((sql, params))
}

/// A moment as the day it falls on, `YYYY-MM-DD`, in UTC.
fn day_text(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000);
    // Civil date from days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// The rule, checked, as the JSON it is kept as.
pub fn check(rules: &Rules, custom: &CustomKinds) -> Result<String> {
    compile(rules, custom, 0)?;
    serde_json::to_string(rules).map_err(|error| Error::Refused(error.to_string()))
}

/// The rule as one SQL condition on the work `w`, and its parameters in order.
pub fn compile(rules: &Rules, custom: &CustomKinds, now_ms: i64) -> Result<(String, Vec<Value>)> {
    if rules.rules.len() > MAX_RULES {
        return Err(Error::Refused(format!(
            "a smart collection has {MAX_RULES} rules at most"
        )));
    }
    if rules.rules.is_empty() {
        return Ok(("0".into(), Vec::new()));
    }
    let mut parts = Vec::with_capacity(rules.rules.len());
    let mut params = Vec::new();
    for rule in &rules.rules {
        let (sql, values) = compile_rule(rule, custom, now_ms)?;
        parts.push(format!("({sql})"));
        params.extend(values);
    }
    let joint = match rules.matching {
        Match::All => " AND ",
        Match::Any => " OR ",
    };
    Ok((parts.join(joint), params))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(field: &str, op: &str, value: serde_json::Value) -> Rules {
        Rules {
            matching: Match::All,
            rules: vec![Rule {
                field: field.to_owned(),
                op: op.to_owned(),
                value,
            }],
        }
    }

    #[test]
    fn a_rule_is_sql_only_through_its_parameters() {
        let custom = CustomKinds::from([(2, "choice")]);
        let injected = "x'); DROP TABLE work; --";
        let (sql, params) = compile(
            &rule("title", "contains", serde_json::json!(injected)),
            &custom,
            0,
        )
        .unwrap();
        assert!(!sql.contains("DROP"), "{sql}");
        assert_eq!(params, [Value::Text(fold(injected))]);
        assert!(
            compile(
                &rule("custom:2", "is", serde_json::json!("emprestado")),
                &custom,
                0
            )
            .is_ok()
        );
        // Fields and operators come from the list, whatever is asked.
        for (field, op) in [
            ("title; DROP TABLE work", "contains"),
            ("custom:9", "is"),
            ("custom:x", "is"),
            ("year", "contains"),
            ("tag", "contains"),
        ] {
            assert!(
                compile(&rule(field, op, serde_json::json!("1")), &custom, 0).is_err(),
                "{field} {op}"
            );
        }
        assert!(
            compile(
                &rule("year", "between", serde_json::json!([1990])),
                &custom,
                0
            )
            .is_err()
        );
        assert!(
            compile(
                &rule("added", "after", serde_json::json!("2024-02-30")),
                &custom,
                0
            )
            .is_err()
        );
        assert!(compile(&rule("status", "is", serde_json::json!("lost")), &custom, 0).is_err());
    }

    #[test]
    fn no_rule_matches_nothing() {
        assert_eq!(
            compile(&Rules::default(), &CustomKinds::new(), 0)
                .unwrap()
                .0,
            "0"
        );
    }

    #[test]
    fn days_are_written_back_as_dates() {
        assert_eq!(day_text(0), "1970-01-01");
        assert_eq!(day_text(1_709_164_800_000), "2024-02-29");
        assert_eq!(day_text(951_868_800_000 - 1), "2000-02-29");
    }
}
