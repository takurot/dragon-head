use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use tokio::sync::Mutex;
use zeroize::Zeroizing;

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

    /// Stages and activates a key for rotation without overwriting an existing
    /// key ID.
    fn stage_key_rotation(&mut self, _key: [u8; 32], _key_id: String) -> Result<()> {
        anyhow::bail!("KMS adapter does not support atomic key rotation")
    }

    /// Restores the pre-rotation state after a later phase fails.
    fn rollback_key_rotation(&mut self, _previous_key_id: &str, _staged_key_id: &str) {}

    /// Atomically retires superseded keys. If this returns an error, none of
    /// the requested keys may have been removed.
    fn finalize_key_rotation(&mut self, _retired_key_ids: &[String]) -> Result<()> {
        anyhow::bail!("KMS adapter does not support atomic key rotation")
    }
}

struct KeyRotationGuard<'a> {
    kms: &'a mut dyn KmsAdapter,
    previous_key_id: String,
    staged_key_id: String,
    committed: bool,
}

impl<'a> KeyRotationGuard<'a> {
    fn new(kms: &'a mut dyn KmsAdapter, previous_key_id: String, staged_key_id: String) -> Self {
        Self {
            kms,
            previous_key_id,
            staged_key_id,
            committed: false,
        }
    }

    fn commit(&mut self, retired_key_ids: &[String]) -> Result<()> {
        self.kms.finalize_key_rotation(retired_key_ids)?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for KeyRotationGuard<'_> {
    fn drop(&mut self) {
        if !self.committed {
            self.kms
                .rollback_key_rotation(&self.previous_key_id, &self.staged_key_id);
        }
    }
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
        let plaintext =
            Zeroizing::new(serde_json::to_vec(data).context("Failed to serialize session data")?);
        let kms = self.kms.lock().await;
        let (ciphertext, key_id) = kms.encrypt(&plaintext).await?;
        let mut storage = self.storage.lock().await;
        storage.insert(session_id.to_string(), (ciphertext, key_id));
        Ok(())
    }

    async fn load_session(&self, session_id: &str) -> Result<Option<SessionData>> {
        {
            let storage = self.storage.lock().await;
            if !storage.contains_key(session_id) {
                return Ok(None);
            }
        }
        let kms = self.kms.lock().await;
        let storage = self.storage.lock().await;
        let (ciphertext, key_id) = match storage.get(session_id) {
            Some(entry) => entry,
            None => return Ok(None),
        };

        let plaintext = Zeroizing::new(kms.decrypt(ciphertext, key_id).await?);
        let data =
            serde_json::from_slice(&plaintext).context("Failed to deserialize session data")?;
        Ok(Some(data))
    }

    async fn rotate_key(&self, new_key: [u8; 32], new_key_id: String) -> Result<()> {
        let mut kms_guard = self.kms.lock().await;
        let mut storage = self.storage.lock().await;

        let old_current_key_id = kms_guard.current_key_id();
        let mut retired_key_ids = HashSet::from([old_current_key_id.clone()]);

        // 1. Decrypt everything with current keys without mutating storage.
        let mut decrypted_items = Vec::with_capacity(storage.len());
        for (sid, (ct, kid)) in storage.iter() {
            let plaintext = Zeroizing::new(kms_guard.decrypt(ct, kid).await?);
            retired_key_ids.insert(kid.clone());
            decrypted_items.push((sid.clone(), plaintext));
        }

        // 2. Stage and activate the new key. Duplicate IDs are rejected so
        // existing key material can never be overwritten mid-rotation.
        kms_guard.stage_key_rotation(new_key, new_key_id.clone())?;
        let mut rotation =
            KeyRotationGuard::new(kms_guard.as_mut(), old_current_key_id, new_key_id.clone());

        // 3. Build the complete replacement map before committing any entry.
        let mut reencrypted = HashMap::with_capacity(decrypted_items.len());
        for (sid, pt) in decrypted_items {
            let encrypted = rotation.kms.encrypt(&pt).await;
            let (ciphertext, key_id) = match encrypted {
                Ok(encrypted) => encrypted,
                Err(err) => return Err(err).context("Failed to re-encrypt stored sessions"),
            };
            reencrypted.insert(sid, (ciphertext, key_id));
        }

        // 4. Retire superseded keys atomically before the infallible storage
        // swap, so an error cannot expose a partially committed rotation.
        let retired_key_ids: Vec<String> = retired_key_ids
            .into_iter()
            .filter(|key_id| key_id != &new_key_id)
            .collect();
        rotation
            .commit(&retired_key_ids)
            .context("Failed to retire superseded session keys")?;
        *storage = reencrypted;

        Ok(())
    }
}

/// Software-based KMS implementation using local keys.
pub struct SoftwareKms {
    keys: HashMap<String, Zeroizing<[u8; 32]>>,
    current_key_id: String,
}

impl SoftwareKms {
    /// Creates a new SoftwareKms with an initial key.
    pub fn new(key: [u8; 32], key_id: String) -> Self {
        let mut keys = HashMap::new();
        keys.insert(key_id.clone(), Zeroizing::new(key));
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
        let cipher = Aes256Gcm::new_from_slice(key.as_ref())
            .expect("SoftwareKms always stores 32-byte AES-256 keys");
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

        let cipher = Aes256Gcm::new_from_slice(key.as_ref())
            .expect("SoftwareKms always stores 32-byte AES-256 keys");
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
        let key = Zeroizing::new(key);
        self.keys.insert(key_id.clone(), key);
        if make_current {
            self.current_key_id = key_id;
        }
    }

    fn stage_key_rotation(&mut self, key: [u8; 32], key_id: String) -> Result<()> {
        let key = Zeroizing::new(key);
        if self.keys.contains_key(&key_id) {
            anyhow::bail!("Key ID already exists: {key_id}");
        }
        self.keys.insert(key_id.clone(), key);
        self.current_key_id = key_id;
        Ok(())
    }

    fn rollback_key_rotation(&mut self, previous_key_id: &str, staged_key_id: &str) {
        if self.keys.contains_key(previous_key_id) {
            self.current_key_id = previous_key_id.to_string();
        }
        self.keys.remove(staged_key_id);
    }

    fn finalize_key_rotation(&mut self, key_ids: &[String]) -> Result<()> {
        for key_id in key_ids {
            if self.current_key_id == *key_id {
                anyhow::bail!("Cannot remove the current key: {key_id}");
            }
            if !self.keys.contains_key(key_id) {
                anyhow::bail!("Key not found: {key_id}");
            }
        }
        for key_id in key_ids {
            self.keys.remove(key_id);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FailOnEncryptKms {
        inner: SoftwareKms,
        encrypt_calls: AtomicUsize,
        fail_on_call: usize,
    }

    struct FailOnRetireKms {
        inner: SoftwareKms,
        fail_once: bool,
    }

    struct PendingOnRotatedKeyKms {
        inner: SoftwareKms,
    }

    struct LegacyKms {
        inner: SoftwareKms,
    }

    #[async_trait]
    impl KmsAdapter for LegacyKms {
        async fn encrypt(&self, plaintext: &[u8]) -> Result<(Vec<u8>, String)> {
            self.inner.encrypt(plaintext).await
        }

        async fn decrypt(&self, ciphertext: &[u8], key_id: &str) -> Result<Vec<u8>> {
            self.inner.decrypt(ciphertext, key_id).await
        }

        fn current_key_id(&self) -> String {
            self.inner.current_key_id()
        }

        fn add_key(&mut self, key: [u8; 32], key_id: String, make_current: bool) {
            self.inner.add_key(key, key_id, make_current);
        }
    }

    #[async_trait]
    impl KmsAdapter for PendingOnRotatedKeyKms {
        async fn encrypt(&self, plaintext: &[u8]) -> Result<(Vec<u8>, String)> {
            if self.inner.current_key_id() == "key-2" {
                std::future::pending().await
            } else {
                self.inner.encrypt(plaintext).await
            }
        }

        async fn decrypt(&self, ciphertext: &[u8], key_id: &str) -> Result<Vec<u8>> {
            self.inner.decrypt(ciphertext, key_id).await
        }

        fn current_key_id(&self) -> String {
            self.inner.current_key_id()
        }

        fn add_key(&mut self, key: [u8; 32], key_id: String, make_current: bool) {
            self.inner.add_key(key, key_id, make_current);
        }

        fn stage_key_rotation(&mut self, key: [u8; 32], key_id: String) -> Result<()> {
            self.inner.stage_key_rotation(key, key_id)
        }

        fn rollback_key_rotation(&mut self, previous_key_id: &str, staged_key_id: &str) {
            self.inner
                .rollback_key_rotation(previous_key_id, staged_key_id);
        }

        fn finalize_key_rotation(&mut self, key_ids: &[String]) -> Result<()> {
            self.inner.finalize_key_rotation(key_ids)
        }
    }

    #[async_trait]
    impl KmsAdapter for FailOnRetireKms {
        async fn encrypt(&self, plaintext: &[u8]) -> Result<(Vec<u8>, String)> {
            self.inner.encrypt(plaintext).await
        }

        async fn decrypt(&self, ciphertext: &[u8], key_id: &str) -> Result<Vec<u8>> {
            self.inner.decrypt(ciphertext, key_id).await
        }

        fn current_key_id(&self) -> String {
            self.inner.current_key_id()
        }

        fn add_key(&mut self, key: [u8; 32], key_id: String, make_current: bool) {
            self.inner.add_key(key, key_id, make_current);
        }

        fn stage_key_rotation(&mut self, key: [u8; 32], key_id: String) -> Result<()> {
            self.inner.stage_key_rotation(key, key_id)
        }

        fn rollback_key_rotation(&mut self, previous_key_id: &str, staged_key_id: &str) {
            self.inner
                .rollback_key_rotation(previous_key_id, staged_key_id);
        }

        fn finalize_key_rotation(&mut self, key_ids: &[String]) -> Result<()> {
            if self.fail_once {
                self.fail_once = false;
                anyhow::bail!("injected key retirement failure");
            }
            self.inner.finalize_key_rotation(key_ids)
        }
    }

    #[async_trait]
    impl KmsAdapter for FailOnEncryptKms {
        async fn encrypt(&self, plaintext: &[u8]) -> Result<(Vec<u8>, String)> {
            let call = self.encrypt_calls.fetch_add(1, Ordering::SeqCst) + 1;
            if call == self.fail_on_call {
                anyhow::bail!("injected encryption failure");
            }
            self.inner.encrypt(plaintext).await
        }

        async fn decrypt(&self, ciphertext: &[u8], key_id: &str) -> Result<Vec<u8>> {
            self.inner.decrypt(ciphertext, key_id).await
        }

        fn current_key_id(&self) -> String {
            self.inner.current_key_id()
        }

        fn add_key(&mut self, key: [u8; 32], key_id: String, make_current: bool) {
            self.inner.add_key(key, key_id, make_current);
        }

        fn stage_key_rotation(&mut self, key: [u8; 32], key_id: String) -> Result<()> {
            self.inner.stage_key_rotation(key, key_id)
        }

        fn rollback_key_rotation(&mut self, previous_key_id: &str, staged_key_id: &str) {
            self.inner
                .rollback_key_rotation(previous_key_id, staged_key_id);
        }

        fn finalize_key_rotation(&mut self, key_ids: &[String]) -> Result<()> {
            self.inner.finalize_key_rotation(key_ids)
        }
    }

    fn sample_session(domain: &str) -> SessionData {
        SessionData {
            domain: domain.to_string(),
            cookies: Vec::new(),
            tokens: HashMap::new(),
        }
    }

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
        let (old_ciphertext, old_key_id) = vault
            .storage
            .lock()
            .await
            .get(session_id)
            .cloned()
            .expect("stored session");

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
        drop(storage);

        let kms = vault.kms.lock().await;
        assert!(
            kms.decrypt(&old_ciphertext, &old_key_id).await.is_err(),
            "the superseded key must be retired after every session is re-encrypted"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_key_rotation_retires_the_old_key_for_an_empty_vault() -> Result<()> {
        let kms = Box::new(SoftwareKms::new([1u8; 32], "key-1".to_string()));
        let vault = LocalSessionVault::new(kms);
        let (old_ciphertext, old_key_id) = {
            let kms = vault.kms.lock().await;
            kms.encrypt(b"secret").await?
        };

        vault.rotate_key([2u8; 32], "key-2".to_string()).await?;

        let kms = vault.kms.lock().await;
        assert!(
            kms.decrypt(&old_ciphertext, &old_key_id).await.is_err(),
            "rotation must retire the previous current key even when storage is empty"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_failed_key_rotation_leaves_storage_and_current_key_unchanged() -> Result<()> {
        let kms = Box::new(FailOnEncryptKms {
            inner: SoftwareKms::new([1u8; 32], "key-1".to_string()),
            encrypt_calls: AtomicUsize::new(0),
            fail_on_call: 4,
        });
        let vault = LocalSessionVault::new(kms);
        vault
            .store_session("first", &sample_session("first.example"))
            .await?;
        vault
            .store_session("second", &sample_session("second.example"))
            .await?;
        let storage_before = vault.storage.lock().await.clone();

        let result = vault.rotate_key([2u8; 32], "key-2".to_string()).await;

        assert!(
            result.is_err(),
            "the injected encryption failure must surface"
        );
        assert_eq!(
            *vault.storage.lock().await,
            storage_before,
            "rotation must commit the re-encrypted map atomically"
        );
        assert_eq!(vault.kms.lock().await.current_key_id(), "key-1");

        Ok(())
    }

    #[tokio::test]
    async fn test_failed_key_retirement_rolls_back_the_staged_rotation() -> Result<()> {
        let kms = Box::new(FailOnRetireKms {
            inner: SoftwareKms::new([1u8; 32], "key-1".to_string()),
            fail_once: true,
        });
        let vault = LocalSessionVault::new(kms);
        vault
            .store_session("session", &sample_session("example.com"))
            .await?;
        let storage_before = vault.storage.lock().await.clone();

        let result = vault.rotate_key([2u8; 32], "key-2".to_string()).await;

        assert!(result.is_err(), "the retirement failure must surface");
        assert_eq!(*vault.storage.lock().await, storage_before);
        assert_eq!(vault.kms.lock().await.current_key_id(), "key-1");

        Ok(())
    }

    #[tokio::test]
    async fn test_cancelled_key_rotation_rolls_back_the_staged_key() -> Result<()> {
        let kms = Box::new(PendingOnRotatedKeyKms {
            inner: SoftwareKms::new([1u8; 32], "key-1".to_string()),
        });
        let vault = LocalSessionVault::new(kms);
        let session = sample_session("example.com");
        vault.store_session("session", &session).await?;

        let result = tokio::time::timeout(
            std::time::Duration::from_millis(10),
            vault.rotate_key([2u8; 32], "key-2".to_string()),
        )
        .await;

        assert!(result.is_err(), "rotation should be cancelled by timeout");
        assert_eq!(vault.kms.lock().await.current_key_id(), "key-1");
        assert_eq!(
            vault
                .load_session("session")
                .await?
                .expect("stored session")
                .domain,
            session.domain
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_legacy_kms_adapter_rejects_rotation_without_mutating_state() -> Result<()> {
        let kms = Box::new(LegacyKms {
            inner: SoftwareKms::new([1u8; 32], "key-1".to_string()),
        });
        let vault = LocalSessionVault::new(kms);

        let error = vault
            .rotate_key([2u8; 32], "key-2".to_string())
            .await
            .expect_err("legacy adapters must fail closed");

        assert!(error.to_string().contains("does not support"));
        assert_eq!(vault.kms.lock().await.current_key_id(), "key-1");
        Ok(())
    }
}
