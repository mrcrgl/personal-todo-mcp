//! Table and column names. A name that passes cannot carry path meaning, so a
//! table name can become a file name without further checks.

use crate::domain::todo::ValidationError;

const MAX_NAME_CHARS: usize = 64;
/// Assigned by the server and present on every row.
pub const ID_FIELD: &str = "id";

/// A table, e.g. `reading-list`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct TableName(String);

impl TableName {
    pub fn parse(value: &str) -> Result<Self, ValidationError> {
        let name = value.trim();
        let mut chars = name.chars();
        let valid_start = chars
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
        let valid_rest =
            chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
        if !valid_start || !valid_rest || name.len() > MAX_NAME_CHARS {
            return Err(ValidationError(format!(
                "table names are 1 to {MAX_NAME_CHARS} characters of a-z, 0-9, _ and -, \
                 starting with a letter or digit, e.g. reading-list"
            )));
        }
        Ok(Self(name.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A column, e.g. `due_date`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ColumnName(String);

impl ColumnName {
    pub fn parse(value: &str) -> Result<Self, ValidationError> {
        let name = value.trim();
        let mut chars = name.chars();
        let valid_start = chars.next().is_some_and(|c| c.is_ascii_lowercase());
        let valid_rest = chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
        if !valid_start || !valid_rest || name.len() > MAX_NAME_CHARS {
            return Err(ValidationError(format!(
                "column names are 1 to {MAX_NAME_CHARS} characters of a-z, 0-9 and _, \
                 starting with a letter, e.g. due_date"
            )));
        }
        if name == ID_FIELD {
            return Err(ValidationError(
                "id is reserved: every row has one, assigned by the server".into(),
            ));
        }
        Ok(Self(name.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What a filter or sort refers to: the row id or a column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Field {
    Id,
    Column(ColumnName),
}

impl Field {
    pub fn parse(value: &str) -> Result<Self, ValidationError> {
        if value.trim() == ID_FIELD {
            return Ok(Self::Id);
        }
        ColumnName::parse(value).map(Self::Column)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_names_are_plain_file_names() {
        for name in ["books", "reading-list", "2026_goals", " books "] {
            assert!(TableName::parse(name).is_ok(), "{name}");
        }
        for name in [
            "",
            "-books",
            "_books",
            "Books",
            "books.jsonl",
            "../books",
            "a/b",
            ".github",
            "a b",
            "a\u{0}b",
            &"a".repeat(65),
        ] {
            assert!(TableName::parse(name).is_err(), "{name:?} was accepted");
        }
    }

    #[test]
    fn column_names_are_identifiers_and_id_is_reserved() {
        assert!(ColumnName::parse("due_date").is_ok());
        for name in ["", "id", "Due", "1st", "due-date", "a.b", "a b"] {
            assert!(ColumnName::parse(name).is_err(), "{name:?} was accepted");
        }
        assert_eq!(Field::parse("id").unwrap(), Field::Id);
        assert!(matches!(Field::parse("title").unwrap(), Field::Column(_)));
    }
}
