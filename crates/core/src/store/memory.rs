//! [`MemoryStore`], the in-memory [`Store`] the job-loop and scheduler tests use.
//!
//! A transaction buffers its writes and applies them at commit, after checking that every row
//! it wrote is still at the version it started from. Each method does its work when its future
//! is first polled, and the lock on the committed state is taken and released within that poll.

use std::any::{Any, TypeId};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use super::{RowVersion, Store, StoreError, Stored, Transaction, Versioned};

/// The rows of one record type, by key.
type Table<R> = BTreeMap<<R as Versioned>::Key, Stored<R>>;

/// The committed tables, each a [`Table`] of the record type whose `TypeId` keys it.
#[derive(Default)]
struct Tables(HashMap<TypeId, Box<dyn Any + Send + Sync>>);

impl Tables {
    fn table<R: Versioned>(&self) -> Option<&Table<R>> {
        self.0.get(&TypeId::of::<R>()).map(|table| {
            table
                .downcast_ref::<Table<R>>()
                .expect("a table is keyed by the TypeId of its record type")
        })
    }

    fn table_mut<R: Versioned>(&mut self) -> &mut Table<R> {
        self.0
            .entry(TypeId::of::<R>())
            .or_insert_with(|| Box::new(Table::<R>::new()))
            .downcast_mut::<Table<R>>()
            .expect("a table is keyed by the TypeId of its record type")
    }

    fn version<R: Versioned>(&self, key: &R::Key) -> Option<RowVersion> {
        self.table::<R>()
            .and_then(|table| table.get(key))
            .map(|stored| stored.version)
    }
}

/// A row a transaction wrote.
struct Write<R> {
    /// The committed version the transaction's first write of the key started from: `None`
    /// for an insert, the expected version for an update.
    base: Option<RowVersion>,
    stored: Stored<R>,
}

/// The rows a transaction wrote in one table, by key.
struct Writes<R: Versioned>(BTreeMap<R::Key, Write<R>>);

/// A table's buffered writes, with the record type erased.
trait Pending: Send {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    /// Whether every written row is still at its base version in `tables`.
    fn holds(&self, tables: &Tables) -> bool;
    /// Applies the writes to `tables`.
    fn apply(self: Box<Self>, tables: &mut Tables);
}

impl<R: Versioned> Pending for Writes<R> {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn holds(&self, tables: &Tables) -> bool {
        self.0
            .iter()
            .all(|(key, write)| tables.version::<R>(key) == write.base)
    }

    fn apply(self: Box<Self>, tables: &mut Tables) {
        let table = tables.table_mut::<R>();
        for (key, write) in self.0 {
            table.insert(key, write.stored);
        }
    }
}

/// A [`Store`] kept in memory. Clones share the same committed state.
#[derive(Clone, Default)]
pub struct MemoryStore {
    tables: Arc<Mutex<Tables>>,
}

impl MemoryStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

impl std::fmt::Debug for MemoryStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryStore").finish_non_exhaustive()
    }
}

impl Store for MemoryStore {
    type Transaction = MemoryTransaction;

    async fn begin(&self) -> Result<MemoryTransaction, StoreError> {
        Ok(MemoryTransaction {
            tables: Arc::clone(&self.tables),
            writes: HashMap::new(),
        })
    }
}

/// A transaction of a [`MemoryStore`]: its writes are buffered until [`commit`](Self::commit).
pub struct MemoryTransaction {
    tables: Arc<Mutex<Tables>>,
    writes: HashMap<TypeId, Box<dyn Pending>>,
}

impl std::fmt::Debug for MemoryTransaction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryTransaction")
            .field("tables_written", &self.writes.len())
            .finish_non_exhaustive()
    }
}

impl MemoryTransaction {
    fn tables(&self) -> MutexGuard<'_, Tables> {
        // Every change is made in full before the guard drops, so a poisoned state is whole.
        self.tables.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn writes<R: Versioned>(&self) -> Option<&Writes<R>> {
        self.writes.get(&TypeId::of::<R>()).map(|writes| {
            writes
                .as_any()
                .downcast_ref::<Writes<R>>()
                .expect("buffered writes are keyed by the TypeId of their record type")
        })
    }

    fn writes_mut<R: Versioned>(&mut self) -> &mut Writes<R> {
        self.writes
            .entry(TypeId::of::<R>())
            .or_insert_with(|| Box::new(Writes::<R>(BTreeMap::new())))
            .as_any_mut()
            .downcast_mut::<Writes<R>>()
            .expect("buffered writes are keyed by the TypeId of their record type")
    }

    /// The row with `key` as this transaction sees it: its own write, or else the committed row.
    fn current<R: Versioned>(&self, key: &R::Key) -> Option<Stored<R>> {
        if let Some(write) = self.writes::<R>().and_then(|writes| writes.0.get(key)) {
            return Some(write.stored.clone());
        }
        self.tables()
            .table::<R>()
            .and_then(|table| table.get(key))
            .cloned()
    }

    fn insert_now<R: Versioned>(&mut self, record: &R) -> Result<RowVersion, StoreError> {
        let key = record.key();
        if self.current::<R>(&key).is_some() {
            return Err(StoreError::Conflict);
        }
        let stored = Stored {
            record: record.clone(),
            version: RowVersion::FIRST,
        };
        self.writes_mut::<R>()
            .0
            .insert(key, Write { base: None, stored });
        Ok(RowVersion::FIRST)
    }

    fn update_now<R: Versioned>(
        &mut self,
        record: &R,
        expected: RowVersion,
    ) -> Result<RowVersion, StoreError> {
        let key = record.key();
        let current = self.current::<R>(&key).ok_or(StoreError::NotFound)?;
        if current.version != expected {
            return Err(StoreError::Conflict);
        }
        let version = expected.next();
        let stored = Stored {
            record: record.clone(),
            version,
        };
        let writes = self.writes_mut::<R>();
        match writes.0.get_mut(&key) {
            Some(write) => write.stored = stored,
            None => {
                writes.0.insert(
                    key,
                    Write {
                        base: Some(expected),
                        stored,
                    },
                );
            }
        }
        Ok(version)
    }

    fn commit_now(self) -> Result<(), StoreError> {
        let mut tables = self.tables.lock().unwrap_or_else(PoisonError::into_inner);
        if !self.writes.values().all(|writes| writes.holds(&tables)) {
            return Err(StoreError::Conflict);
        }
        for writes in self.writes.into_values() {
            writes.apply(&mut tables);
        }
        Ok(())
    }
}

impl Transaction for MemoryTransaction {
    async fn get<R: Versioned>(&mut self, key: &R::Key) -> Result<Option<Stored<R>>, StoreError> {
        Ok(self.current(key))
    }

    async fn insert<R: Versioned>(&mut self, record: &R) -> Result<RowVersion, StoreError> {
        self.insert_now(record)
    }

    async fn update<R: Versioned>(
        &mut self,
        record: &R,
        expected: RowVersion,
    ) -> Result<RowVersion, StoreError> {
        self.update_now(record, expected)
    }

    async fn commit(self) -> Result<(), StoreError> {
        self.commit_now()
    }

    async fn rollback(self) -> Result<(), StoreError> {
        Ok(())
    }
}
