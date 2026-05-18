use std::sync::Arc;

use bitcoin::io;
use lightning::util::persist::KVStoreSync;
use sea_orm::sea_query::OnConflict;

use crate::database::entities::{kv_store, prelude::KvStore};
use crate::runtime::block_on;
use sea_orm::{ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};

pub struct SeaOrmKvStore {
    connection: Arc<DatabaseConnection>,
}

impl SeaOrmKvStore {
    /// Caller must ensure migrations have already been run.
    pub fn from_connection(connection: Arc<DatabaseConnection>) -> Self {
        Self { connection }
    }

    fn get_connection(&self) -> &DatabaseConnection {
        &self.connection
    }
}

impl KVStoreSync for SeaOrmKvStore {
    fn read(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
    ) -> Result<Vec<u8>, io::Error> {
        tracing::trace!(primary_namespace, secondary_namespace, key, "KVStore read");

        let result = block_on(
            KvStore::find()
                .filter(kv_store::Column::PrimaryNamespace.eq(primary_namespace))
                .filter(kv_store::Column::SecondaryNamespace.eq(secondary_namespace))
                .filter(kv_store::Column::Key.eq(key))
                .one(self.get_connection()),
        )
        .map_err(|e| {
            tracing::error!(
                primary_namespace,
                secondary_namespace,
                key,
                error = %e,
                "KVStore read failed"
            );
            io::Error::new(io::ErrorKind::Other, format!("Database read failed: {e}"))
        })?;

        match result {
            Some(record) => Ok(record.value),
            None => {
                tracing::trace!(
                    primary_namespace,
                    secondary_namespace,
                    key,
                    "KVStore key not found"
                );
                Err(io::Error::new(io::ErrorKind::NotFound, "Key not found"))
            }
        }
    }

    fn write(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
        buf: Vec<u8>,
    ) -> Result<(), io::Error> {
        tracing::trace!(
            primary_namespace,
            secondary_namespace,
            key,
            value_len = buf.len(),
            "KVStore write"
        );

        let model = kv_store::ActiveModel {
            primary_namespace: ActiveValue::Set(primary_namespace.to_string()),
            secondary_namespace: ActiveValue::Set(secondary_namespace.to_string()),
            key: ActiveValue::Set(key.to_string()),
            value: ActiveValue::Set(buf),
        };

        block_on(
            KvStore::insert(model)
                .on_conflict(
                    OnConflict::columns([
                        kv_store::Column::PrimaryNamespace,
                        kv_store::Column::SecondaryNamespace,
                        kv_store::Column::Key,
                    ])
                    .update_column(kv_store::Column::Value)
                    .to_owned(),
                )
                .exec(self.get_connection()),
        )
        .map_err(|e| {
            tracing::error!(
                primary_namespace,
                secondary_namespace,
                key,
                error = %e,
                "KVStore write failed"
            );
            io::Error::new(io::ErrorKind::Other, format!("Database write failed: {e}"))
        })?;

        Ok(())
    }

    fn remove(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
        lazy: bool,
    ) -> Result<(), io::Error> {
        tracing::trace!(
            primary_namespace,
            secondary_namespace,
            key,
            lazy,
            "KVStore remove"
        );

        block_on(
            KvStore::delete_many()
                .filter(kv_store::Column::PrimaryNamespace.eq(primary_namespace))
                .filter(kv_store::Column::SecondaryNamespace.eq(secondary_namespace))
                .filter(kv_store::Column::Key.eq(key))
                .exec(self.get_connection()),
        )
        .map_err(|e| {
            tracing::error!(
                primary_namespace,
                secondary_namespace,
                key,
                error = %e,
                "KVStore remove failed"
            );
            io::Error::new(io::ErrorKind::Other, format!("Database delete failed: {e}"))
        })?;

        Ok(())
    }

    fn list(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
    ) -> Result<Vec<String>, io::Error> {
        tracing::trace!(primary_namespace, secondary_namespace, "KVStore list");

        let results = block_on(
            KvStore::find()
                .filter(kv_store::Column::PrimaryNamespace.eq(primary_namespace))
                .filter(kv_store::Column::SecondaryNamespace.eq(secondary_namespace))
                .all(self.get_connection()),
        )
        .map_err(|e| {
            tracing::error!(
                primary_namespace,
                secondary_namespace,
                error = %e,
                "KVStore list failed"
            );
            io::Error::new(io::ErrorKind::Other, format!("Database list failed: {e}"))
        })?;

        let keys: Vec<String> = results.into_iter().map(|r| r.key).collect();
        Ok(keys)
    }
}
