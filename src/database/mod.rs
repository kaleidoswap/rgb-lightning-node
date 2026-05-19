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
    channel_peer, config,
    prelude::{ChannelPeer, Config, RevokedToken},
    revoked_token,
};
use crate::error::APIError;
use crate::runtime::block_on;

const CONFIG_IDX: i32 = 1;

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

    pub fn get_config(&self) -> Result<Option<config::Model>, APIError> {
        Ok(block_on(
            Config::find_by_id(CONFIG_IDX).one(self.get_connection()),
        )?)
    }

    pub fn is_initialized(&self) -> Result<bool, APIError> {
        Ok(self.get_config()?.is_some())
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

        let row = config::ActiveModel {
            idx: ActiveValue::Set(CONFIG_IDX),
            encrypted_mnemonic: ActiveValue::Set(encrypted_mnemonic),
            indexer_url: ActiveValue::NotSet,
            bitcoin_network: ActiveValue::NotSet,
            wallet_fingerprint: ActiveValue::NotSet,
            wallet_account_xpub_vanilla: ActiveValue::NotSet,
            wallet_account_xpub_colored: ActiveValue::NotSet,
            wallet_master_fingerprint: ActiveValue::NotSet,
            created_at: ActiveValue::Set(now),
            updated_at: ActiveValue::Set(now),
        };

        block_on(
            Config::insert(row)
                .on_conflict(
                    OnConflict::column(config::Column::Idx)
                        .update_columns([
                            config::Column::EncryptedMnemonic,
                            config::Column::UpdatedAt,
                        ])
                        .to_owned(),
                )
                .exec(self.get_connection()),
        )?;

        Ok(())
    }

    fn update_config_field(&self, column: config::Column, value: &str) -> Result<(), APIError> {
        let now = crate::utils::get_current_timestamp() as i64;
        block_on(
            Config::update_many()
                .filter(config::Column::Idx.eq(CONFIG_IDX))
                .col_expr(column, value.into())
                .col_expr(config::Column::UpdatedAt, now.into())
                .exec(self.get_connection()),
        )?;
        Ok(())
    }

    pub fn set_indexer_url(&self, value: &str) -> Result<(), APIError> {
        self.update_config_field(config::Column::IndexerUrl, value)
    }

    pub fn set_bitcoin_network(&self, value: &str) -> Result<(), APIError> {
        self.update_config_field(config::Column::BitcoinNetwork, value)
    }

    pub fn set_wallet_fingerprint(&self, value: &str) -> Result<(), APIError> {
        self.update_config_field(config::Column::WalletFingerprint, value)
    }

    pub fn set_wallet_account_xpub_vanilla(&self, value: &str) -> Result<(), APIError> {
        self.update_config_field(config::Column::WalletAccountXpubVanilla, value)
    }

    pub fn set_wallet_account_xpub_colored(&self, value: &str) -> Result<(), APIError> {
        self.update_config_field(config::Column::WalletAccountXpubColored, value)
    }

    pub fn set_wallet_master_fingerprint(&self, value: &str) -> Result<(), APIError> {
        self.update_config_field(config::Column::WalletMasterFingerprint, value)
    }
}
