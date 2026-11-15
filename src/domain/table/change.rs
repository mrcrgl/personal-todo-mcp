//! Changes to a table. A change is data, so the service can re-apply it to a
//! fresher copy when someone else wrote the table in the meantime. Applying is
//! all or nothing.

use std::collections::HashSet;

use crate::domain::table::error::{MAX_BATCH, TableError, Target};
use crate::domain::table::model::{Row, Table, record_values};
use crate::domain::table::names::ColumnName;
use crate::domain::table::query::{Filter, RawCondition};
use crate::domain::table::schema::{Column, Schema};
use crate::domain::table::value::{Raw, RawRecord};
use crate::domain::todo::ValidationError;

/// A column to add. `default` fills the rows that already exist.
#[derive(Debug, Clone, PartialEq)]
pub struct NewColumn {
    pub column: Column,
    pub default: Option<Raw>,
}

/// Removals apply first, so a column can be replaced in one call.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Alteration {
    pub add: Vec<NewColumn>,
    pub remove: Vec<ColumnName>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    Insert(Vec<RawRecord>),
    /// Per row id, the cells to change. `null` empties a cell.
    Update(Vec<(u64, RawRecord)>),
    /// The same cells for every row that matches.
    UpdateWhere {
        conditions: Vec<RawCondition>,
        set: RawRecord,
    },
    Delete(Vec<u64>),
    DeleteWhere(Vec<RawCondition>),
    Alter(Alteration),
}

/// What a change did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Applied {
    /// Ids of inserted rows, in the order given.
    pub inserted: Vec<u64>,
    /// Rows updated or deleted.
    pub affected: usize,
}

impl Change {
    /// Checks what can be checked without the table, so a useless call fails
    /// before anything is read.
    pub fn check(&self) -> Result<(), TableError> {
        let batch = match self {
            Self::Insert(rows) => rows.len(),
            Self::Update(rows) => rows.len(),
            Self::Delete(ids) => ids.len(),
            Self::UpdateWhere { conditions, set } => {
                require_conditions(conditions)?;
                if set.is_empty() {
                    return Err(TableError::NoChange);
                }
                1
            }
            Self::DeleteWhere(conditions) => {
                require_conditions(conditions)?;
                1
            }
            Self::Alter(alteration) => {
                if alteration.add.is_empty() && alteration.remove.is_empty() {
                    return Err(TableError::NoChange);
                }
                1
            }
        };
        if batch == 0 {
            return Err(TableError::EmptyBatch);
        }
        if batch > MAX_BATCH {
            return Err(TableError::BatchTooLarge);
        }
        if let Self::Update(rows) = self
            && let Some((index, _)) = rows
                .iter()
                .enumerate()
                .find(|(_, (_, record))| record.is_empty())
        {
            return Err(TableError::Invalid(ValidationError(format!(
                "item {}: give at least one cell to change",
                index + 1
            ))));
        }
        Ok(())
    }
}

/// Changing or deleting every row has to be asked for explicitly.
fn require_conditions(conditions: &[RawCondition]) -> Result<(), TableError> {
    if conditions.is_empty() {
        return Err(TableError::BadCondition {
            index: 1,
            problem: "give at least one condition, e.g. id not_null to match every row".into(),
        });
    }
    Ok(())
}

impl Table {
    pub fn apply(&mut self, change: &Change) -> Result<Applied, TableError> {
        change.check()?;
        let mut next = self.clone();
        let applied = match change {
            Change::Insert(records) => next.insert(records)?,
            Change::Update(changes) => next.update(changes)?,
            Change::UpdateWhere { conditions, set } => {
                let filter = Filter::compile(&next.schema, conditions)?;
                next.update_where(&filter, set)?
            }
            Change::Delete(ids) => next.delete(ids)?,
            Change::DeleteWhere(conditions) => {
                let filter = Filter::compile(&next.schema, conditions)?;
                next.delete_where(&filter)
            }
            Change::Alter(alteration) => next.alter(alteration)?,
        };
        *self = next;
        Ok(applied)
    }

    fn insert(&mut self, records: &[RawRecord]) -> Result<Applied, TableError> {
        let mut inserted = Vec::with_capacity(records.len());
        for (index, record) in records.iter().enumerate() {
            let values = record_values(&self.schema, Target::Item(index + 1), record, None)?;
            let id = self.next_id;
            self.next_id += 1;
            self.rows.push(Row { id, values });
            inserted.push(id);
        }
        Ok(Applied {
            inserted,
            affected: 0,
        })
    }

    fn update(&mut self, changes: &[(u64, RawRecord)]) -> Result<Applied, TableError> {
        for (id, record) in changes {
            let row = self
                .rows
                .iter_mut()
                .find(|row| row.id == *id)
                .ok_or(TableError::NoSuchRow(*id))?;
            row.values = record_values(&self.schema, Target::Row(*id), record, Some(&row.values))?;
        }
        let distinct: HashSet<u64> = changes.iter().map(|(id, _)| *id).collect();
        Ok(Applied {
            inserted: Vec::new(),
            affected: distinct.len(),
        })
    }

    fn update_where(&mut self, filter: &Filter, set: &RawRecord) -> Result<Applied, TableError> {
        let mut affected = 0;
        for row in self.rows.iter_mut().filter(|row| filter.matches(row)) {
            row.values = record_values(&self.schema, Target::Row(row.id), set, Some(&row.values))?;
            affected += 1;
        }
        Ok(Applied {
            inserted: Vec::new(),
            affected,
        })
    }

    fn delete(&mut self, ids: &[u64]) -> Result<Applied, TableError> {
        let wanted: HashSet<u64> = ids.iter().copied().collect();
        if let Some(missing) = ids
            .iter()
            .find(|id| !self.rows.iter().any(|row| row.id == **id))
        {
            return Err(TableError::NoSuchRow(*missing));
        }
        self.rows.retain(|row| !wanted.contains(&row.id));
        Ok(Applied {
            inserted: Vec::new(),
            affected: wanted.len(),
        })
    }

    fn delete_where(&mut self, filter: &Filter) -> Applied {
        let before = self.rows.len();
        self.rows.retain(|row| !filter.matches(row));
        Applied {
            inserted: Vec::new(),
            affected: before - self.rows.len(),
        }
    }

    fn alter(&mut self, alteration: &Alteration) -> Result<Applied, TableError> {
        let mut columns: Vec<Column> = self.schema.columns().to_vec();
        for name in &alteration.remove {
            let before = columns.len();
            columns.retain(|column| column.name != *name);
            if columns.len() == before {
                return Err(TableError::NoSuchColumn(name.as_str().to_string()));
            }
            for row in &mut self.rows {
                row.values.remove(name);
            }
        }
        for new in &alteration.add {
            let name = new.column.name.as_str().to_string();
            if columns.iter().any(|column| column.name == new.column.name) {
                return Err(TableError::ColumnExists(name));
            }
            let default = match &new.default {
                None => None,
                Some(raw) => {
                    new.column
                        .kind
                        .coerce(raw)
                        .map_err(|problem| TableError::BadDefault {
                            column: name.clone(),
                            problem,
                        })?
                }
            };
            match default {
                Some(value) => {
                    for row in &mut self.rows {
                        row.values.insert(new.column.name.clone(), value.clone());
                    }
                }
                None if new.column.required && !self.rows.is_empty() => {
                    return Err(TableError::DefaultNeeded(name));
                }
                None => {}
            }
            columns.push(new.column.clone());
        }
        self.schema = Schema::new(columns)?;
        Ok(Applied::default())
    }
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;
    use crate::domain::table::model::StoredRow;
    use crate::domain::table::names::Field;
    use crate::domain::table::query::{Op, RawQuery, SortKey};
    use crate::domain::table::schema::ColumnType;
    use crate::domain::table::value::Value;

    fn column(name: &str, kind: ColumnType, required: bool) -> Column {
        Column::new(ColumnName::parse(name).unwrap(), kind, required, None).unwrap()
    }

    fn books() -> Table {
        Table::new(
            Schema::new(vec![
                column("title", ColumnType::String, true),
                column(
                    "status",
                    ColumnType::Enum(vec!["unread".into(), "read".into()]),
                    false,
                ),
                column("pages", ColumnType::Integer, false),
                column("finished", ColumnType::Date, false),
                column("tags", ColumnType::List, false),
            ])
            .unwrap(),
            None,
        )
    }

    fn record(cells: &[(&str, Raw)]) -> RawRecord {
        cells
            .iter()
            .map(|(name, raw)| (name.to_string(), raw.clone()))
            .collect()
    }

    fn text(value: &str) -> Raw {
        Raw::Text(value.into())
    }

    fn field(name: &str) -> Field {
        Field::parse(name).unwrap()
    }

    fn condition(name: &str, op: Op, value: Option<Raw>) -> RawCondition {
        RawCondition {
            field: field(name),
            op,
            value,
        }
    }

    fn stocked() -> Table {
        let mut table = books();
        table
            .apply(&Change::Insert(vec![
                record(&[
                    ("title", text("Dune")),
                    ("status", text("read")),
                    ("pages", Raw::Integer(600)),
                    ("tags", Raw::List(vec![text("scifi")])),
                ]),
                record(&[("title", text("Emma")), ("pages", Raw::Integer(400))]),
                record(&[
                    ("title", text("Ubik")),
                    ("status", text("unread")),
                    ("pages", Raw::Integer(200)),
                    ("tags", Raw::List(vec![text("SciFi"), text("short")])),
                ]),
            ]))
            .unwrap();
        table
    }

    fn ids(table: &Table, query: RawQuery) -> Vec<u64> {
        let query = query.compile(table.schema()).unwrap();
        table.query(&query).rows.iter().map(|row| row.id).collect()
    }

    fn query(conditions: Vec<RawCondition>, sort: Vec<SortKey>) -> RawQuery {
        RawQuery {
            conditions,
            sort,
            offset: 0,
            limit: 100,
        }
    }

    fn titles(table: &Table) -> Vec<String> {
        let title = ColumnName::parse("title").unwrap();
        table
            .rows()
            .iter()
            .map(|row| match &row.values[&title] {
                Value::Text(text) => text.clone(),
                other => panic!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn inserts_get_ascending_ids() {
        let mut table = stocked();
        let applied = table
            .apply(&Change::Insert(vec![record(&[("title", text("Kim"))])]))
            .unwrap();
        assert_eq!(applied.inserted, [4]);
        assert_eq!(table.next_id(), 5);
    }

    #[test]
    fn a_bad_row_rejects_the_whole_batch() {
        let mut table = stocked();
        let err = table
            .apply(&Change::Insert(vec![
                record(&[("title", text("Ok"))]),
                record(&[("title", text("Bad")), ("pages", text("many"))]),
            ]))
            .unwrap_err();
        assert_eq!(err.to_string(), "item 2, column pages: expected an integer");
        assert_eq!(table.rows().len(), 3);
        assert_eq!(table.next_id(), 4);
    }

    #[test]
    fn rows_are_checked_against_the_schema() {
        let mut table = books();
        let cases = [
            (record(&[]), "item 1: column title is required"),
            (
                record(&[("title", text("a")), ("author", text("b"))]),
                "item 1: unknown column author",
            ),
            (
                record(&[("title", text("a")), ("id", Raw::Integer(9))]),
                "item 1: id is assigned by the server",
            ),
            (
                record(&[("title", text("a")), ("status", text("lost"))]),
                "item 1, column status: not one of the column's values",
            ),
        ];
        for (row, message) in cases {
            let err = table.apply(&Change::Insert(vec![row])).unwrap_err();
            assert!(err.to_string().starts_with(message), "{err}");
        }
    }

    #[test]
    fn updates_merge_cells_and_null_empties_one() {
        let mut table = stocked();
        let applied = table
            .apply(&Change::Update(vec![(
                1,
                record(&[("status", Raw::Null), ("pages", Raw::Integer(612))]),
            )]))
            .unwrap();
        assert_eq!(applied.affected, 1);
        let row = &table.rows()[0];
        assert_eq!(
            row.values[&ColumnName::parse("pages").unwrap()],
            Value::Integer(612)
        );
        assert!(
            !row.values
                .contains_key(&ColumnName::parse("status").unwrap())
        );

        let err = table
            .apply(&Change::Update(vec![(1, record(&[("title", Raw::Null)]))]))
            .unwrap_err();
        assert_eq!(err.to_string(), "row 1: column title is required");
        assert_eq!(
            table
                .apply(&Change::Update(vec![(
                    9,
                    record(&[("pages", Raw::Integer(1))])
                )]))
                .unwrap_err(),
            TableError::NoSuchRow(9)
        );
    }

    #[test]
    fn update_where_changes_every_matching_row() {
        let mut table = stocked();
        let applied = table
            .apply(&Change::UpdateWhere {
                conditions: vec![condition("status", Op::Ne, Some(text("read")))],
                set: record(&[("status", text("read")), ("finished", text("2026-11-14"))]),
            })
            .unwrap();
        assert_eq!(applied.affected, 2);
        let finished = ColumnName::parse("finished").unwrap();
        assert!(table.rows()[1..].iter().all(|row| row.values[&finished]
            == Value::Date(NaiveDate::from_ymd_opt(2026, 11, 14).unwrap())));
    }

    #[test]
    fn deletes_by_id_or_condition() {
        let mut table = stocked();
        assert_eq!(
            table.apply(&Change::Delete(vec![2, 7])).unwrap_err(),
            TableError::NoSuchRow(7)
        );
        assert_eq!(table.apply(&Change::Delete(vec![2])).unwrap().affected, 1);
        let applied = table
            .apply(&Change::DeleteWhere(vec![condition(
                "pages",
                Op::Lt,
                Some(Raw::Integer(300)),
            )]))
            .unwrap();
        assert_eq!(applied.affected, 1);
        assert_eq!(titles(&table), ["Dune"]);
    }

    #[test]
    fn changing_or_deleting_everything_needs_a_condition() {
        let mut table = stocked();
        assert!(table.apply(&Change::DeleteWhere(vec![])).is_err());
        assert!(
            table
                .apply(&Change::UpdateWhere {
                    conditions: vec![],
                    set: record(&[("pages", Raw::Integer(1))]),
                })
                .is_err()
        );
        let all = table
            .apply(&Change::DeleteWhere(vec![condition(
                "id",
                Op::NotNull,
                None,
            )]))
            .unwrap();
        assert_eq!(all.affected, 3);
    }

    #[test]
    fn batches_are_limited() {
        let mut table = books();
        assert_eq!(
            table.apply(&Change::Insert(vec![])).unwrap_err(),
            TableError::EmptyBatch
        );
        let rows = vec![record(&[("title", text("x"))]); MAX_BATCH + 1];
        assert_eq!(
            table.apply(&Change::Insert(rows)).unwrap_err(),
            TableError::BatchTooLarge
        );
    }

    #[test]
    fn filters_compare_typed_values() {
        let table = stocked();
        assert_eq!(
            ids(
                &table,
                query(
                    vec![condition("pages", Op::Gte, Some(Raw::Integer(400)))],
                    vec![]
                )
            ),
            [1, 2]
        );
        assert_eq!(
            ids(
                &table,
                query(
                    vec![condition("tags", Op::Contains, Some(text("scifi")))],
                    vec![]
                )
            ),
            [1, 3]
        );
        assert_eq!(
            ids(
                &table,
                query(
                    vec![condition("title", Op::Contains, Some(text("UB")))],
                    vec![]
                )
            ),
            [3]
        );
        assert_eq!(
            ids(
                &table,
                query(
                    vec![condition(
                        "status",
                        Op::In,
                        Some(Raw::List(vec![text("read"), text("unread")]))
                    )],
                    vec![]
                )
            ),
            [1, 3]
        );
        assert_eq!(
            ids(
                &table,
                query(vec![condition("status", Op::IsNull, None)], vec![])
            ),
            [2]
        );
        assert_eq!(
            ids(
                &table,
                query(vec![condition("id", Op::Gt, Some(Raw::Integer(1)))], vec![])
            ),
            [2, 3]
        );
    }

    #[test]
    fn filters_that_cannot_match_are_refused() {
        let schema = books().schema().clone();
        for (conditions, message) in [
            (
                vec![condition("author", Op::Eq, Some(text("x")))],
                "condition 1: unknown column author",
            ),
            (
                vec![condition("pages", Op::Eq, Some(text("many")))],
                "condition 1: the value does not fit the column",
            ),
            (
                vec![condition("tags", Op::Lt, Some(text("a")))],
                "condition 1: list columns cannot be compared by size",
            ),
            (
                vec![condition("pages", Op::Contains, Some(text("1")))],
                "condition 1: contains works on",
            ),
            (
                vec![condition("pages", Op::Eq, None)],
                "condition 1: this operator needs a value",
            ),
        ] {
            let err = Filter::compile(&schema, &conditions).unwrap_err();
            assert!(err.to_string().starts_with(message), "{err}");
        }
    }

    #[test]
    fn sorting_puts_empty_cells_last_and_pages() {
        let table = stocked();
        let by_status = |descending| {
            query(
                vec![],
                vec![SortKey {
                    field: field("status"),
                    descending,
                }],
            )
        };
        assert_eq!(ids(&table, by_status(false)), [1, 3, 2]);
        assert_eq!(ids(&table, by_status(true)), [3, 1, 2]);

        let mut paged = query(
            vec![],
            vec![SortKey {
                field: field("pages"),
                descending: true,
            }],
        );
        paged.offset = 1;
        paged.limit = 1;
        let compiled = paged.compile(table.schema()).unwrap();
        let result = table.query(&compiled);
        assert_eq!(result.matched, 3);
        assert_eq!(result.rows.iter().map(|r| r.id).collect::<Vec<_>>(), [2]);
    }

    #[test]
    fn alter_adds_with_defaults_and_removes() {
        let mut table = stocked();
        let rating = column("rating", ColumnType::Integer, true);
        assert_eq!(
            table
                .apply(&Change::Alter(Alteration {
                    add: vec![NewColumn {
                        column: rating.clone(),
                        default: None,
                    }],
                    remove: vec![],
                }))
                .unwrap_err(),
            TableError::DefaultNeeded("rating".into())
        );
        table
            .apply(&Change::Alter(Alteration {
                add: vec![NewColumn {
                    column: rating,
                    default: Some(Raw::Integer(3)),
                }],
                remove: vec![ColumnName::parse("tags").unwrap()],
            }))
            .unwrap();
        let names: Vec<&str> = table
            .schema()
            .columns()
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(names, ["title", "status", "pages", "finished", "rating"]);
        let tags = ColumnName::parse("tags").unwrap();
        assert!(
            table
                .rows()
                .iter()
                .all(|row| !row.values.contains_key(&tags)
                    && row.values[&ColumnName::parse("rating").unwrap()] == Value::Integer(3))
        );
    }

    #[test]
    fn alter_can_replace_a_column_but_not_duplicate_one() {
        let mut table = stocked();
        table
            .apply(&Change::Alter(Alteration {
                add: vec![NewColumn {
                    column: column("pages", ColumnType::Number, false),
                    default: None,
                }],
                remove: vec![ColumnName::parse("pages").unwrap()],
            }))
            .unwrap();
        assert!(
            table
                .apply(&Change::Alter(Alteration {
                    add: vec![NewColumn {
                        column: column("title", ColumnType::String, false),
                        default: None,
                    }],
                    remove: vec![],
                }))
                .is_err()
        );
        assert!(
            table
                .apply(&Change::Alter(Alteration {
                    add: vec![],
                    remove: vec![ColumnName::parse("nope").unwrap()],
                }))
                .is_err()
        );
    }

    #[test]
    fn restore_checks_rows_and_keeps_ids_unique() {
        let schema = books().schema().clone();
        let row = |id, title: &str| StoredRow {
            id,
            record: record(&[("title", text(title))]),
        };
        let table =
            Table::restore(schema.clone(), None, 2, vec![row(1, "a"), row(5, "b")]).unwrap();
        assert_eq!(table.next_id(), 6, "never reuses a hand-added id");

        assert!(matches!(
            Table::restore(schema.clone(), None, 9, vec![row(1, "a"), row(1, "b")]),
            Err(crate::domain::table::model::RestoreError::DuplicateId { index: 2, id: 1 })
        ));
        let bad = StoredRow {
            id: 3,
            record: record(&[("title", Raw::Integer(1))]),
        };
        let err = Table::restore(schema, None, 9, vec![bad]).unwrap_err();
        assert_eq!(err.index(), 1);
        assert!(err.to_string().starts_with("row 3, column title"), "{err}");
    }
}
