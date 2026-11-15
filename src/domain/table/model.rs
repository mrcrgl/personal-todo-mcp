//! A table and its rows. Every row conforms to the schema: rows are only
//! built through `record_values`, and a schema change rewrites the rows.

use std::collections::{BTreeMap, HashSet};

use crate::domain::table::error::{TableError, Target};
use crate::domain::table::names::{ColumnName, ID_FIELD};
use crate::domain::table::query::Query;
use crate::domain::table::schema::Schema;
use crate::domain::table::value::{RawRecord, Value};

/// The largest a stored table may be. Every query reads the whole table, and
/// GitHub serves file contents inline only up to 1 MB.
pub const MAX_TABLE_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub id: u64,
    /// Empty cells are absent.
    pub values: BTreeMap<ColumnName, Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    // Visible to `change`, which applies the operations that keep them valid.
    pub(super) description: Option<String>,
    pub(super) schema: Schema,
    pub(super) next_id: u64,
    pub(super) rows: Vec<Row>,
}

/// A row as stored, before it is checked against the schema.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredRow {
    pub id: u64,
    pub record: RawRecord,
}

/// A stored row that does not fit the table, e.g. after a hand edit. `index`
/// counts stored rows from 1; the adapter knows which line that is.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RestoreError {
    #[error("{problem}")]
    Row { index: usize, problem: String },
    #[error("id {id} appears more than once")]
    DuplicateId { index: usize, id: u64 },
}

impl RestoreError {
    pub fn index(&self) -> usize {
        match self {
            Self::Row { index, .. } | Self::DuplicateId { index, .. } => *index,
        }
    }
}

pub struct QueryResult<'a> {
    pub matched: usize,
    pub rows: Vec<&'a Row>,
}

impl Table {
    pub fn new(schema: Schema, description: Option<String>) -> Self {
        Self {
            description,
            schema,
            next_id: 1,
            rows: Vec::new(),
        }
    }

    /// Rebuilds a stored table. Rows are checked like new rows, so a table
    /// that loads can be written back unchanged. `next_id` never goes back to
    /// an id in use, even if a hand edit added rows.
    pub fn restore(
        schema: Schema,
        description: Option<String>,
        next_id: u64,
        stored: Vec<StoredRow>,
    ) -> Result<Self, RestoreError> {
        let mut ids = HashSet::new();
        let mut rows = Vec::with_capacity(stored.len());
        for (index, row) in stored.into_iter().enumerate() {
            let index = index + 1;
            if !ids.insert(row.id) {
                return Err(RestoreError::DuplicateId { index, id: row.id });
            }
            let values = record_values(&schema, Target::Row(row.id), &row.record, None).map_err(
                |error| RestoreError::Row {
                    index,
                    problem: error.to_string(),
                },
            )?;
            rows.push(Row { id: row.id, values });
        }
        let next_id = rows
            .iter()
            .map(|row| row.id + 1)
            .max()
            .unwrap_or(1)
            .max(next_id)
            .max(1);
        Ok(Self {
            description,
            schema,
            next_id,
            rows,
        })
    }

    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    pub fn schema(&self) -> &Schema {
        &self.schema
    }

    pub fn next_id(&self) -> u64 {
        self.next_id
    }

    /// In id order.
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn query(&self, query: &Query) -> QueryResult<'_> {
        let mut matching: Vec<&Row> = self
            .rows
            .iter()
            .filter(|row| query.filter.matches(row))
            .collect();
        matching.sort_by(|a, b| query.sort.compare(a, b));
        let matched = matching.len();
        let rows = matching
            .into_iter()
            .skip(query.offset)
            .take(query.limit)
            .collect();
        QueryResult { matched, rows }
    }
}

/// Checks `record` against `schema`. With `base`, the record changes an
/// existing row: given cells replace the row's, and `null` empties one.
pub(super) fn record_values(
    schema: &Schema,
    target: Target,
    record: &RawRecord,
    base: Option<&BTreeMap<ColumnName, Value>>,
) -> Result<BTreeMap<ColumnName, Value>, TableError> {
    let mut values = base.cloned().unwrap_or_default();
    for (name, raw) in record {
        if name == ID_FIELD {
            return Err(TableError::IdGiven(target));
        }
        let column = schema
            .column(name)
            .ok_or_else(|| TableError::UnknownColumn {
                target,
                column: name.clone(),
            })?;
        match column.kind.coerce(raw) {
            Ok(Some(value)) => {
                values.insert(column.name.clone(), value);
            }
            Ok(None) => {
                values.remove(&column.name);
            }
            Err(problem) => {
                return Err(TableError::BadValue {
                    target,
                    column: name.clone(),
                    problem,
                });
            }
        }
    }
    if let Some(missing) = schema
        .columns()
        .iter()
        .find(|column| column.required && !values.contains_key(&column.name))
    {
        return Err(TableError::Missing {
            target,
            column: missing.name.as_str().to_string(),
        });
    }
    Ok(values)
}
