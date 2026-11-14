//! Table tool arguments and results, with explicit mapping to and from the
//! domain. Cell values cross as JSON and are mapped to `Raw` here; the domain
//! checks them against the schema.

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value as Json};

use crate::domain::table::change::{Alteration, Change, NewColumn};
use crate::domain::table::model::{Row, Table};
use crate::domain::table::names::{ColumnName, Field, ID_FIELD, TableName};
use crate::domain::table::query::{DEFAULT_LIMIT, MAX_LIMIT, Op, RawCondition, RawQuery, SortKey};
use crate::domain::table::schema::{Column, ColumnType, Schema, description_text};
use crate::domain::table::value::{Raw, RawRecord, Value};
use crate::domain::todo::ValidationError;
use crate::tables::service::{ChangeOutcome, QueryView};
use crate::tables::store::TableListing;

#[derive(Debug, Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ColumnTypeArg {
    String,
    Integer,
    Number,
    Boolean,
    /// YYYY-MM-DD.
    Date,
    /// RFC 3339, e.g. 2026-11-14T09:30:00Z.
    Datetime,
    /// One of `values`.
    Enum,
    /// A list of strings, e.g. tags.
    List,
}

impl ColumnTypeArg {
    fn name(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Integer => "integer",
            Self::Number => "number",
            Self::Boolean => "boolean",
            Self::Date => "date",
            Self::Datetime => "datetime",
            Self::Enum => "enum",
            Self::List => "list",
        }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ColumnArg {
    /// a-z, 0-9 and _, starting with a letter, e.g. "due_date". "id" is reserved.
    pub name: String,
    #[serde(rename = "type")]
    pub kind: ColumnTypeArg,
    /// Every row must have a value. Defaults to false.
    pub required: Option<bool>,
    /// The allowed values of an enum column, e.g. ["todo", "doing", "done"].
    pub values: Option<Vec<String>>,
    /// What the column holds, for whoever reads the table later.
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct NewColumnArg {
    #[serde(flatten)]
    pub column: ColumnArg,
    /// Value for the rows that already exist. Needed for a required column
    /// when the table has rows.
    pub default: Option<Json>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TableArgs {
    /// Table name, e.g. "reading-list".
    pub name: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TableCreateArgs {
    /// a-z, 0-9, _ and -, e.g. "reading-list".
    pub name: String,
    /// What the table is for.
    pub description: Option<String>,
    /// In display order. Every row also gets an "id", assigned by the server.
    pub columns: Vec<ColumnArg>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TableAlterArgs {
    /// Table name, e.g. "reading-list".
    pub name: String,
    /// Columns to add at the end.
    pub add: Option<Vec<NewColumnArg>>,
    /// Columns to remove, with their values. Applied before `add`, so a
    /// column can be replaced in one call.
    pub remove: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TableDropArgs {
    /// Table name, e.g. "reading-list".
    pub name: String,
    /// The sha from table_describe or table_query, so a table changed since
    /// is not dropped.
    pub sha: String,
}

#[derive(Debug, Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OpArg {
    Eq,
    Ne,
    Lt,
    Lte,
    Gt,
    Gte,
    /// Substring of a string or enum value (ignoring case), or an item of a list.
    Contains,
    /// One of a list of values.
    In,
    IsNull,
    NotNull,
}

impl From<OpArg> for Op {
    fn from(op: OpArg) -> Self {
        match op {
            OpArg::Eq => Self::Eq,
            OpArg::Ne => Self::Ne,
            OpArg::Lt => Self::Lt,
            OpArg::Lte => Self::Lte,
            OpArg::Gt => Self::Gt,
            OpArg::Gte => Self::Gte,
            OpArg::Contains => Self::Contains,
            OpArg::In => Self::In,
            OpArg::IsNull => Self::IsNull,
            OpArg::NotNull => Self::NotNull,
        }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ConditionArg {
    /// A column, or "id".
    pub column: String,
    pub op: OpArg,
    /// Compared with the column's values. A list for "in"; omit for
    /// "is_null" and "not_null".
    pub value: Option<Json>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SortArg {
    /// A column, or "id".
    pub column: String,
    /// Largest first. Rows without a value come last either way.
    pub descending: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TableQueryArgs {
    /// Table name, e.g. "reading-list".
    pub name: String,
    /// Conditions that all have to hold. Omit for every row.
    #[serde(rename = "where")]
    pub conditions: Option<Vec<ConditionArg>>,
    /// Sort keys, most important first. Defaults to id order.
    pub order_by: Option<Vec<SortArg>>,
    /// Rows to return. Defaults to 100, at most 1000.
    pub limit: Option<u32>,
    /// Rows to skip, for paging. Defaults to 0.
    pub offset: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TableInsertArgs {
    /// Table name, e.g. "reading-list".
    pub name: String,
    /// Up to 500 rows, each an object of column → value. Omit "id".
    pub rows: Vec<Map<String, Json>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RowUpdateArg {
    pub id: u64,
    /// Column → new value. null empties a cell. Columns not given keep their value.
    pub set: Map<String, Json>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TableUpdateArgs {
    /// Table name, e.g. "reading-list".
    pub name: String,
    /// Up to 500 rows by id, each with its own changes. Use either this, or
    /// `where` with `set`.
    pub rows: Option<Vec<RowUpdateArg>>,
    /// Change every row that matches all conditions.
    #[serde(rename = "where")]
    pub conditions: Option<Vec<ConditionArg>>,
    /// With `where`: column → new value for every matching row.
    pub set: Option<Map<String, Json>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TableDeleteArgs {
    /// Table name, e.g. "reading-list".
    pub name: String,
    /// Up to 500 row ids. Use either this or `where`.
    pub ids: Option<Vec<u64>>,
    /// Delete every row that matches all conditions.
    #[serde(rename = "where")]
    pub conditions: Option<Vec<ConditionArg>>,
}

fn raw_from_json(value: Json) -> Raw {
    match value {
        Json::Null => Raw::Null,
        Json::Bool(b) => Raw::Bool(b),
        Json::Number(n) => match n.as_i64() {
            Some(i) => Raw::Integer(i),
            None => Raw::Number(n.as_f64().unwrap_or(f64::NAN)),
        },
        Json::String(s) => Raw::Text(s),
        Json::Array(items) => Raw::List(items.into_iter().map(raw_from_json).collect()),
        Json::Object(_) => Raw::Object,
    }
}

fn record(cells: Map<String, Json>) -> RawRecord {
    cells
        .into_iter()
        .map(|(name, value)| (name, raw_from_json(value)))
        .collect()
}

impl ColumnArg {
    fn into_column(self) -> Result<Column, ValidationError> {
        let name = ColumnName::parse(&self.name)?;
        let kind = ColumnType::parse(self.kind.name(), self.values)
            .map_err(|e| ValidationError(format!("column {}: {e}", name.as_str())))?;
        Column::new(name, kind, self.required.unwrap_or(false), self.description)
    }
}

fn conditions(args: Vec<ConditionArg>) -> Result<Vec<RawCondition>, ValidationError> {
    args.into_iter()
        .map(|arg| {
            Ok(RawCondition {
                field: Field::parse(&arg.column)?,
                op: arg.op.into(),
                value: arg.value.map(raw_from_json),
            })
        })
        .collect()
}

impl TableArgs {
    pub fn into_name(self) -> Result<TableName, ValidationError> {
        TableName::parse(&self.name)
    }
}

/// A create request in domain terms.
pub struct CreateRequest {
    pub name: TableName,
    pub schema: Schema,
    pub description: Option<String>,
}

impl TableCreateArgs {
    pub fn into_request(self) -> Result<CreateRequest, ValidationError> {
        let columns = self
            .columns
            .into_iter()
            .map(ColumnArg::into_column)
            .collect::<Result<_, _>>()?;
        Ok(CreateRequest {
            name: TableName::parse(&self.name)?,
            schema: Schema::new(columns)?,
            description: description_text(self.description)?,
        })
    }
}

impl TableAlterArgs {
    pub fn into_change(self) -> Result<(TableName, Change), ValidationError> {
        let add = self
            .add
            .unwrap_or_default()
            .into_iter()
            .map(|arg| {
                Ok(NewColumn {
                    column: arg.column.into_column()?,
                    default: arg.default.map(raw_from_json),
                })
            })
            .collect::<Result<_, ValidationError>>()?;
        let remove = self
            .remove
            .unwrap_or_default()
            .iter()
            .map(|name| ColumnName::parse(name))
            .collect::<Result<_, _>>()?;
        Ok((
            TableName::parse(&self.name)?,
            Change::Alter(Alteration { add, remove }),
        ))
    }
}

impl TableDropArgs {
    pub fn into_drop(self) -> Result<(TableName, String), ValidationError> {
        let sha = self.sha.trim().to_string();
        if sha.is_empty() {
            return Err(ValidationError("sha must not be empty".into()));
        }
        Ok((TableName::parse(&self.name)?, sha))
    }
}

impl TableQueryArgs {
    pub fn into_query(self) -> Result<(TableName, RawQuery), ValidationError> {
        let sort = self
            .order_by
            .unwrap_or_default()
            .into_iter()
            .map(|arg| {
                Ok(SortKey {
                    field: Field::parse(&arg.column)?,
                    descending: arg.descending.unwrap_or(false),
                })
            })
            .collect::<Result<_, ValidationError>>()?;
        let limit = self.limit.map_or(DEFAULT_LIMIT, |limit| limit as usize);
        Ok((
            TableName::parse(&self.name)?,
            RawQuery {
                conditions: conditions(self.conditions.unwrap_or_default())?,
                sort,
                offset: self.offset.unwrap_or(0) as usize,
                limit: limit.clamp(1, MAX_LIMIT),
            },
        ))
    }
}

impl TableInsertArgs {
    pub fn into_change(self) -> Result<(TableName, Change), ValidationError> {
        Ok((
            TableName::parse(&self.name)?,
            Change::Insert(self.rows.into_iter().map(record).collect()),
        ))
    }
}

impl TableUpdateArgs {
    pub fn into_change(self) -> Result<(TableName, Change), ValidationError> {
        let name = TableName::parse(&self.name)?;
        let change = match (self.rows, self.conditions, self.set) {
            (Some(rows), None, None) => Change::Update(
                rows.into_iter()
                    .map(|row| (row.id, record(row.set)))
                    .collect(),
            ),
            (None, Some(where_), Some(set)) => Change::UpdateWhere {
                conditions: conditions(where_)?,
                set: record(set),
            },
            _ => {
                return Err(ValidationError(
                    "give either rows (changes by id), or where together with set".into(),
                ));
            }
        };
        Ok((name, change))
    }
}

impl TableDeleteArgs {
    pub fn into_change(self) -> Result<(TableName, Change), ValidationError> {
        let name = TableName::parse(&self.name)?;
        let change = match (self.ids, self.conditions) {
            (Some(ids), None) => Change::Delete(ids),
            (None, Some(where_)) => Change::DeleteWhere(conditions(where_)?),
            _ => return Err(ValidationError("give either ids or where".into())),
        };
        Ok((name, change))
    }
}

#[derive(Debug, Serialize)]
pub struct TableEntryDto {
    pub name: String,
    pub size: u64,
}

#[derive(Debug, Serialize)]
pub struct TableListingDto {
    pub tables: Vec<TableEntryDto>,
    pub truncated: bool,
}

impl From<TableListing> for TableListingDto {
    fn from(listing: TableListing) -> Self {
        Self {
            tables: listing
                .tables
                .into_iter()
                .map(|entry| TableEntryDto {
                    name: entry.name.as_str().to_string(),
                    size: entry.size,
                })
                .collect(),
            truncated: listing.truncated,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ColumnDto {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

fn columns(schema: &Schema) -> Vec<ColumnDto> {
    schema
        .columns()
        .iter()
        .map(|column| ColumnDto {
            name: column.name.as_str().to_string(),
            kind: column.kind.name(),
            required: column.required,
            values: match &column.kind {
                ColumnType::Enum(values) => Some(values.clone()),
                _ => None,
            },
            description: column.description.clone(),
        })
        .collect()
}

/// A table's shape, as table_describe, table_create, and table_alter show it.
#[derive(Debug, Serialize)]
pub struct TableDto {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub sha: String,
    pub rows: usize,
    pub columns: Vec<ColumnDto>,
}

impl TableDto {
    pub fn new(name: &TableName, table: &Table, sha: String) -> Self {
        Self {
            name: name.as_str().to_string(),
            description: table.description().map(str::to_string),
            sha,
            rows: table.rows().len(),
            columns: columns(table.schema()),
        }
    }
}

fn json_from_value(value: &Value) -> Json {
    match value {
        Value::Text(text) => Json::String(text.clone()),
        Value::Integer(n) => Json::from(*n),
        Value::Number(n) => Number::from_f64(*n).map_or(Json::Null, Json::Number),
        Value::Boolean(b) => Json::Bool(*b),
        Value::Date(date) => Json::String(Value::date_text(date)),
        Value::DateTime(at) => Json::String(Value::datetime_text(at)),
        Value::List(items) => Json::Array(items.iter().cloned().map(Json::String).collect()),
    }
}

/// `id` first, then the cells in column order. Empty cells are left out.
fn row_json(schema: &Schema, row: &Row) -> Map<String, Json> {
    let mut object = Map::new();
    object.insert(ID_FIELD.into(), Json::from(row.id));
    for column in schema.columns() {
        if let Some(value) = row.values.get(&column.name) {
            object.insert(column.name.as_str().to_string(), json_from_value(value));
        }
    }
    object
}

#[derive(Debug, Serialize)]
pub struct QueryDto {
    pub sha: String,
    pub total_rows: usize,
    pub matched: usize,
    pub offset: usize,
    /// Where the next page starts, if there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<usize>,
    pub rows: Vec<Map<String, Json>>,
}

impl From<QueryView> for QueryDto {
    fn from(view: QueryView) -> Self {
        let end = view.offset + view.rows.len();
        Self {
            next_offset: (end < view.matched && !view.rows.is_empty()).then_some(end),
            rows: view
                .rows
                .iter()
                .map(|row| row_json(&view.schema, row))
                .collect(),
            sha: view.sha,
            total_rows: view.total_rows,
            matched: view.matched,
            offset: view.offset,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct InsertDto {
    pub sha: String,
    pub inserted: usize,
    pub ids: Vec<u64>,
    pub total_rows: usize,
}

impl From<ChangeOutcome> for InsertDto {
    fn from(outcome: ChangeOutcome) -> Self {
        Self {
            inserted: outcome.applied.inserted.len(),
            ids: outcome.applied.inserted,
            total_rows: outcome.table.rows().len(),
            sha: outcome.sha,
        }
    }
}

/// The result of an update or delete. `affected` is named by the tool.
pub fn affected_json(outcome: &ChangeOutcome, verb: &str) -> Json {
    let mut object = Map::new();
    object.insert("sha".into(), Json::String(outcome.sha.clone()));
    object.insert(verb.into(), Json::from(outcome.applied.affected));
    object.insert("total_rows".into(), Json::from(outcome.table.rows().len()));
    Json::Object(object)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn parse<T: serde::de::DeserializeOwned>(value: Json) -> T {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn create_arguments_map_to_a_schema() {
        let request = parse::<TableCreateArgs>(json!({
            "name": "books",
            "description": "  ",
            "columns": [
                {"name": "title", "type": "string", "required": true},
                {"name": "status", "type": "enum", "values": ["unread", "read"]},
                {"name": "finished", "type": "date", "description": "When I finished it"},
            ],
        }))
        .into_request()
        .unwrap();
        assert_eq!(request.name.as_str(), "books");
        assert_eq!(request.description, None);
        let kinds: Vec<&str> = request
            .schema
            .columns()
            .iter()
            .map(|c| c.kind.name())
            .collect();
        assert_eq!(kinds, ["string", "enum", "date"]);
    }

    #[test]
    fn bad_names_are_refused_at_the_boundary() {
        for name in ["../x", "Books", "a/b", ".github"] {
            assert!(
                parse::<TableArgs>(json!({"name": name}))
                    .into_name()
                    .is_err()
            );
        }
        assert!(
            parse::<TableCreateArgs>(json!({
                "name": "books",
                "columns": [{"name": "id", "type": "integer"}],
            }))
            .into_request()
            .is_err()
        );
    }

    #[test]
    fn update_and_delete_take_exactly_one_mode() {
        let update = |value| parse::<TableUpdateArgs>(value).into_change();
        assert!(update(json!({"name": "t", "rows": [{"id": 1, "set": {"a": 1}}]})).is_ok());
        assert!(
            update(
                json!({"name": "t", "where": [{"column": "id", "op": "not_null"}], "set": {"a": 1}})
            )
            .is_ok()
        );
        assert!(update(json!({"name": "t", "where": []})).is_err());
        assert!(update(json!({"name": "t", "rows": [], "where": [], "set": {}})).is_err());

        let delete = |value| parse::<TableDeleteArgs>(value).into_change();
        assert!(delete(json!({"name": "t", "ids": [1]})).is_ok());
        assert!(delete(json!({"name": "t"})).is_err());
        assert!(delete(json!({"name": "t", "ids": [1], "where": []})).is_err());
    }

    #[test]
    fn queries_default_and_clamp_paging() {
        let (_, query) = parse::<TableQueryArgs>(json!({"name": "t", "limit": 99_999}))
            .into_query()
            .unwrap();
        assert_eq!((query.offset, query.limit), (0, MAX_LIMIT));
        let (_, query) = parse::<TableQueryArgs>(json!({
            "name": "t",
            "where": [{"column": "pages", "op": "gt", "value": 100}],
            "order_by": [{"column": "id", "descending": true}],
        }))
        .into_query()
        .unwrap();
        assert_eq!(query.limit, DEFAULT_LIMIT);
        assert_eq!(query.conditions[0].value, Some(Raw::Integer(100)));
        assert_eq!(query.sort[0].field, Field::Id);
    }

    #[test]
    fn json_values_map_to_raw_values() {
        assert_eq!(raw_from_json(json!(1.5)), Raw::Number(1.5));
        assert_eq!(raw_from_json(json!(-2)), Raw::Integer(-2));
        assert_eq!(
            raw_from_json(json!(["a", null])),
            Raw::List(vec![Raw::Text("a".into()), Raw::Null])
        );
        assert_eq!(raw_from_json(json!({"a": 1})), Raw::Object);
    }
}
