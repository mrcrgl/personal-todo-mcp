//! Columns and the schema they form.

use std::collections::HashSet;

use crate::domain::table::names::ColumnName;
use crate::domain::todo::ValidationError;

const MAX_COLUMNS: usize = 50;
const MAX_ENUM_VALUES: usize = 100;
const MAX_ENUM_VALUE_CHARS: usize = 100;
pub const MAX_DESCRIPTION_CHARS: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColumnType {
    String,
    Integer,
    Number,
    Boolean,
    Date,
    DateTime,
    /// One of a fixed set of strings.
    Enum(Vec<String>),
    /// A list of strings, e.g. tags.
    List,
}

impl ColumnType {
    /// The type's name in tool arguments and in storage.
    pub fn name(&self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Integer => "integer",
            Self::Number => "number",
            Self::Boolean => "boolean",
            Self::Date => "date",
            Self::DateTime => "datetime",
            Self::Enum(_) => "enum",
            Self::List => "list",
        }
    }

    /// `values` is required for `enum` and refused for every other type.
    pub fn parse(name: &str, values: Option<Vec<String>>) -> Result<Self, ValidationError> {
        let kind = match name.trim() {
            "string" => Self::String,
            "integer" => Self::Integer,
            "number" => Self::Number,
            "boolean" => Self::Boolean,
            "date" => Self::Date,
            "datetime" => Self::DateTime,
            "list" => Self::List,
            "enum" => {
                let values = values.ok_or_else(|| {
                    ValidationError("an enum column needs its list of values".into())
                })?;
                return Ok(Self::Enum(enum_values(values)?));
            }
            _ => {
                return Err(ValidationError(
                    "column types are string, integer, number, boolean, date, datetime, enum, \
                     and list"
                        .into(),
                ));
            }
        };
        if values.is_some() {
            return Err(ValidationError(
                "only enum columns take a list of values".into(),
            ));
        }
        Ok(kind)
    }

    /// Whether `<`, `>` and sorting by meaning make sense.
    pub fn is_ordered(&self) -> bool {
        matches!(
            self,
            Self::String
                | Self::Integer
                | Self::Number
                | Self::Date
                | Self::DateTime
                | Self::Enum(_)
        )
    }
}

fn enum_values(values: Vec<String>) -> Result<Vec<String>, ValidationError> {
    if values.is_empty() || values.len() > MAX_ENUM_VALUES {
        return Err(ValidationError(format!(
            "an enum column has 1 to {MAX_ENUM_VALUES} values"
        )));
    }
    let mut seen = HashSet::new();
    for value in &values {
        if value.trim().is_empty() || value.chars().count() > MAX_ENUM_VALUE_CHARS {
            return Err(ValidationError(format!(
                "enum values are 1 to {MAX_ENUM_VALUE_CHARS} characters"
            )));
        }
        if !seen.insert(value.as_str()) {
            return Err(ValidationError("enum values must be unique".into()));
        }
    }
    Ok(values)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    pub name: ColumnName,
    pub kind: ColumnType,
    pub required: bool,
    pub description: Option<String>,
}

impl Column {
    pub fn new(
        name: ColumnName,
        kind: ColumnType,
        required: bool,
        description: Option<String>,
    ) -> Result<Self, ValidationError> {
        Ok(Self {
            name,
            kind,
            required,
            description: description_text(description)?,
        })
    }
}

/// A description for a table or column. Blank means none.
pub fn description_text(value: Option<String>) -> Result<Option<String>, ValidationError> {
    let Some(value) = value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
    else {
        return Ok(None);
    };
    if value.chars().count() > MAX_DESCRIPTION_CHARS {
        return Err(ValidationError(format!(
            "descriptions are at most {MAX_DESCRIPTION_CHARS} characters"
        )));
    }
    Ok(Some(value))
}

/// The columns of a table, in order. Names are unique.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schema(Vec<Column>);

impl Schema {
    pub fn new(columns: Vec<Column>) -> Result<Self, ValidationError> {
        if columns.is_empty() || columns.len() > MAX_COLUMNS {
            return Err(ValidationError(format!(
                "a table has 1 to {MAX_COLUMNS} columns"
            )));
        }
        let mut seen = HashSet::new();
        for column in &columns {
            if !seen.insert(&column.name) {
                return Err(ValidationError(format!(
                    "column {} appears twice",
                    column.name.as_str()
                )));
            }
        }
        Ok(Self(columns))
    }

    pub fn columns(&self) -> &[Column] {
        &self.0
    }

    pub fn column(&self, name: &str) -> Option<&Column> {
        self.0.iter().find(|column| column.name.as_str() == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column(name: &str) -> Column {
        Column::new(
            ColumnName::parse(name).unwrap(),
            ColumnType::String,
            false,
            None,
        )
        .unwrap()
    }

    #[test]
    fn column_types_parse_by_name() {
        assert_eq!(ColumnType::parse("date", None).unwrap(), ColumnType::Date);
        assert_eq!(
            ColumnType::parse("enum", Some(vec!["a".into(), "b".into()])).unwrap(),
            ColumnType::Enum(vec!["a".into(), "b".into()])
        );
        assert!(ColumnType::parse("enum", None).is_err());
        assert!(ColumnType::parse("enum", Some(vec!["a".into(), "a".into()])).is_err());
        assert!(ColumnType::parse("enum", Some(vec![])).is_err());
        assert!(ColumnType::parse("string", Some(vec!["a".into()])).is_err());
        assert!(ColumnType::parse("text", None).is_err());
    }

    #[test]
    fn a_schema_has_unique_columns() {
        assert!(Schema::new(vec![column("a"), column("b")]).is_ok());
        assert!(Schema::new(vec![column("a"), column("a")]).is_err());
        assert!(Schema::new(vec![]).is_err());
        assert!(Schema::new((0..51).map(|n| column(&format!("c{n}"))).collect()).is_err());
    }

    #[test]
    fn blank_descriptions_are_none() {
        assert_eq!(description_text(Some("  ".into())).unwrap(), None);
        assert!(description_text(Some("x".repeat(501))).is_err());
    }
}
