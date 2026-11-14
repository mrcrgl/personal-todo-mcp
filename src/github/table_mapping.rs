//! Repository contents ↔ tables. A table is one JSONL file below `tables/`:
//! line 1 is the header (`TableHeaderRow`), every further line one row.
//! The folder, the extension, and the line format appear here and nowhere
//! else.

use serde_json::{Map, Number, Value as Json};

use crate::domain::table::model::{MAX_TABLE_BYTES, StoredRow, Table};
use crate::domain::table::names::{ColumnName, ID_FIELD, TableName};
use crate::domain::table::schema::{Column, ColumnType, Schema};
use crate::domain::table::value::{Raw, RawRecord, Value};
use crate::github::contents;
use crate::github::models::{ColumnRow, ContentRow, TableHeaderRow, TreeRow};
use crate::tables::store::{TableEntry, TableListing, TableStoreError};
use crate::todos::store::StoreError;

/// Where tables live in the repository.
pub const ROOT: &str = "tables";
const EXTENSION: &str = ".jsonl";
const FORMAT_VERSION: u32 = 1;

/// The table's file name inside `ROOT`.
pub fn file_name(name: &TableName) -> String {
    format!("{}{EXTENSION}", name.as_str())
}

/// The path as it appears in the repository, e.g. for commit messages.
pub fn repository_path(name: &TableName) -> String {
    format!("{ROOT}/{}", file_name(name))
}

/// Keeps `tables/<name>.jsonl` files whose name would pass as a tool
/// argument, so every listed table can be read. Subfolders are ignored.
pub fn listing_from_tree(row: TreeRow) -> TableListing {
    let prefix = format!("{ROOT}/");
    let tables = row
        .tree
        .into_iter()
        .filter(|entry| entry.kind == "blob")
        .filter_map(|entry| {
            let stem = entry.path.strip_prefix(&prefix)?.strip_suffix(EXTENSION)?;
            let name = TableName::parse(stem).ok()?;
            (name.as_str() == stem).then_some(TableEntry {
                name,
                size: entry.size.unwrap_or(0),
            })
        })
        .collect();
    TableListing {
        tables,
        truncated: row.truncated,
    }
}

pub fn table_from_content(row: ContentRow) -> Result<Table, TableStoreError> {
    if row.kind != "file" {
        return Err(TableStoreError::NotFound);
    }
    if row.size > MAX_TABLE_BYTES as u64 {
        return Err(TableStoreError::TooLarge);
    }
    let bytes = contents::decoded_bytes(&row)
        .ok_or_else(|| TableStoreError::Unavailable("GitHub returned unreadable content".into()))?;
    let text = String::from_utf8(bytes).map_err(|_| damaged(1, "the file is not UTF-8 text"))?;
    decode(&text)
}

fn damaged(line: usize, problem: impl Into<String>) -> TableStoreError {
    TableStoreError::Damaged {
        line,
        problem: problem.into(),
    }
}

/// Parses a stored table. Problems name the line, never its content.
pub fn decode(text: &str) -> Result<Table, TableStoreError> {
    let mut lines = text
        .lines()
        .enumerate()
        .map(|(index, line)| (index + 1, line))
        .filter(|(_, line)| !line.trim().is_empty());
    let (header_line, header) = lines
        .next()
        .ok_or_else(|| damaged(1, "the header line is missing"))?;
    let header: TableHeaderRow = serde_json::from_str(header)
        .map_err(|_| damaged(header_line, "the header is not a valid table header"))?;
    if header.version != FORMAT_VERSION {
        return Err(damaged(
            header_line,
            format!("format version {} is not supported", header.version),
        ));
    }
    let schema = schema_from_header(&header).map_err(|problem| damaged(header_line, problem))?;

    let mut line_numbers = Vec::new();
    let mut rows = Vec::new();
    for (number, line) in lines {
        rows.push(row_from_line(line).map_err(|problem| damaged(number, problem))?);
        line_numbers.push(number);
    }
    Table::restore(schema, header.description, header.next_id, rows).map_err(|error| {
        let line = line_numbers
            .get(error.index().saturating_sub(1))
            .copied()
            .unwrap_or(header_line);
        damaged(line, error.to_string())
    })
}

fn schema_from_header(header: &TableHeaderRow) -> Result<Schema, String> {
    let columns = header
        .columns
        .iter()
        .map(|row| {
            let name = ColumnName::parse(&row.name).map_err(|e| e.to_string())?;
            let kind = ColumnType::parse(&row.kind, row.values.clone())
                .map_err(|e| format!("column {}: {e}", name.as_str()))?;
            Column::new(name, kind, row.required, row.description.clone())
                .map_err(|e| e.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    Schema::new(columns).map_err(|e| e.to_string())
}

fn row_from_line(line: &str) -> Result<StoredRow, String> {
    let Ok(Json::Object(mut object)) = serde_json::from_str::<Json>(line) else {
        return Err("a row must be a JSON object".into());
    };
    let id = object
        .remove(ID_FIELD)
        .and_then(|id| id.as_u64())
        .filter(|id| *id > 0)
        .ok_or("a row needs a positive integer id")?;
    let record: RawRecord = object
        .into_iter()
        .map(|(name, value)| (name, raw_from_json(value)))
        .collect();
    Ok(StoredRow { id, record })
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

/// The header, then one line per row in id order. Cells follow the schema's
/// column order, so diffs in the repository stay readable.
pub fn encode(table: &Table) -> String {
    let header = TableHeaderRow {
        version: FORMAT_VERSION,
        description: table.description().map(str::to_string),
        next_id: table.next_id(),
        columns: table
            .schema()
            .columns()
            .iter()
            .map(|column| ColumnRow {
                name: column.name.as_str().to_string(),
                kind: column.kind.name().to_string(),
                required: column.required,
                values: match &column.kind {
                    ColumnType::Enum(values) => Some(values.clone()),
                    _ => None,
                },
                description: column.description.clone(),
            })
            .collect(),
    };
    let mut out = serde_json::to_string(&header).expect("header serializes to JSON");
    out.push('\n');
    for row in table.rows() {
        let mut object = Map::new();
        object.insert(ID_FIELD.into(), Json::from(row.id));
        for column in table.schema().columns() {
            if let Some(value) = row.values.get(&column.name) {
                object.insert(column.name.as_str().to_string(), json_from_value(value));
            }
        }
        out.push_str(&Json::Object(object).to_string());
        out.push('\n');
    }
    out
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

/// The encoded table, or `TooLarge` when it would exceed the limit.
pub fn encoded_content(table: &Table) -> Result<String, TableStoreError> {
    let text = encode(table);
    if text.len() > MAX_TABLE_BYTES {
        return Err(TableStoreError::TooLarge);
    }
    Ok(contents::encoded(text.as_bytes()))
}

/// The failures every table operation shares. Status codes whose meaning
/// depends on the operation are mapped by the caller first.
pub fn table_error(error: StoreError) -> TableStoreError {
    match error {
        StoreError::Rejected { status: 404, .. } | StoreError::NotFound(_) => {
            TableStoreError::NotFound
        }
        StoreError::Rejected { status: 409, .. } => TableStoreError::Conflict,
        StoreError::Rejected { status, message } => TableStoreError::Rejected { status, message },
        StoreError::PermissionNotGranted => TableStoreError::PermissionNotGranted,
        StoreError::InstallationGone => TableStoreError::InstallationGone,
        StoreError::Repository(problem) => TableStoreError::Repository(problem),
        StoreError::RateLimited => TableStoreError::RateLimited,
        StoreError::Unavailable(message) => TableStoreError::Unavailable(message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::table::change::Change;
    use crate::github::models::TreeEntryRow;

    const STORED: &str = concat!(
        r#"{"version":1,"description":"Books","next_id":3,"columns":["#,
        r#"{"name":"title","type":"string","required":true},"#,
        r#"{"name":"status","type":"enum","values":["unread","read"]},"#,
        r#"{"name":"finished","type":"date"},"#,
        r#"{"name":"tags","type":"list"},"#,
        r#"{"name":"rating","type":"number"}]}"#,
        "\n",
        r#"{"id":1,"title":"Dune","status":"read","finished":"2026-01-02","tags":["scifi"],"rating":4.5}"#,
        "\n",
        r#"{"id":2,"title":"Emma"}"#,
        "\n",
    );

    fn entry(path: &str, kind: &str) -> TreeEntryRow {
        TreeEntryRow {
            path: path.into(),
            kind: kind.into(),
            size: Some(7),
        }
    }

    #[test]
    fn a_stored_table_round_trips_byte_for_byte() {
        let table = decode(STORED).unwrap();
        assert_eq!(table.rows().len(), 2);
        assert_eq!(table.next_id(), 3);
        assert_eq!(table.description(), Some("Books"));
        assert_eq!(encode(&table), STORED);
    }

    #[test]
    fn rows_are_written_in_schema_order_with_id_first() {
        let mut table = decode(STORED).unwrap();
        table
            .apply(&Change::Insert(vec![RawRecord::from([
                ("tags".to_string(), Raw::List(vec![])),
                ("title".to_string(), Raw::Text("Kim".into())),
            ])]))
            .unwrap();
        let encoded = encode(&table);
        assert!(encoded.ends_with("{\"id\":3,\"title\":\"Kim\",\"tags\":[]}\n"));
        assert!(encoded.starts_with("{\"version\":1,\"description\":\"Books\",\"next_id\":4,"));
    }

    #[test]
    fn damage_is_reported_by_line_without_content() {
        let cases = [
            ("", 1, "the header line is missing"),
            ("not json\n", 1, "not a valid table header"),
            (
                &STORED.replace(r#""version":1"#, r#""version":2"#),
                1,
                "format version 2",
            ),
            (&format!("{STORED}[1]\n"), 4, "must be a JSON object"),
            (
                &format!("{STORED}{{\"title\":\"x\"}}\n"),
                4,
                "positive integer id",
            ),
            (
                &STORED.replace(r#""title":"Emma""#, r#""title":7"#),
                3,
                "column title: expected a string",
            ),
            (
                &STORED.replace(r#""id":2"#, r#""id":1"#),
                3,
                "appears more than once",
            ),
        ];
        for (text, line, problem) in cases {
            match decode(text) {
                Err(TableStoreError::Damaged {
                    line: got,
                    problem: message,
                }) => {
                    assert_eq!(got, line, "{message}");
                    assert!(message.contains(problem), "{message}");
                    assert!(!message.contains("Emma"));
                }
                other => panic!("{text:?}: {other:?}"),
            }
        }
    }

    #[test]
    fn blank_lines_are_skipped_but_counted() {
        let text = STORED.replace("\n{\"id\":2", "\n\n{\"id\":\"two\"");
        match decode(&text) {
            Err(TableStoreError::Damaged { line, .. }) => assert_eq!(line, 4),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_listing_keeps_table_files_only() {
        let listing = listing_from_tree(TreeRow {
            truncated: false,
            tree: vec![
                entry("tables", "tree"),
                entry("tables/books.jsonl", "blob"),
                entry("tables/notes.md", "blob"),
                entry("tables/old/books.jsonl", "blob"),
                entry("tables/Bad Name.jsonl", "blob"),
                entry("files/x.jsonl", "blob"),
                entry("tablesx/y.jsonl", "blob"),
            ],
        });
        let names: Vec<&str> = listing.tables.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["books"]);
        assert_eq!(listing.tables[0].size, 7);
    }

    #[test]
    fn tables_live_below_the_root_folder() {
        let name = TableName::parse("reading-list").unwrap();
        assert_eq!(file_name(&name), "reading-list.jsonl");
        assert_eq!(repository_path(&name), "tables/reading-list.jsonl");
    }

    #[test]
    fn oversized_tables_are_not_written() {
        let mut table = decode(STORED).unwrap();
        let long = "x".repeat(9_000);
        let rows = (0..60)
            .map(|_| RawRecord::from([("title".to_string(), Raw::Text(long.clone()))]))
            .collect();
        table.apply(&Change::Insert(rows)).unwrap();
        assert!(matches!(
            encoded_content(&table),
            Err(TableStoreError::TooLarge)
        ));
    }
}
