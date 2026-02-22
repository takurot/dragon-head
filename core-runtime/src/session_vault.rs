use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionData {
    pub domain: String,
    pub cookies: Vec<String>, // Simplification for now: each string is a Set-Cookie value
    pub tokens: HashMap<String, String>,
}

#[async_trait]
pub trait KmsAdapter: Send + Sync {
    async fn encrypt(&self, plaintext: &[u8]) -> Result<(Vec<u8>, String)>;
    async fn decrypt(&self, ciphertext: &[u8], key_id: &str) -> Result<Vec<u8>>;
    fn current_key_id(&self) -> String;
    fn add_key(&mut self, key: [u8; 32], key_id: String, make_current: bool);
}

#[async_trait]
pub trait SessionVault: Send + Sync {
    async fn store_session(&self, session_id: &str, data: &SessionData) -> Result<()>;
    async fn load_session(&self, session_id: &str) -> Result<Option<SessionData>>;
    async fn rotate_key(&self, new_key: [u8; 32], new_key_id: String) -> Result<()>;
}

type VaultStorage = HashMap<String, (Vec<u8>, String)>;

pub struct LocalSessionVault {
    kms: Arc<Mutex<Box<dyn KmsAdapter>>>,
    storage: Arc<Mutex<VaultStorage>>,
}

impl LocalSessionVault {
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
        let kms = self.kms.lock().await;
        let (ciphertext, key_id) = kms.encrypt(&plaintext).await?;
        let mut storage = self.storage.lock().await;
        storage.insert(session_id.to_string(), (ciphertext, key_id));
        Ok(())
    }

    async fn load_session(&self, session_id: &str) -> Result<Option<SessionData>> {
        let storage = self.storage.lock().await;
        let (ciphertext, key_id) = match storage.get(session_id) {
            Some(entry) => entry,
            None => return Ok(None),
        };
        let kms = self.kms.lock().await;
        let plaintext = kms.decrypt(ciphertext, key_id).await?;
        let data =
            serde_json::from_slice(&plaintext).context("Failed to deserialize session data")?;
        Ok(Some(data))
    }

    async fn rotate_key(&self, new_key: [u8; 32], new_key_id: String) -> Result<()> {
        let mut kms_guard = self.kms.lock().await;
        let mut storage = self.storage.lock().await;

        // 1. Decrypt everything with current keys
        let mut all_data = Vec::new();
        for (sid, (ct, kid)) in storage.iter() {
            let pt = kms_guard.decrypt(ct, kid).await?;
            all_data.push((sid.clone(), pt));
        }

        // 2. Add new key and make it current
        kms_guard.add_key(new_key, new_key_id.clone(), true);

        // 3. Re-encrypt everything with the NEW current key
        for (sid, pt) in all_data {
            let (ct, kid) = kms_guard.encrypt(&pt).await?;
            storage.insert(sid, (ct, kid));
        }

        Ok(())
    }
}

pub struct SoftwareKms {
    keys: HashMap<String, [u8; 32]>,
    current_key_id: String,
}

impl SoftwareKms {
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
            aead::{Aead, KeyInit, OsRng},
            Aes256Gcm, Nonce,
        };
        use rand::RngCore;

        let key = self
            .keys
            .get(&self.current_key_id)
            .context("Current key not found")?;
        let cipher = Aes256Gcm::new(key.into());
        let mut nonce_bytes = [0u8; 12];
        OsRng.fill_bytes(&mut nonce_bytes);
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
            cookies: vec!["sessionid=abc".to_string()],
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
            cookies: vec!["sessionid=123".to_string()],
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
        assert_eq!(loaded.unwrap().cookies[0], "sessionid=123");

        // Verify it was re-encrypted with key2
        let storage = vault.storage.lock().await;
        let (_, key_id) = storage.get(session_id).unwrap();
        assert_eq!(key_id, "key-2");

        Ok(())
    }
}
