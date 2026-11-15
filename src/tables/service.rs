//! Table use cases.

use std::sync::Arc;

use crate::domain::table::change::{Applied, Change};
use crate::domain::table::error::TableError;
use crate::domain::table::model::{Row, Table};
use crate::domain::table::names::TableName;
use crate::domain::table::query::RawQuery;
use crate::domain::table::schema::Schema;
use crate::domain::tenant::Tenant;
use crate::domain::version::Expected;
use crate::tables::store::{StoredTable, TableListing, TableStore, TableStoreError};

/// How often a change is tried when the table changes underneath it. One
/// retry covers a concurrent writer; more would hide a real fight.
const CHANGE_ATTEMPTS: usize = 2;

#[derive(Debug, thiserror::Error)]
pub enum TableServiceError {
    #[error(transparent)]
    Store(#[from] TableStoreError),
    #[error(transparent)]
    Rule(#[from] TableError),
}

impl TableServiceError {
    /// A stable, content-free name for logs.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Store(error) => error.kind(),
            Self::Rule(_) => "rule_violated",
        }
    }
}

/// One page of a query.
#[derive(Debug)]
pub struct QueryView {
    pub sha: String,
    pub schema: Schema,
    pub total_rows: usize,
    pub matched: usize,
    pub offset: usize,
    pub rows: Vec<Row>,
}

#[derive(Debug)]
pub struct ChangeOutcome {
    pub sha: String,
    pub applied: Applied,
    pub table: Table,
}

pub struct TableService {
    store: Arc<dyn TableStore>,
}

impl TableService {
    pub fn new(store: Arc<dyn TableStore>) -> Self {
        Self { store }
    }

    pub async fn list(&self, tenant: &Tenant) -> Result<TableListing, TableServiceError> {
        let mut listing = self.store.list(tenant).await?;
        listing.tables.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(listing)
    }

    /// Refuses to replace a table that exists.
    pub async fn create(
        &self,
        tenant: &Tenant,
        name: &TableName,
        schema: Schema,
        description: Option<String>,
    ) -> Result<StoredTable, TableServiceError> {
        let table = Table::new(schema, description);
        let sha = self
            .store
            .write(tenant, name, &table, &Expected::Absent)
            .await?;
        Ok(StoredTable { table, sha })
    }

    pub async fn describe(
        &self,
        tenant: &Tenant,
        name: &TableName,
    ) -> Result<StoredTable, TableServiceError> {
        Ok(self.store.read(tenant, name).await?)
    }

    /// The query is checked against the schema before rows are looked at.
    pub async fn query(
        &self,
        tenant: &Tenant,
        name: &TableName,
        query: &RawQuery,
    ) -> Result<QueryView, TableServiceError> {
        let stored = self.store.read(tenant, name).await?;
        let compiled = query.compile(stored.table.schema())?;
        let result = stored.table.query(&compiled);
        Ok(QueryView {
            matched: result.matched,
            rows: result.rows.into_iter().cloned().collect(),
            total_rows: stored.table.rows().len(),
            offset: compiled.offset,
            schema: stored.table.schema().clone(),
            sha: stored.sha,
        })
    }

    /// Changes are data, not a diff against a version, so re-applying one to
    /// a table someone else just wrote is safe: inserts get fresh ids, and
    /// updates and deletes find their rows again or fail.
    pub async fn change(
        &self,
        tenant: &Tenant,
        name: &TableName,
        change: &Change,
    ) -> Result<ChangeOutcome, TableServiceError> {
        change.check()?;
        let mut attempt = 1;
        loop {
            let StoredTable { mut table, sha } = self.store.read(tenant, name).await?;
            let applied = table.apply(change)?;
            let expected = Expected::Version(sha);
            match self.store.write(tenant, name, &table, &expected).await {
                Ok(sha) => {
                    return Ok(ChangeOutcome {
                        sha,
                        applied,
                        table,
                    });
                }
                Err(TableStoreError::Conflict) if attempt < CHANGE_ATTEMPTS => attempt += 1,
                Err(error) => return Err(error.into()),
            }
        }
    }

    pub async fn drop_table(
        &self,
        tenant: &Tenant,
        name: &TableName,
        sha: &str,
    ) -> Result<(), TableServiceError> {
        Ok(self.store.delete(tenant, name, sha).await?)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::domain::table::names::ColumnName;
    use crate::domain::table::schema::{Column, ColumnType};
    use crate::domain::table::value::{Raw, RawRecord};
    use crate::tables::store::BoxFut;

    /// An in-memory store whose table can be changed "by someone else"
    /// between a read and a write.
    struct MemoryStore {
        table: Mutex<(Table, String)>,
        changes_before_write: Mutex<Vec<RawRecord>>,
        writes: Mutex<usize>,
    }

    impl MemoryStore {
        fn new() -> Self {
            let schema = Schema::new(vec![
                Column::new(
                    ColumnName::parse("title").unwrap(),
                    ColumnType::String,
                    true,
                    None,
                )
                .unwrap(),
            ])
            .unwrap();
            Self {
                table: Mutex::new((Table::new(schema, None), "v1".into())),
                changes_before_write: Mutex::new(Vec::new()),
                writes: Mutex::new(0),
            }
        }

        fn titles(&self) -> usize {
            self.table.lock().unwrap().0.rows().len()
        }
    }

    impl TableStore for MemoryStore {
        fn list<'a>(&'a self, _: &'a Tenant) -> BoxFut<'a, TableListing> {
            Box::pin(async {
                Ok(TableListing {
                    tables: ["b", "a"]
                        .into_iter()
                        .map(|name| crate::tables::store::TableEntry {
                            name: TableName::parse(name).unwrap(),
                            size: 1,
                        })
                        .collect(),
                    truncated: false,
                })
            })
        }

        fn read<'a>(&'a self, _: &'a Tenant, _: &'a TableName) -> BoxFut<'a, StoredTable> {
            Box::pin(async {
                let (table, sha) = self.table.lock().unwrap().clone();
                Ok(StoredTable { table, sha })
            })
        }

        fn write<'a>(
            &'a self,
            _: &'a Tenant,
            _: &'a TableName,
            table: &'a Table,
            expected: &'a Expected,
        ) -> BoxFut<'a, String> {
            Box::pin(async move {
                *self.writes.lock().unwrap() += 1;
                let mut stored = self.table.lock().unwrap();
                if let Some(row) = self.changes_before_write.lock().unwrap().pop() {
                    stored.0.apply(&Change::Insert(vec![row])).unwrap();
                    stored.1 = format!("{}x", stored.1);
                }
                match expected {
                    Expected::Absent => return Err(TableStoreError::AlreadyExists),
                    Expected::Version(sha) if *sha != stored.1 => {
                        return Err(TableStoreError::Conflict);
                    }
                    Expected::Version(_) => {}
                }
                *stored = (table.clone(), format!("{}+", stored.1));
                Ok(stored.1.clone())
            })
        }

        fn delete<'a>(&'a self, _: &'a Tenant, _: &'a TableName, _: &'a str) -> BoxFut<'a, ()> {
            Box::pin(async { Ok(()) })
        }
    }

    fn tenant() -> Tenant {
        Tenant {
            user_id: 1,
            login: "alice".into(),
            installation_id: 1,
        }
    }

    fn name() -> TableName {
        TableName::parse("books").unwrap()
    }

    fn row(title: &str) -> RawRecord {
        RawRecord::from([("title".to_string(), Raw::Text(title.into()))])
    }

    #[tokio::test]
    async fn a_change_writes_against_the_version_it_read() {
        let store = Arc::new(MemoryStore::new());
        let service = TableService::new(store.clone());

        let outcome = service
            .change(&tenant(), &name(), &Change::Insert(vec![row("Dune")]))
            .await
            .unwrap();

        assert_eq!(outcome.sha, "v1+");
        assert_eq!(outcome.applied.inserted, [1]);
        assert_eq!(*store.writes.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn a_change_is_retried_once_and_keeps_the_concurrent_row() {
        let store = Arc::new(MemoryStore::new());
        store.changes_before_write.lock().unwrap().push(row("Emma"));
        let service = TableService::new(store.clone());

        let outcome = service
            .change(&tenant(), &name(), &Change::Insert(vec![row("Dune")]))
            .await
            .unwrap();

        assert_eq!(outcome.applied.inserted, [2], "the retry gets a fresh id");
        assert_eq!(store.titles(), 2);
        assert_eq!(*store.writes.lock().unwrap(), 2);
    }

    #[tokio::test]
    async fn a_second_conflict_is_reported() {
        let store = Arc::new(MemoryStore::new());
        store
            .changes_before_write
            .lock()
            .unwrap()
            .extend([row("Emma"), row("Kim")]);
        let service = TableService::new(store.clone());

        let err = service
            .change(&tenant(), &name(), &Change::Insert(vec![row("Dune")]))
            .await
            .unwrap_err();

        assert!(matches!(
            err,
            TableServiceError::Store(TableStoreError::Conflict)
        ));
        assert_eq!(*store.writes.lock().unwrap(), 2);
    }

    #[tokio::test]
    async fn a_useless_change_is_refused_before_reading() {
        let store = Arc::new(MemoryStore::new());
        let service = TableService::new(store.clone());

        let err = service
            .change(&tenant(), &name(), &Change::Insert(vec![]))
            .await
            .unwrap_err();

        assert!(matches!(
            err,
            TableServiceError::Rule(TableError::EmptyBatch)
        ));
        assert_eq!(*store.writes.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn a_rejected_change_writes_nothing() {
        let store = Arc::new(MemoryStore::new());
        let service = TableService::new(store.clone());

        let err = service
            .change(&tenant(), &name(), &Change::Insert(vec![RawRecord::new()]))
            .await
            .unwrap_err();

        assert!(matches!(err, TableServiceError::Rule(_)));
        assert_eq!(*store.writes.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn listings_are_sorted_by_name() {
        let service = TableService::new(Arc::new(MemoryStore::new()));
        let listing = service.list(&tenant()).await.unwrap();
        let names: Vec<&str> = listing.tables.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["a", "b"]);
    }
}
