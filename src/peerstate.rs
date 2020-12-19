//! # [Autocrypt Peer State](https://autocrypt.org/level1.html#peer-state-management) module
use std::collections::HashSet;
use std::fmt;

use num_traits::FromPrimitive;
use sqlx::Row;

use crate::aheader::*;
use crate::chat;
use crate::constants::Blocked;
use crate::context::Context;
use crate::error::{bail, Result};
use crate::events::EventType;
use crate::key::{DcKey, Fingerprint, SignedPublicKey};
use crate::sql::Sql;
use crate::stock::StockMessage;

#[derive(Debug)]
pub enum PeerstateKeyType {
    GossipKey,
    PublicKey,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, FromPrimitive)]
#[repr(u8)]
pub enum PeerstateVerifiedStatus {
    Unverified = 0,
    //Verified = 1, // not used
    BidirectVerified = 2,
}

/// Peerstate represents the state of an Autocrypt peer.
pub struct Peerstate<'a> {
    pub context: &'a Context,
    pub addr: String,
    pub last_seen: i64,
    pub last_seen_autocrypt: i64,
    pub prefer_encrypt: EncryptPreference,
    pub public_key: Option<SignedPublicKey>,
    pub public_key_fingerprint: Option<Fingerprint>,
    pub gossip_key: Option<SignedPublicKey>,
    pub gossip_timestamp: i64,
    pub gossip_key_fingerprint: Option<Fingerprint>,
    pub verified_key: Option<SignedPublicKey>,
    pub verified_key_fingerprint: Option<Fingerprint>,
    pub to_save: Option<ToSave>,
    pub fingerprint_changed: bool,
}

impl<'a> PartialEq for Peerstate<'a> {
    fn eq(&self, other: &Peerstate) -> bool {
        self.addr == other.addr
            && self.last_seen == other.last_seen
            && self.last_seen_autocrypt == other.last_seen_autocrypt
            && self.prefer_encrypt == other.prefer_encrypt
            && self.public_key == other.public_key
            && self.public_key_fingerprint == other.public_key_fingerprint
            && self.gossip_key == other.gossip_key
            && self.gossip_timestamp == other.gossip_timestamp
            && self.gossip_key_fingerprint == other.gossip_key_fingerprint
            && self.verified_key == other.verified_key
            && self.verified_key_fingerprint == other.verified_key_fingerprint
            && self.to_save == other.to_save
            && self.fingerprint_changed == other.fingerprint_changed
    }
}

impl<'a> Eq for Peerstate<'a> {}

impl<'a> fmt::Debug for Peerstate<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("Peerstate")
            .field("addr", &self.addr)
            .field("last_seen", &self.last_seen)
            .field("last_seen_autocrypt", &self.last_seen_autocrypt)
            .field("prefer_encrypt", &self.prefer_encrypt)
            .field("public_key", &self.public_key)
            .field("public_key_fingerprint", &self.public_key_fingerprint)
            .field("gossip_key", &self.gossip_key)
            .field("gossip_timestamp", &self.gossip_timestamp)
            .field("gossip_key_fingerprint", &self.gossip_key_fingerprint)
            .field("verified_key", &self.verified_key)
            .field("verified_key_fingerprint", &self.verified_key_fingerprint)
            .field("to_save", &self.to_save)
            .field("fingerprint_changed", &self.fingerprint_changed)
            .finish()
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, FromPrimitive, ToPrimitive)]
#[repr(u8)]
pub enum ToSave {
    Timestamps = 0x01,
    All = 0x02,
}

impl<'a> Peerstate<'a> {
    pub fn from_header(context: &'a Context, header: &Aheader, message_time: i64) -> Self {
        Peerstate {
            context,
            addr: header.addr.clone(),
            last_seen: message_time,
            last_seen_autocrypt: message_time,
            prefer_encrypt: header.prefer_encrypt,
            public_key: Some(header.public_key.clone()),
            public_key_fingerprint: Some(header.public_key.fingerprint()),
            gossip_key: None,
            gossip_key_fingerprint: None,
            gossip_timestamp: 0,
            verified_key: None,
            verified_key_fingerprint: None,
            to_save: Some(ToSave::All),
            fingerprint_changed: false,
        }
    }

    pub fn from_gossip(context: &'a Context, gossip_header: &Aheader, message_time: i64) -> Self {
        Peerstate {
            context,
            addr: gossip_header.addr.clone(),
            last_seen: 0,
            last_seen_autocrypt: 0,
            prefer_encrypt: Default::default(),
            public_key: None,
            public_key_fingerprint: None,
            gossip_key: Some(gossip_header.public_key.clone()),
            gossip_key_fingerprint: Some(gossip_header.public_key.fingerprint()),
            gossip_timestamp: message_time,
            verified_key: None,
            verified_key_fingerprint: None,
            to_save: Some(ToSave::All),
            fingerprint_changed: false,
        }
    }

    pub async fn from_addr(context: &'a Context, addr: &str) -> Result<Option<Peerstate<'a>>> {
        let query = sqlx::query(
            "SELECT addr, last_seen, last_seen_autocrypt, prefer_encrypted, public_key, \
                     gossip_timestamp, gossip_key, public_key_fingerprint, gossip_key_fingerprint, \
                     verified_key, verified_key_fingerprint \
                     FROM acpeerstates \
                     WHERE addr=? COLLATE NOCASE;",
        )
        .bind(addr);
        Self::from_stmt(context, query).await
    }

    pub async fn from_fingerprint(
        context: &'a Context,
        _sql: &Sql,
        fingerprint: &Fingerprint,
    ) -> Result<Option<Peerstate<'a>>> {
        let fp = fingerprint.hex();
        let query = sqlx::query(
            "SELECT addr, last_seen, last_seen_autocrypt, prefer_encrypted, public_key, \
                     gossip_timestamp, gossip_key, public_key_fingerprint, gossip_key_fingerprint, \
                     verified_key, verified_key_fingerprint \
                     FROM acpeerstates  \
                     WHERE public_key_fingerprint=? COLLATE NOCASE \
                     OR gossip_key_fingerprint=? COLLATE NOCASE  \
                     ORDER BY public_key_fingerprint=? DESC;",
        )
        .bind(&fp)
        .bind(&fp)
        .bind(&fp);

        Self::from_stmt(context, query).await
    }

    async fn from_stmt<'e, 'q, E>(context: &'a Context, query: E) -> Result<Option<Peerstate<'a>>>
    where
        'q: 'e,
        E: 'q + sqlx::Execute<'q, sqlx::Sqlite>,
    {
        if let Some(row) = context.sql.fetch_optional(query).await? {
            // all the above queries start with this: SELECT
            //   addr, last_seen, last_seen_autocrypt, prefer_encrypted,
            //   public_key, gossip_timestamp, gossip_key, public_key_fingerprint,
            //   gossip_key_fingerprint, verified_key, verified_key_fingerprint

            let peerstate = Peerstate {
                context,
                addr: row.try_get(0)?,
                last_seen: row.try_get(1)?,
                last_seen_autocrypt: row.try_get(2)?,
                prefer_encrypt: EncryptPreference::from_i32(row.try_get(3)?).unwrap_or_default(),
                public_key: row
                    .try_get::<&[u8], _>(4)
                    .ok()
                    .and_then(|blob| SignedPublicKey::from_slice(blob).ok()),
                public_key_fingerprint: row
                    .try_get::<Option<String>, _>(7)?
                    .map(|s| s.parse::<Fingerprint>())
                    .transpose()
                    .unwrap_or_default(),
                gossip_key: row
                    .try_get::<&[u8], _>(6)
                    .ok()
                    .and_then(|blob| SignedPublicKey::from_slice(blob).ok()),
                gossip_key_fingerprint: row
                    .try_get::<Option<String>, _>(8)?
                    .map(|s| s.parse::<Fingerprint>())
                    .transpose()
                    .unwrap_or_default(),
                gossip_timestamp: row.try_get(5)?,
                verified_key: row
                    .try_get::<&[u8], _>(9)
                    .ok()
                    .and_then(|blob| SignedPublicKey::from_slice(blob).ok()),
                verified_key_fingerprint: row
                    .try_get::<Option<String>, _>(10)?
                    .map(|s| s.parse::<Fingerprint>())
                    .transpose()
                    .unwrap_or_default(),
                to_save: None,
                fingerprint_changed: false,
            };

            Ok(Some(peerstate))
        } else {
            Ok(None)
        }
    }

    pub fn recalc_fingerprint(&mut self) {
        if let Some(ref public_key) = self.public_key {
            let old_public_fingerprint = self.public_key_fingerprint.take();
            self.public_key_fingerprint = Some(public_key.fingerprint());

            if old_public_fingerprint.is_none()
                || self.public_key_fingerprint.is_none()
                || old_public_fingerprint != self.public_key_fingerprint
            {
                self.to_save = Some(ToSave::All);
                if old_public_fingerprint.is_some() {
                    self.fingerprint_changed = true;
                }
            }
        }

        if let Some(ref gossip_key) = self.gossip_key {
            let old_gossip_fingerprint = self.gossip_key_fingerprint.take();
            self.gossip_key_fingerprint = Some(gossip_key.fingerprint());

            if old_gossip_fingerprint.is_none()
                || self.gossip_key_fingerprint.is_none()
                || old_gossip_fingerprint != self.gossip_key_fingerprint
            {
                self.to_save = Some(ToSave::All);

                // Warn about gossip key change only if there is no public key obtained from
                // Autocrypt header, which overrides gossip key.
                if old_gossip_fingerprint.is_some() && self.public_key_fingerprint.is_none() {
                    self.fingerprint_changed = true;
                }
            }
        }
    }

    pub fn degrade_encryption(&mut self, message_time: i64) {
        self.prefer_encrypt = EncryptPreference::Reset;
        self.last_seen = message_time;
        self.to_save = Some(ToSave::All);
    }

    /// Adds a warning to the chat corresponding to peerstate if fingerprint has changed.
    pub(crate) async fn handle_fingerprint_change(&self, context: &Context) -> Result<()> {
        if self.fingerprint_changed {
            if let Some(contact_id) = context
                .sql
                .query_get_value(
                    sqlx::query("SELECT id FROM contacts WHERE addr=?;").bind(&self.addr),
                )
                .await?
            {
                let (contact_chat_id, _) =
                    chat::create_or_lookup_by_contact_id(context, contact_id, Blocked::Deaddrop)
                        .await
                        .unwrap_or_default();

                let msg = context
                    .stock_string_repl_str(StockMessage::ContactSetupChanged, self.addr.clone())
                    .await;

                chat::add_info_msg(context, contact_chat_id, msg).await;
                emit_event!(context, EventType::ChatModified(contact_chat_id));
            } else {
                bail!("contact with peerstate.addr {:?} not found", &self.addr);
            }
        }
        Ok(())
    }

    pub fn apply_header(&mut self, header: &Aheader, message_time: i64) {
        if self.addr.to_lowercase() != header.addr.to_lowercase() {
            return;
        }

        if message_time > self.last_seen {
            self.last_seen = message_time;
            self.last_seen_autocrypt = message_time;
            self.to_save = Some(ToSave::Timestamps);
            if (header.prefer_encrypt == EncryptPreference::Mutual
                || header.prefer_encrypt == EncryptPreference::NoPreference)
                && header.prefer_encrypt != self.prefer_encrypt
            {
                self.prefer_encrypt = header.prefer_encrypt;
                self.to_save = Some(ToSave::All)
            }

            if self.public_key.as_ref() != Some(&header.public_key) {
                self.public_key = Some(header.public_key.clone());
                self.recalc_fingerprint();
                self.to_save = Some(ToSave::All);
            }
        }
    }

    pub fn apply_gossip(&mut self, gossip_header: &Aheader, message_time: i64) {
        if self.addr.to_lowercase() != gossip_header.addr.to_lowercase() {
            return;
        }

        if message_time > self.gossip_timestamp {
            self.gossip_timestamp = message_time;
            self.to_save = Some(ToSave::Timestamps);
            if self.gossip_key.as_ref() != Some(&gossip_header.public_key) {
                self.gossip_key = Some(gossip_header.public_key.clone());
                self.recalc_fingerprint();
                self.to_save = Some(ToSave::All)
            }

            // This is non-standard.
            //
            // According to Autocrypt 1.1.0 gossip headers SHOULD NOT
            // contain encryption preference, but we include it into
            // Autocrypt-Gossip and apply it one way (from
            // "nopreference" to "mutual").
            //
            // This is compatible to standard clients, because they
            // can't distinguish it from the case where we have
            // contacted the client in the past and received this
            // preference via Autocrypt header.
            if self.last_seen_autocrypt == 0
                && self.prefer_encrypt == EncryptPreference::NoPreference
                && gossip_header.prefer_encrypt == EncryptPreference::Mutual
            {
                self.prefer_encrypt = EncryptPreference::Mutual;
                self.to_save = Some(ToSave::All);
            }
        };
    }

    pub fn render_gossip_header(&self, min_verified: PeerstateVerifiedStatus) -> Option<String> {
        if let Some(key) = self.peek_key(min_verified) {
            let header = Aheader::new(
                self.addr.clone(),
                key.clone(), // TODO: avoid cloning
                // Autocrypt 1.1.0 specification says that
                // `prefer-encrypt` attribute SHOULD NOT be included,
                // but we include it anyway to propagate encryption
                // preference to new members in group chats.
                if self.last_seen_autocrypt > 0 {
                    self.prefer_encrypt
                } else {
                    EncryptPreference::NoPreference
                },
            );
            Some(header.to_string())
        } else {
            None
        }
    }

    pub fn take_key(mut self, min_verified: PeerstateVerifiedStatus) -> Option<SignedPublicKey> {
        match min_verified {
            PeerstateVerifiedStatus::BidirectVerified => self.verified_key.take(),
            PeerstateVerifiedStatus::Unverified => {
                self.public_key.take().or_else(|| self.gossip_key.take())
            }
        }
    }

    pub fn peek_key(&self, min_verified: PeerstateVerifiedStatus) -> Option<&SignedPublicKey> {
        match min_verified {
            PeerstateVerifiedStatus::BidirectVerified => self.verified_key.as_ref(),
            PeerstateVerifiedStatus::Unverified => self
                .public_key
                .as_ref()
                .or_else(|| self.gossip_key.as_ref()),
        }
    }

    pub fn set_verified(
        &mut self,
        which_key: PeerstateKeyType,
        fingerprint: &Fingerprint,
        verified: PeerstateVerifiedStatus,
    ) -> bool {
        if verified == PeerstateVerifiedStatus::BidirectVerified {
            match which_key {
                PeerstateKeyType::PublicKey => {
                    if self.public_key_fingerprint.is_some()
                        && self.public_key_fingerprint.as_ref().unwrap() == fingerprint
                    {
                        self.to_save = Some(ToSave::All);
                        self.verified_key = self.public_key.clone();
                        self.verified_key_fingerprint = self.public_key_fingerprint.clone();
                        true
                    } else {
                        false
                    }
                }
                PeerstateKeyType::GossipKey => {
                    if self.gossip_key_fingerprint.is_some()
                        && self.gossip_key_fingerprint.as_ref().unwrap() == fingerprint
                    {
                        self.to_save = Some(ToSave::All);
                        self.verified_key = self.gossip_key.clone();
                        self.verified_key_fingerprint = self.gossip_key_fingerprint.clone();
                        true
                    } else {
                        false
                    }
                }
            }
        } else {
            false
        }
    }

    pub async fn save_to_db(&self, sql: &Sql, create: bool) -> crate::sql::Result<()> {
        if self.to_save == Some(ToSave::All) || create {
            sql.execute(
                (if create {
                    sqlx::query(
                        "INSERT INTO acpeerstates (last_seen, last_seen_autocrypt, prefer_encrypted, \
                                 public_key, gossip_timestamp, gossip_key, public_key_fingerprint, gossip_key_fingerprint, \
                                 verified_key, verified_key_fingerprint, addr \
                                 ) VALUES(?,?,?,?,?,?,?,?,?,?,?)"
                    )
                } else {
                    sqlx::query(
                        "UPDATE acpeerstates \
                 SET last_seen=?, last_seen_autocrypt=?, prefer_encrypted=?, \
                 public_key=?, gossip_timestamp=?, gossip_key=?, public_key_fingerprint=?, gossip_key_fingerprint=?, \
                 verified_key=?, verified_key_fingerprint=? \
                 WHERE addr=?"
                    )
                }).bind(
                    self.last_seen).bind(
                    self.last_seen_autocrypt).bind(
                    self.prefer_encrypt as i64).bind(
                    self.public_key.as_ref().map(|k| k.to_bytes())).bind(
                    self.gossip_timestamp).bind(
                    self.gossip_key.as_ref().map(|k| k.to_bytes())).bind(
                    self.public_key_fingerprint.as_ref().map(|fp| fp.hex())).bind(
                    self.gossip_key_fingerprint.as_ref().map(|fp| fp.hex())).bind(
                    self.verified_key.as_ref().map(|k| k.to_bytes())).bind(
                    self.verified_key_fingerprint.as_ref().map(|fp| fp.hex())).bind(
                    &self.addr)

            ).await?;
        } else if self.to_save == Some(ToSave::Timestamps) {
            sql.execute(
                sqlx::query("UPDATE acpeerstates SET last_seen=?, last_seen_autocrypt=?, gossip_timestamp=? \
                 WHERE addr=?;").bind(
                    self.last_seen).bind(
                    self.last_seen_autocrypt).bind(
                    self.gossip_timestamp).bind(
                    &self.addr)
            )
            .await?;
        }

        Ok(())
    }

    pub fn has_verified_key(&self, fingerprints: &HashSet<Fingerprint>) -> bool {
        if let Some(vkc) = &self.verified_key_fingerprint {
            fingerprints.contains(vkc) && self.verified_key.is_some()
        } else {
            false
        }
    }
}

impl From<crate::key::FingerprintError> for rusqlite::Error {
    fn from(_source: crate::key::FingerprintError) -> Self {
        Self::InvalidColumnType(0, "Invalid fingerprint".into(), rusqlite::types::Type::Text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::*;
    use pretty_assertions::assert_eq;

    #[async_std::test]
    async fn test_peerstate_save_to_db() {
        let ctx = crate::test_utils::TestContext::new().await;
        let addr = "hello@mail.com";

        let pub_key = alice_keypair().public;

        let mut peerstate = Peerstate {
            context: &ctx.ctx,
            addr: addr.into(),
            last_seen: 10,
            last_seen_autocrypt: 11,
            prefer_encrypt: EncryptPreference::Mutual,
            public_key: Some(pub_key.clone()),
            public_key_fingerprint: Some(pub_key.fingerprint()),
            gossip_key: Some(pub_key.clone()),
            gossip_timestamp: 12,
            gossip_key_fingerprint: Some(pub_key.fingerprint()),
            verified_key: Some(pub_key.clone()),
            verified_key_fingerprint: Some(pub_key.fingerprint()),
            to_save: Some(ToSave::All),
            fingerprint_changed: false,
        };

        assert!(
            peerstate.save_to_db(&ctx.ctx.sql, true).await.is_ok(),
            "failed to save to db"
        );

        let peerstate_new = Peerstate::from_addr(&ctx.ctx, addr)
            .await
            .expect("failed to load peerstate from db")
            .expect("no peerstate found in the database");

        // clear to_save, as that is not persissted
        peerstate.to_save = None;
        assert_eq!(peerstate, peerstate_new);
        let peerstate_new2 =
            Peerstate::from_fingerprint(&ctx.ctx, &ctx.ctx.sql, &pub_key.fingerprint())
                .await
                .expect("failed to load peerstate from db")
                .expect("no peerstate found in the database");
        assert_eq!(peerstate, peerstate_new2);
    }

    #[async_std::test]
    async fn test_peerstate_double_create() {
        let ctx = crate::test_utils::TestContext::new().await;
        let addr = "hello@mail.com";
        let pub_key = alice_keypair().public;

        let peerstate = Peerstate {
            context: &ctx.ctx,
            addr: addr.into(),
            last_seen: 10,
            last_seen_autocrypt: 11,
            prefer_encrypt: EncryptPreference::Mutual,
            public_key: Some(pub_key.clone()),
            public_key_fingerprint: Some(pub_key.fingerprint()),
            gossip_key: None,
            gossip_timestamp: 12,
            gossip_key_fingerprint: None,
            verified_key: None,
            verified_key_fingerprint: None,
            to_save: Some(ToSave::All),
            fingerprint_changed: false,
        };

        assert!(
            peerstate.save_to_db(&ctx.ctx.sql, true).await.is_ok(),
            "failed to save"
        );
        assert!(
            peerstate.save_to_db(&ctx.ctx.sql, true).await.is_ok(),
            "double-call with create failed"
        );
    }

    #[async_std::test]
    async fn test_peerstate_with_empty_gossip_key_save_to_db() {
        let ctx = crate::test_utils::TestContext::new().await;
        let addr = "hello@mail.com";

        let pub_key = alice_keypair().public;

        let mut peerstate = Peerstate {
            context: &ctx.ctx,
            addr: addr.into(),
            last_seen: 10,
            last_seen_autocrypt: 11,
            prefer_encrypt: EncryptPreference::Mutual,
            public_key: Some(pub_key.clone()),
            public_key_fingerprint: Some(pub_key.fingerprint()),
            gossip_key: None,
            gossip_timestamp: 12,
            gossip_key_fingerprint: None,
            verified_key: None,
            verified_key_fingerprint: None,
            to_save: Some(ToSave::All),
            fingerprint_changed: false,
        };

        assert!(
            peerstate.save_to_db(&ctx.ctx.sql, true).await.is_ok(),
            "failed to save"
        );

        let peerstate_new = Peerstate::from_addr(&ctx.ctx, addr)
            .await
            .expect("failed to load peerstate from db");

        // clear to_save, as that is not persissted
        peerstate.to_save = None;
        assert_eq!(Some(peerstate), peerstate_new);
    }

    #[async_std::test]
    async fn test_peerstate_load_db_defaults() {
        let ctx = crate::test_utils::TestContext::new().await;
        let addr = "hello@mail.com";

        // Old code created peerstates with this code and updated
        // other values later.  If UPDATE failed, other columns had
        // default values, in particular fingerprints were set to
        // empty strings instead of NULL. This should not be the case
        // anymore, but the regression test still checks that defaults
        // can be loaded without errors.
        ctx.ctx
            .sql
            .execute(sqlx::query("INSERT INTO acpeerstates (addr) VALUES(?)").bind(addr))
            .await
            .expect("Failed to write to the database");

        let peerstate = Peerstate::from_addr(&ctx.ctx, addr)
            .await
            .expect("Failed to load peerstate from db")
            .expect("Loaded peerstate is empty");

        // Check that default values for fingerprints are treated like
        // NULL.
        assert_eq!(peerstate.public_key_fingerprint, None);
        assert_eq!(peerstate.gossip_key_fingerprint, None);
        assert_eq!(peerstate.verified_key_fingerprint, None);
    }

    #[async_std::test]
    async fn test_peerstate_degrade_reordering() {
        let context = crate::test_utils::TestContext::new().await.ctx;
        let addr = "example@example.org";
        let pub_key = alice_keypair().public;
        let header = Aheader::new(addr.to_string(), pub_key, EncryptPreference::Mutual);

        let mut peerstate = Peerstate {
            context: &context,
            addr: addr.to_string(),
            last_seen: 0,
            last_seen_autocrypt: 0,
            prefer_encrypt: EncryptPreference::NoPreference,
            public_key: None,
            public_key_fingerprint: None,
            gossip_key: None,
            gossip_timestamp: 0,
            gossip_key_fingerprint: None,
            verified_key: None,
            verified_key_fingerprint: None,
            to_save: None,
            fingerprint_changed: false,
        };
        assert_eq!(peerstate.prefer_encrypt, EncryptPreference::NoPreference);

        peerstate.apply_header(&header, 100);
        assert_eq!(peerstate.prefer_encrypt, EncryptPreference::Mutual);

        peerstate.degrade_encryption(300);
        assert_eq!(peerstate.prefer_encrypt, EncryptPreference::Reset);

        // This has message time 200, while encryption was degraded at timestamp 300.
        // Because of reordering, header should not be applied.
        peerstate.apply_header(&header, 200);
        assert_eq!(peerstate.prefer_encrypt, EncryptPreference::Reset);

        // Same header will be applied in the future.
        peerstate.apply_header(&header, 400);
        assert_eq!(peerstate.prefer_encrypt, EncryptPreference::Mutual);
    }
}
