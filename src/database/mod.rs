pub(crate) mod entities;

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::str::FromStr;

use bitcoin::secp256k1::PublicKey;
use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, TransactionTrait,
};

use crate::database::entities::{
    channel_peer, config, mnemonic,
    prelude::{ChannelPeer, Config, Mnemonic, RevokedToken},
    revoked_token,
};
use crate::error::APIError;
use crate::runtime::block_on;

pub struct RlnDatabase {
    connection: DatabaseConnection,
}

impl RlnDatabase {
    pub fn new(connection: DatabaseConnection) -> Self {
        Self { connection }
    }

    pub fn get_connection(&self) -> &DatabaseConnection {
        &self.connection
    }

    pub fn add_revoked_tokens(&self, token_id_hexes: Vec<String>) -> Result<(), APIError> {
        let now = crate::utils::get_current_timestamp() as i64;

        block_on(
            self.connection
                .transaction::<_, (), sea_orm::DbErr>(move |txn| {
                    Box::pin(async move {
                        for hex in token_id_hexes {
                            let token = revoked_token::ActiveModel {
                                token_id: ActiveValue::Set(hex),
                                revoked_at: ActiveValue::Set(now),
                            };
                            RevokedToken::insert(token)
                                .on_conflict(
                                    OnConflict::column(revoked_token::Column::TokenId)
                                        .do_nothing()
                                        .to_owned(),
                                )
                                .exec(txn)
                                .await?;
                        }
                        Ok(())
                    })
                }),
        )
        .map_err(|e| match e {
            sea_orm::TransactionError::Connection(err)
            | sea_orm::TransactionError::Transaction(err) => APIError::from(err),
        })?;

        Ok(())
    }

    pub fn delete_channel_peer(&self, pubkey: &str) -> Result<(), APIError> {
        block_on(
            ChannelPeer::delete_many()
                .filter(channel_peer::Column::Pubkey.eq(pubkey))
                .exec(self.get_connection()),
        )?;

        Ok(())
    }

    pub fn get_config(&self, key: &str) -> Result<Option<String>, APIError> {
        let result = block_on(
            Config::find()
                .filter(config::Column::Key.eq(key))
                .one(self.get_connection()),
        )?;

        Ok(result.map(|r| r.value))
    }

    pub fn get_mnemonic(&self) -> Result<Option<mnemonic::Model>, APIError> {
        Ok(block_on(
            Mnemonic::find_by_id(1).one(self.get_connection()),
        )?)
    }

    pub fn load_revoked_tokens(&self) -> Result<HashSet<Vec<u8>>, APIError> {
        let results = block_on(RevokedToken::find().all(self.get_connection()))?;

        let mut revoked = HashSet::new();
        for record in results {
            if let Some(token_bytes) = crate::utils::hex_str_to_vec(&record.token_id) {
                revoked.insert(token_bytes);
            }
        }

        Ok(revoked)
    }

    pub fn mnemonic_exists(&self) -> Result<bool, APIError> {
        Ok(block_on(Mnemonic::find_by_id(1).one(self.get_connection()))?.is_some())
    }

    pub fn persist_channel_peer(
        &self,
        pubkey: &PublicKey,
        address: &SocketAddr,
    ) -> Result<(), APIError> {
        let now = crate::utils::get_current_timestamp() as i64;

        let peer = channel_peer::ActiveModel {
            pubkey: ActiveValue::Set(pubkey.to_string()),
            address: ActiveValue::Set(address.to_string()),
            created_at: ActiveValue::Set(now),
        };

        block_on(
            ChannelPeer::insert(peer)
                .on_conflict(
                    OnConflict::column(channel_peer::Column::Pubkey)
                        .update_column(channel_peer::Column::Address)
                        .to_owned(),
                )
                .exec(self.get_connection()),
        )?;

        tracing::info!("persisted peer (pubkey: {pubkey}, addr: {address})");
        Ok(())
    }

    pub fn read_channel_peer_data(&self) -> Result<HashMap<PublicKey, SocketAddr>, APIError> {
        let results = block_on(ChannelPeer::find().all(self.get_connection()))?;

        let mut peer_data = HashMap::new();
        for record in results {
            if let (Ok(pubkey), Ok(address)) = (
                PublicKey::from_str(&record.pubkey),
                SocketAddr::from_str(&record.address),
            ) {
                peer_data.insert(pubkey, address);
            }
        }

        Ok(peer_data)
    }

    pub fn save_mnemonic(&self, encrypted_mnemonic: String) -> Result<(), APIError> {
        let now = crate::utils::get_current_timestamp() as i64;

        let mnemonic = mnemonic::ActiveModel {
            idx: ActiveValue::Set(1),
            encrypted_mnemonic: ActiveValue::Set(encrypted_mnemonic),
            created_at: ActiveValue::Set(now),
            updated_at: ActiveValue::Set(now),
        };

        block_on(
            Mnemonic::insert(mnemonic)
                .on_conflict(
                    OnConflict::column(mnemonic::Column::Idx)
                        .update_columns([
                            mnemonic::Column::EncryptedMnemonic,
                            mnemonic::Column::UpdatedAt,
                        ])
                        .to_owned(),
                )
                .exec(self.get_connection()),
        )?;

        Ok(())
    }

    pub fn set_config(&self, key: &str, value: &str) -> Result<(), APIError> {
        let now = crate::utils::get_current_timestamp() as i64;

        let config = config::ActiveModel {
            key: ActiveValue::Set(key.to_string()),
            value: ActiveValue::Set(value.to_string()),
            created_at: ActiveValue::Set(now),
            updated_at: ActiveValue::Set(now),
        };

        block_on(
            Config::insert(config)
                .on_conflict(
                    OnConflict::column(config::Column::Key)
                        .update_columns([config::Column::Value, config::Column::UpdatedAt])
                        .to_owned(),
                )
                .exec(self.get_connection()),
        )?;

        Ok(())
    }
}
