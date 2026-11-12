//! Transactional persistence adapter for the pure IPC state machine.
//!
//! Every namespace is an isolated aggregate row. A caller's closure runs after
//! `BEGIN IMMEDIATE` and before the row is atomically replaced, so a database
//! transaction can include IPC state and other SIC writes when sharing the same
//! `Store::begin()` connection. This adapter alone is not Core/CB publication
//! integration and does not perform OS scheduling or process control.

use crate::{
    error::{CoreError, Result},
    ipc::IpcState,
    storage::{self, Store},
};
use serde_json::Value;
use sqlx::{Row, Sqlite, Transaction};

#[derive(Clone)]
pub struct IpcStore {
    store: Store,
    namespace: String,
}

impl IpcStore {
    pub fn new(store: Store, namespace: impl Into<String>) -> Result<Self> {
        let namespace = namespace.into();
        if namespace.trim().is_empty() {
            return Err(CoreError::invalid("IPC namespace must be nonempty"));
        }
        Ok(Self { store, namespace })
    }

    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    /// Load one isolated state aggregate from a consistent SQLite transaction.
    pub async fn load(&self) -> Result<IpcState> {
        let mut tx = self.store.begin().await?;
        Self::ensure_schema(&mut tx).await?;
        let state = self.load_in_transaction(&mut tx).await?;
        tx.commit().await?;
        Ok(state)
    }

    /// Read a namespace within an already-authorized Core transaction without
    /// rewriting its aggregate row. Callers must commit before awaiting work.
    pub(crate) async fn read_in_transaction(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
    ) -> Result<IpcState> {
        Self::ensure_schema(tx).await?;
        self.load_in_transaction(tx).await
    }

    /// Execute a synchronous state-machine operation while holding one write
    /// transaction. Any closure error or serialization failure rolls it back.
    pub async fn transact<T, F>(&self, operation: F) -> Result<T>
    where
        F: FnOnce(&mut IpcState) -> Result<T>,
    {
        let mut tx = self.store.begin().await?;
        let result = self.transact_in_transaction(&mut tx, operation).await?;
        tx.commit().await?;
        Ok(result)
    }

    /// Apply an IPC state transition inside a caller-owned transaction. This
    /// is the composition point for atomic CB/cut + IPC receipt persistence.
    /// If this returns an error, the caller must abort its transaction.
    pub async fn transact_in_transaction<T, F>(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        operation: F,
    ) -> Result<T>
    where
        F: FnOnce(&mut IpcState) -> Result<T>,
    {
        Self::ensure_schema(tx).await?;
        let mut state = self.load_in_transaction(tx).await?;
        let result = operation(&mut state)?;
        let value = serde_json::to_value(&state)
            .map_err(|_| CoreError::invalid("IPC state cannot be serialized"))?;
        let encoded = storage::json_text(&value)?;
        let digest = storage::sha256(encoded.as_bytes());
        sqlx::query(
            "INSERT INTO sic_rust_ipc_states(namespace,state,digest) VALUES(?,?,?) \
             ON CONFLICT(namespace) DO UPDATE SET state=excluded.state,digest=excluded.digest",
        )
        .bind(&self.namespace)
        .bind(encoded)
        .bind(digest)
        .execute(&mut **tx)
        .await?;
        Ok(result)
    }

    async fn ensure_schema(tx: &mut Transaction<'_, Sqlite>) -> Result<()> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS sic_rust_ipc_states(\
             namespace TEXT PRIMARY KEY, state TEXT NOT NULL, digest TEXT NOT NULL)",
        )
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    async fn load_in_transaction(&self, tx: &mut Transaction<'_, Sqlite>) -> Result<IpcState> {
        let row = sqlx::query("SELECT state,digest FROM sic_rust_ipc_states WHERE namespace=?")
            .bind(&self.namespace)
            .fetch_optional(&mut **tx)
            .await?;
        let Some(row) = row else {
            return Ok(IpcState::new());
        };
        let encoded: String = row.try_get("state")?;
        let expected_digest: String = row.try_get("digest")?;
        if storage::sha256(encoded.as_bytes()) != expected_digest {
            return Err(CoreError::new(
                "integrity_error",
                "IPC state digest mismatch",
                500,
            ));
        }
        let value: Value = serde_json::from_str(&encoded)
            .map_err(|_| CoreError::new("integrity_error", "IPC state is invalid JSON", 500))?;
        serde_json::from_value(value)
            .map_err(|_| CoreError::new("integrity_error", "IPC state schema is invalid", 500))
    }
}
