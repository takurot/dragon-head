use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;

/// Encapsulates cookie data for storage and restoration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CookieData {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub expires: f64,
    pub size: u32,
    pub http_only: bool,
    pub secure: bool,
    pub session: bool,
    pub same_site: Option<String>,
    pub priority: String,
}

/// Represents the complete state of a browser session including cookies and other tokens.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionData {
    pub domain: String,
    pub cookies: Vec<CookieData>,
    pub tokens: HashMap<String, String>,
}

/// Interface for Key Management System (KMS) operations.
#[async_trait]
pub trait KmsAdapter: Send + Sync {
    /// Encrypts plaintext using the current active key.
    /// Returns (ciphertext, key_id).
    async fn encrypt(&self, plaintext: &[u8]) -> Result<(Vec<u8>, String)>;

    /// Decrypts ciphertext using a specific key ID.
    async fn decrypt(&self, ciphertext: &[u8], key_id: &str) -> Result<Vec<u8>>;

    /// Returns the ID of the current active key.
    fn current_key_id(&self) -> String;

    /// Adds a new key to the adapter.
    fn add_key(&mut self, key: [u8; 32], key_id: String, make_current: bool);
}

/// Trait for storing and loading encrypted session data.
#[async_trait]
pub trait SessionVault: Send + Sync {
    /// Stores session data encrypted for a given session ID.
    async fn store_session(&self, session_id: &str, data: &SessionData) -> Result<()>;

    /// Loads and decrypts session data for a given session ID.
    async fn load_session(&self, session_id: &str) -> Result<Option<SessionData>>;

    /// Rotates keys by re-encrypting all stored sessions with a new key.
    async fn rotate_key(&self, new_key: [u8; 32], new_key_id: String) -> Result<()>;
}

type VaultStorage = HashMap<String, (Vec<u8>, String)>;

/// A local implementation of SessionVault using an in-memory storage.
pub struct LocalSessionVault {
    kms: Arc<Mutex<Box<dyn KmsAdapter>>>,
    storage: Arc<Mutex<VaultStorage>>,
}

impl LocalSessionVault {
    /// Creates a new LocalSessionVault with the provided KMS adapter.
    pub fn new(kms: Box<dyn KmsAdapter>) -> Self {
        Self {
            kms: Arc::new(Mutex::new(kms)),
            storage: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl SessionVault for LocalSessionVault {
    async fn store_session(&self, session_id: &str, data: &SessionData) -> Result<()> {
        let plaintext = serde_json::to_vec(data).context("Failed to serialize session data")?;
        let (ciphertext, key_id) = {
            let kms = self.kms.lock().await;
            kms.encrypt(&plaintext).await?
        };
        let mut storage = self.storage.lock().await;
        storage.insert(session_id.to_string(), (ciphertext, key_id));
        Ok(())
    }

    async fn load_session(&self, session_id: &str) -> Result<Option<SessionData>> {
        let entry = {
            let storage = self.storage.lock().await;
            storage.get(session_id).cloned()
        };

        let (ciphertext, key_id) = match entry {
            Some(e) => e,
            None => return Ok(None),
        };

        let kms = self.kms.lock().await;
        let plaintext = kms.decrypt(&ciphertext, &key_id).await?;
        let data =
            serde_json::from_slice(&plaintext).context("Failed to deserialize session data")?;
        Ok(Some(data))
    }

    async fn rotate_key(&self, new_key: [u8; 32], new_key_id: String) -> Result<()> {
        let mut kms_guard = self.kms.lock().await;
        let mut storage = self.storage.lock().await;

        // 1. Decrypt everything with current keys
        let mut decrypted_items = Vec::with_capacity(storage.len());
        for (sid, (ct, kid)) in storage.iter() {
            let pt = kms_guard.decrypt(ct, kid).await?;
            decrypted_items.push((sid.clone(), pt));
        }

        // 2. Add new key and make it current
        kms_guard.add_key(new_key, new_key_id, true);

        // 3. Re-encrypt everything with the NEW current key
        for (sid, pt) in decrypted_items {
            let (ct, kid) = kms_guard.encrypt(&pt).await?;
            storage.insert(sid, (ct, kid));
        }

        Ok(())
    }
}

/// Software-based KMS implementation using local keys.
pub struct SoftwareKms {
    keys: HashMap<String, [u8; 32]>,
    current_key_id: String,
}

impl SoftwareKms {
    /// Creates a new SoftwareKms with an initial key.
    pub fn new(key: [u8; 32], key_id: String) -> Self {
        let mut keys = HashMap::new();
        keys.insert(key_id.clone(), key);
        Self {
            keys,
            current_key_id: key_id,
        }
    }
}

#[async_trait]
impl KmsAdapter for SoftwareKms {
    async fn encrypt(&self, plaintext: &[u8]) -> Result<(Vec<u8>, String)> {
        use aes_gcm::{
            aead::{Aead, KeyInit},
            Aes256Gcm, Nonce,
        };
        use rand::RngCore;

        let key = self
            .keys
            .get(&self.current_key_id)
            .context("Current key not found")?;
        let cipher = Aes256Gcm::new(key.into());
        let mut nonce_bytes = [0u8; 12];
        rand::rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = cipher
            .encrypt(nonce, plaintext)
            .map_err(|e| anyhow::anyhow!("Encryption failed: {}", e))?;

        let mut result = Vec::with_capacity(nonce_bytes.len() + ciphertext.len());
        result.extend_from_slice(&nonce_bytes);
        result.extend_from_slice(&ciphertext);
        Ok((result, self.current_key_id.clone()))
    }

    async fn decrypt(&self, ciphertext: &[u8], key_id: &str) -> Result<Vec<u8>> {
        use aes_gcm::{
            aead::{Aead, KeyInit},
            Aes256Gcm, Nonce,
        };

        let key = self.keys.get(key_id).context("Key not found")?;
        if ciphertext.len() < 12 {
            anyhow::bail!("Invalid ciphertext length");
        }

        let cipher = Aes256Gcm::new(key.into());
        let nonce = Nonce::from_slice(&ciphertext[..12]);
        let ciphertext = &ciphertext[12..];

        cipher
            .decrypt(nonce, ciphertext)
            .map_err(|e| anyhow::anyhow!("Decryption failed: {}", e))
    }

    fn current_key_id(&self) -> String {
        self.current_key_id.clone()
    }

    fn add_key(&mut self, key: [u8; 32], key_id: String, make_current: bool) {
        self.keys.insert(key_id.clone(), key);
        if make_current {
            self.current_key_id = key_id;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_local_session_vault_roundtrip() -> Result<()> {
        let key = [0u8; 32];
        let kms = Box::new(SoftwareKms::new(key, "test-key".to_string()));
        let vault = LocalSessionVault::new(kms);

        let session_id = "test-session";
        let data = SessionData {
            domain: "example.com".to_string(),
            cookies: vec![CookieData {
                name: "sessionid".to_string(),
                value: "abc".to_string(),
                domain: "example.com".to_string(),
                path: "/".to_string(),
                expires: -1.0,
                size: 10,
                http_only: true,
                secure: true,
                session: true,
                same_site: None,
                priority: "Medium".to_string(),
            }],
            tokens: [("auth".to_string(), "xyz".to_string())]
                .into_iter()
                .collect(),
        };

        vault.store_session(session_id, &data).await?;
        let loaded = vault.load_session(session_id).await?;

        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert_eq!(loaded.domain, data.domain);
        assert_eq!(loaded.cookies, data.cookies);
        assert_eq!(loaded.tokens, data.tokens);

        Ok(())
    }

    #[tokio::test]
    async fn test_key_rotation() -> Result<()> {
        let key1 = [1u8; 32];
        let kms = Box::new(SoftwareKms::new(key1, "key-1".to_string()));
        let vault = LocalSessionVault::new(kms);

        let session_id = "test-session";
        let data = SessionData {
            domain: "example.com".to_string(),
            cookies: vec![CookieData {
                name: "sessionid".to_string(),
                value: "123".to_string(),
                domain: "example.com".to_string(),
                path: "/".to_string(),
                expires: -1.0,
                size: 10,
                http_only: true,
                secure: true,
                session: true,
                same_site: None,
                priority: "Medium".to_string(),
            }],
            tokens: HashMap::new(),
        };

        // Store with key1
        vault.store_session(session_id, &data).await?;

        // Rotate to key2
        let key2 = [2u8; 32];
        vault.rotate_key(key2, "key-2".to_string()).await?;

        // Should still be loadable (decrypted with key2 after rotation)
        let loaded = vault.load_session(session_id).await?;
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().cookies[0].value, "123");

        // Verify it was re-encrypted with key2
        let storage = vault.storage.lock().await;
        let (_, key_id) = storage.get(session_id).unwrap();
        assert_eq!(key_id, "key-2");

        Ok(())
    }
}
