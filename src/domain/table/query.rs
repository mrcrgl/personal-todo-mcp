//! Filters and sorting. Conditions arrive raw and are compiled against the
//! schema, so a filter that names an unknown column or compares a date with a
//! number fails before any row is looked at.

use std::cmp::Ordering;

use crate::domain::table::error::TableError;
use crate::domain::table::model::Row;
use crate::domain::table::names::Field;
use crate::domain::table::schema::{ColumnType, Schema};
use crate::domain::table::value::{Raw, Value};

const MAX_CONDITIONS: usize = 20;
const MAX_SORT_KEYS: usize = 5;
const MAX_IN_VALUES: usize = 500;
pub const DEFAULT_LIMIT: usize = 100;
pub const MAX_LIMIT: usize = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Eq,
    Ne,
    Lt,
    Lte,
    Gt,
    Gte,
    /// Substring of a string (ignoring case), or an item of a list.
    Contains,
    In,
    IsNull,
    NotNull,
}

/// A condition as given: unchecked field and operand.
#[derive(Debug, Clone, PartialEq)]
pub struct RawCondition {
    pub field: Field,
    pub op: Op,
    pub value: Option<Raw>,
}

#[derive(Debug, Clone)]
enum Operand {
    None,
    One(Value),
    Many(Vec<Value>),
    /// Lowercased, for case-insensitive substring matches.
    Needle(String),
}

#[derive(Debug, Clone)]
struct Condition {
    field: Field,
    op: Op,
    operand: Operand,
}

/// Conditions that all have to hold. Empty matches every row.
#[derive(Debug, Clone, Default)]
pub struct Filter(Vec<Condition>);

impl Filter {
    pub fn compile(schema: &Schema, conditions: &[RawCondition]) -> Result<Self, TableError> {
        if conditions.len() > MAX_CONDITIONS {
            return Err(TableError::BadCondition {
                index: MAX_CONDITIONS + 1,
                problem: format!("give at most {MAX_CONDITIONS} conditions"),
            });
        }
        conditions
            .iter()
            .enumerate()
            .map(|(index, condition)| {
                compile_condition(schema, condition).map_err(|problem| TableError::BadCondition {
                    index: index + 1,
                    problem,
                })
            })
            .collect::<Result<_, _>>()
            .map(Self)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn matches(&self, row: &Row) -> bool {
        self.0.iter().all(|condition| condition.matches(row))
    }
}

/// The id column behaves like a required integer column.
const ID_TYPE: ColumnType = ColumnType::Integer;

fn field_type<'a>(schema: &'a Schema, field: &Field) -> Result<&'a ColumnType, String> {
    match field {
        Field::Id => Ok(&ID_TYPE),
        Field::Column(name) => schema
            .column(name.as_str())
            .map(|column| &column.kind)
            .ok_or_else(|| format!("unknown column {}", name.as_str())),
    }
}

fn compile_condition(schema: &Schema, condition: &RawCondition) -> Result<Condition, String> {
    let kind = field_type(schema, &condition.field)?;
    let value = condition.value.as_ref().filter(|v| **v != Raw::Null);
    let operand = match (condition.op, value) {
        (Op::IsNull | Op::NotNull, None) => Operand::None,
        (Op::IsNull | Op::NotNull, Some(_)) => {
            return Err("is_null and not_null take no value".into());
        }
        (_, None) => return Err("this operator needs a value".into()),
        (Op::Eq | Op::Ne, Some(raw)) => Operand::One(typed(kind, raw)?),
        (Op::Lt | Op::Lte | Op::Gt | Op::Gte, Some(raw)) => {
            if !kind.is_ordered() {
                return Err(format!(
                    "{} columns cannot be compared by size",
                    kind.name()
                ));
            }
            Operand::One(typed(kind, raw)?)
        }
        (Op::Contains, Some(Raw::Text(text))) => match kind {
            ColumnType::String | ColumnType::Enum(_) | ColumnType::List => {
                Operand::Needle(text.to_lowercase())
            }
            _ => return Err("contains works on string, enum, and list columns".into()),
        },
        (Op::Contains, Some(_)) => return Err("contains needs a string".into()),
        (Op::In, Some(Raw::List(items))) => {
            if items.len() > MAX_IN_VALUES {
                return Err(format!("in takes at most {MAX_IN_VALUES} values"));
            }
            Operand::Many(
                items
                    .iter()
                    .map(|item| typed(kind, item))
                    .collect::<Result<_, _>>()?,
            )
        }
        (Op::In, Some(_)) => return Err("in needs a list of values".into()),
    };
    Ok(Condition {
        field: condition.field.clone(),
        op: condition.op,
        operand,
    })
}

fn typed(kind: &ColumnType, raw: &Raw) -> Result<Value, String> {
    match kind.coerce(raw) {
        Ok(Some(value)) => Ok(value),
        Ok(None) => Err("the value must not be null; use is_null instead".into()),
        Err(problem) => Err(format!("the value does not fit the column: {problem}")),
    }
}

impl Condition {
    fn matches(&self, row: &Row) -> bool {
        let id;
        let value = match &self.field {
            Field::Id => {
                id = Value::Integer(row.id as i64);
                Some(&id)
            }
            Field::Column(name) => row.values.get(name),
        };
        let ordering = |operand: &Value| value.and_then(|v| v.compare(operand));
        match (&self.operand, self.op) {
            (_, Op::IsNull) => value.is_none(),
            (_, Op::NotNull) => value.is_some(),
            (Operand::One(operand), Op::Eq) => ordering(operand) == Some(Ordering::Equal),
            // A row without a value is "not equal" too, which is what "status
            // is not done" means to a person.
            (Operand::One(operand), Op::Ne) => ordering(operand) != Some(Ordering::Equal),
            (Operand::One(operand), Op::Lt) => ordering(operand) == Some(Ordering::Less),
            (Operand::One(operand), Op::Lte) => {
                matches!(ordering(operand), Some(Ordering::Less | Ordering::Equal))
            }
            (Operand::One(operand), Op::Gt) => ordering(operand) == Some(Ordering::Greater),
            (Operand::One(operand), Op::Gte) => {
                matches!(ordering(operand), Some(Ordering::Greater | Ordering::Equal))
            }
            (Operand::Many(operands), Op::In) => operands
                .iter()
                .any(|operand| ordering(operand) == Some(Ordering::Equal)),
            (Operand::Needle(needle), Op::Contains) => match value {
                Some(Value::Text(text)) => text.to_lowercase().contains(needle),
                Some(Value::List(items)) => items.iter().any(|item| item.to_lowercase() == *needle),
                _ => false,
            },
            _ => false,
        }
    }
}

/// A sort key as given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SortKey {
    pub field: Field,
    pub descending: bool,
}

/// Sort keys checked against the schema. Ties fall back to the id.
#[derive(Debug, Clone, Default)]
pub struct Sort(Vec<SortKey>);

impl Sort {
    pub fn compile(schema: &Schema, keys: Vec<SortKey>) -> Result<Self, TableError> {
        if keys.len() > MAX_SORT_KEYS {
            return Err(TableError::BadSort {
                index: MAX_SORT_KEYS + 1,
                problem: format!("give at most {MAX_SORT_KEYS} sort keys"),
            });
        }
        for (index, key) in keys.iter().enumerate() {
            field_type(schema, &key.field).map_err(|problem| TableError::BadSort {
                index: index + 1,
                problem,
            })?;
        }
        Ok(Self(keys))
    }

    /// Rows without a value come last, whichever the direction.
    pub fn compare(&self, a: &Row, b: &Row) -> Ordering {
        for key in &self.0 {
            let ordering = match &key.field {
                Field::Id => a.id.cmp(&b.id),
                Field::Column(name) => match (a.values.get(name), b.values.get(name)) {
                    (Some(x), Some(y)) => x.compare(y).unwrap_or(Ordering::Equal),
                    (Some(_), None) => return Ordering::Less,
                    (None, Some(_)) => return Ordering::Greater,
                    (None, None) => Ordering::Equal,
                },
            };
            let ordering = if key.descending {
                ordering.reverse()
            } else {
                ordering
            };
            if ordering != Ordering::Equal {
                return ordering;
            }
        }
        a.id.cmp(&b.id)
    }
}

/// A checked query.
#[derive(Debug, Clone)]
pub struct Query {
    pub filter: Filter,
    pub sort: Sort,
    pub offset: usize,
    pub limit: usize,
}

/// A query before it meets the schema it runs against.
#[derive(Debug, Clone)]
pub struct RawQuery {
    pub conditions: Vec<RawCondition>,
    pub sort: Vec<SortKey>,
    pub offset: usize,
    pub limit: usize,
}

impl RawQuery {
    pub fn compile(&self, schema: &Schema) -> Result<Query, TableError> {
        Ok(Query {
            filter: Filter::compile(schema, &self.conditions)?,
            sort: Sort::compile(schema, self.sort.clone())?,
            offset: self.offset,
            limit: self.limit.clamp(1, MAX_LIMIT),
        })
    }
}
