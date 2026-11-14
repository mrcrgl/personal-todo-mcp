//! Cell values. `Raw` is a value as a caller wrote it, before it is checked
//! against a column; `Value` is one that passed.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use chrono::{DateTime, FixedOffset, NaiveDate, SecondsFormat};

use crate::domain::table::schema::ColumnType;

const MAX_TEXT_CHARS: usize = 10_000;
const MAX_LIST_ITEMS: usize = 200;
const MAX_LIST_ITEM_CHARS: usize = 1_000;

/// A value in the shape JSON gives it. Boundaries map their wire format to
/// this; the domain never sees the wire format itself.
#[derive(Debug, Clone, PartialEq)]
pub enum Raw {
    Null,
    Bool(bool),
    Integer(i64),
    Number(f64),
    Text(String),
    List(Vec<Raw>),
    /// An object, which no column type accepts.
    Object,
}

/// Column name (unchecked) → value, as given for a row.
pub type RawRecord = BTreeMap<String, Raw>;

/// A checked value. Enum values are `Text`.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Text(String),
    Integer(i64),
    Number(f64),
    Boolean(bool),
    Date(NaiveDate),
    DateTime(DateTime<FixedOffset>),
    List(Vec<String>),
}

impl Value {
    /// Values of different kinds never compare; the schema keeps them apart.
    pub fn compare(&self, other: &Self) -> Option<Ordering> {
        match (self, other) {
            (Self::Text(a), Self::Text(b)) => Some(a.cmp(b)),
            (Self::Integer(a), Self::Integer(b)) => Some(a.cmp(b)),
            (Self::Number(a), Self::Number(b)) => Some(a.total_cmp(b)),
            (Self::Boolean(a), Self::Boolean(b)) => Some(a.cmp(b)),
            (Self::Date(a), Self::Date(b)) => Some(a.cmp(b)),
            (Self::DateTime(a), Self::DateTime(b)) => Some(a.cmp(b)),
            (Self::List(a), Self::List(b)) => Some(a.cmp(b)),
            _ => None,
        }
    }

    /// The canonical text of dates and timestamps, for boundaries to write.
    pub fn date_text(date: &NaiveDate) -> String {
        date.format("%Y-%m-%d").to_string()
    }

    pub fn datetime_text(value: &DateTime<FixedOffset>) -> String {
        value.to_rfc3339_opts(SecondsFormat::AutoSi, true)
    }
}

/// Why a value does not fit its column. Static, so messages never echo it.
pub type Mismatch = &'static str;

impl ColumnType {
    /// `None` for `Raw::Null`: the cell is empty.
    pub fn coerce(&self, raw: &Raw) -> Result<Option<Value>, Mismatch> {
        if *raw == Raw::Null {
            return Ok(None);
        }
        let value = match (self, raw) {
            (Self::String, Raw::Text(text)) => Value::Text(checked_text(text)?),
            (Self::String, _) => return Err("expected a string"),
            (Self::Integer, Raw::Integer(n)) => Value::Integer(*n),
            (Self::Integer, Raw::Number(n)) if n.fract() == 0.0 && n.abs() < 9.0e15 => {
                Value::Integer(*n as i64)
            }
            (Self::Integer, _) => return Err("expected an integer"),
            (Self::Number, Raw::Integer(n)) => Value::Number(*n as f64),
            (Self::Number, Raw::Number(n)) if n.is_finite() => Value::Number(*n),
            (Self::Number, _) => return Err("expected a number"),
            (Self::Boolean, Raw::Bool(b)) => Value::Boolean(*b),
            (Self::Boolean, _) => return Err("expected true or false"),
            (Self::Date, Raw::Text(text)) => Value::Date(
                NaiveDate::parse_from_str(text.trim(), "%Y-%m-%d")
                    .map_err(|_| "expected a date as YYYY-MM-DD")?,
            ),
            (Self::Date, _) => return Err("expected a date as YYYY-MM-DD"),
            (Self::DateTime, Raw::Text(text)) => Value::DateTime(
                DateTime::parse_from_rfc3339(text.trim())
                    .map_err(|_| "expected an RFC 3339 timestamp, e.g. 2026-11-14T09:30:00Z")?,
            ),
            (Self::DateTime, _) => {
                return Err("expected an RFC 3339 timestamp, e.g. 2026-11-14T09:30:00Z");
            }
            (Self::Enum(values), Raw::Text(text)) => {
                if !values.contains(text) {
                    return Err("not one of the column's values");
                }
                Value::Text(text.clone())
            }
            (Self::Enum(_), _) => return Err("expected one of the column's values"),
            (Self::List, Raw::List(items)) => Value::List(checked_list(items)?),
            (Self::List, _) => return Err("expected a list of strings"),
        };
        Ok(Some(value))
    }
}

fn checked_text(text: &str) -> Result<String, Mismatch> {
    if text.chars().count() > MAX_TEXT_CHARS {
        return Err("text is longer than 10000 characters");
    }
    if text.contains('\0') {
        return Err("text must not contain NUL characters");
    }
    Ok(text.to_string())
}

fn checked_list(items: &[Raw]) -> Result<Vec<String>, Mismatch> {
    if items.len() > MAX_LIST_ITEMS {
        return Err("a list holds at most 200 items");
    }
    items
        .iter()
        .map(|item| match item {
            Raw::Text(text) if text.chars().count() <= MAX_LIST_ITEM_CHARS => checked_text(text),
            Raw::Text(_) => Err("list items are at most 1000 characters"),
            _ => Err("expected a list of strings"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: &str) -> Raw {
        Raw::Text(value.into())
    }

    #[test]
    fn values_are_checked_against_their_column_type() {
        assert_eq!(
            ColumnType::Integer.coerce(&Raw::Number(3.0)),
            Ok(Some(Value::Integer(3)))
        );
        assert!(ColumnType::Integer.coerce(&Raw::Number(3.5)).is_err());
        assert!(ColumnType::Integer.coerce(&text("3")).is_err());
        assert_eq!(
            ColumnType::Number.coerce(&Raw::Integer(2)),
            Ok(Some(Value::Number(2.0)))
        );
        assert!(ColumnType::Boolean.coerce(&text("true")).is_err());
        assert!(ColumnType::String.coerce(&Raw::Object).is_err());
        assert_eq!(ColumnType::String.coerce(&Raw::Null), Ok(None));
    }

    #[test]
    fn dates_and_timestamps_are_parsed() {
        let date = ColumnType::Date
            .coerce(&text("2026-11-14"))
            .unwrap()
            .unwrap();
        assert_eq!(
            date,
            Value::Date(NaiveDate::from_ymd_opt(2026, 11, 14).unwrap())
        );
        assert!(ColumnType::Date.coerce(&text("14.11.2026")).is_err());

        let Some(Value::DateTime(at)) = ColumnType::DateTime
            .coerce(&text("2026-11-14T09:30:00+01:00"))
            .unwrap()
        else {
            panic!("not a timestamp");
        };
        assert_eq!(Value::datetime_text(&at), "2026-11-14T09:30:00+01:00");
    }

    #[test]
    fn enums_accept_only_their_values() {
        let status = ColumnType::Enum(vec!["todo".into(), "done".into()]);
        assert_eq!(
            status.coerce(&text("done")),
            Ok(Some(Value::Text("done".into())))
        );
        assert!(status.coerce(&text("Done")).is_err());
    }

    #[test]
    fn lists_hold_strings() {
        assert_eq!(
            ColumnType::List.coerce(&Raw::List(vec![text("a"), text("b")])),
            Ok(Some(Value::List(vec!["a".into(), "b".into()])))
        );
        assert!(
            ColumnType::List
                .coerce(&Raw::List(vec![Raw::Integer(1)]))
                .is_err()
        );
        assert!(
            ColumnType::List
                .coerce(&Raw::List(vec![text("a"); 201]))
                .is_err()
        );
    }

    #[test]
    fn long_text_is_refused_without_echoing_it() {
        let err = ColumnType::String
            .coerce(&text(&"x".repeat(10_001)))
            .unwrap_err();
        assert!(!err.contains("xxx"));
    }

    #[test]
    fn values_compare_only_with_their_own_kind() {
        assert_eq!(
            Value::Integer(1).compare(&Value::Integer(2)),
            Some(Ordering::Less)
        );
        assert_eq!(Value::Integer(1).compare(&Value::Number(1.0)), None);
    }
}
