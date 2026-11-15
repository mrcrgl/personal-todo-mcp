//! Why a table operation was refused. Messages say where the problem is (item,
//! row id, column) but never echo a value: the agent knows what it sent, and
//! values are user data.

use std::fmt;

use crate::domain::table::value::Mismatch;
use crate::domain::todo::ValidationError;

/// The most rows one call may insert, update by id, or delete by id.
pub const MAX_BATCH: usize = 500;

/// Where in a call a problem is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// The n-th row given, counted from 1.
    Item(usize),
    /// An existing row.
    Row(u64),
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Item(index) => write!(f, "item {index}"),
            Self::Row(id) => write!(f, "row {id}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TableError {
    #[error(transparent)]
    Invalid(#[from] ValidationError),
    #[error("give at least one row")]
    EmptyBatch,
    #[error("give at most {MAX_BATCH} rows per call; split larger imports into several calls")]
    BatchTooLarge,
    #[error("{target}: unknown column {column}. Use table_describe to see the columns")]
    UnknownColumn { target: Target, column: String },
    #[error("{0}: id is assigned by the server and cannot be set")]
    IdGiven(Target),
    #[error("{target}, column {column}: {problem}")]
    BadValue {
        target: Target,
        column: String,
        problem: Mismatch,
    },
    #[error("{target}: column {column} is required")]
    Missing { target: Target, column: String },
    #[error("row {0} does not exist")]
    NoSuchRow(u64),
    #[error("condition {index}: {problem}")]
    BadCondition { index: usize, problem: String },
    #[error("sort {index}: {problem}")]
    BadSort { index: usize, problem: String },
    #[error("column {0} already exists")]
    ColumnExists(String),
    #[error("column {0} does not exist")]
    NoSuchColumn(String),
    #[error("column {0} is required: give a default for the existing rows")]
    DefaultNeeded(String),
    #[error("default for column {column}: {problem}")]
    BadDefault { column: String, problem: Mismatch },
    #[error("give at least one change")]
    NoChange,
}
