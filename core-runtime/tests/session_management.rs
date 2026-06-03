use std::sync::{Arc, Mutex};

use anyhow::Context;

use core_runtime::{BrowserClient, KmsAdapter, LocalSessionVault, SessionVault, SoftwareKms};

#[tokio::test]
async fn test_session_management_cross_domain_save_restore() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    page.navigate("https://example.com")?;
    set_cookie(&page, "session_com", "alpha")?;
    assert_eq!(
        cookie_value(&page, "session_com")?.as_deref(),
        Some("alpha")
    );
    page.save_to_vault("session-example-com").await?;

    page.navigate("https://example.org")?;
    set_cookie(&page, "session_org", "bravo")?;
    assert_eq!(
        cookie_value(&page, "session_org")?.as_deref(),
        Some("bravo")
    );
    page.save_to_vault("session-example-org").await?;

    page.navigate("https://example.com")?;
    clear_cookie(&page, "session_com")?;
    assert!(
        cookie_value(&page, "session_com")?.is_none(),
        "cookie must be cleared before restore"
    );
    page.load_from_vault("session-example-com").await?;
    assert_eq!(
        cookie_value(&page, "session_com")?.as_deref(),
        Some("alpha")
    );
    assert!(
        cookie_value(&page, "session_org")?.is_none(),
        "cross-domain cookie must not leak into example.com"
    );

    page.navigate("https://example.org")?;
    clear_cookie(&page, "session_org")?;
    assert!(
        cookie_value(&page, "session_org")?.is_none(),
        "cookie must be cleared before restore"
    );
    page.load_from_vault("session-example-org").await?;
    assert_eq!(
        cookie_value(&page, "session_org")?.as_deref(),
        Some("bravo")
    );
    assert!(
        cookie_value(&page, "session_com")?.is_none(),
        "cross-domain cookie must not leak into example.org"
    );

    Ok(())
}

#[tokio::test]
async fn test_session_management_key_rotation_restore_roundtrip() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let kms_log = Arc::new(Mutex::new(KmsCallLog::default()));
    let vault = Arc::new(LocalSessionVault::new(Box::new(RecordingKms::new(
        [1u8; 32],
        "key-1".to_string(),
        Arc::clone(&kms_log),
    ))));
    let client = BrowserClient::new_with_vault(vault.clone())?;
    let page = client.new_page()?;

    page.navigate("https://example.com")?;
    set_cookie(&page, "rotate_cookie", "before_rotation")?;
    page.save_to_vault("rotation-session").await?;
    assert_eq!(
        last_encrypt_key_id(&kms_log).as_deref(),
        Some("key-1"),
        "pre-rotation saves must use the original key"
    );

    vault.rotate_key([2u8; 32], "key-2".to_string()).await?;
    clear_decrypt_log(&kms_log);

    let restored_page = client.new_page()?;
    restored_page.navigate("https://example.com")?;
    clear_cookie(&restored_page, "rotate_cookie")?;
    assert!(
        cookie_value(&restored_page, "rotate_cookie")?.is_none(),
        "cookie should be absent before restore"
    );
    restored_page.load_from_vault("rotation-session").await?;
    assert_eq!(
        cookie_value(&restored_page, "rotate_cookie")?.as_deref(),
        Some("before_rotation")
    );
    assert_eq!(
        last_decrypt_key_id(&kms_log).as_deref(),
        Some("key-2"),
        "restores after rotation must decrypt with the rotated key"
    );

    set_cookie(&restored_page, "rotate_cookie", "after_rotation")?;
    restored_page.save_to_vault("rotation-session").await?;
    assert_eq!(
        last_encrypt_key_id(&kms_log).as_deref(),
        Some("key-2"),
        "post-rotation saves must use the rotated key"
    );

    clear_decrypt_log(&kms_log);
    let verifier_page = client.new_page()?;
    verifier_page.navigate("https://example.com")?;
    clear_cookie(&verifier_page, "rotate_cookie")?;
    verifier_page.load_from_vault("rotation-session").await?;
    assert_eq!(
        cookie_value(&verifier_page, "rotate_cookie")?.as_deref(),
        Some("after_rotation")
    );
    assert_eq!(
        last_decrypt_key_id(&kms_log).as_deref(),
        Some("key-2"),
        "updated sessions must remain readable with the rotated key"
    );

    Ok(())
}

fn set_cookie(page: &core_runtime::PageSession, name: &str, value: &str) -> anyhow::Result<()> {
    let script = format!(
        r#"document.cookie = "{}={}; path=/; max-age=3600";"#,
        name, value
    );
    page.evaluate_script(&script)?;
    Ok(())
}

fn clear_cookie(page: &core_runtime::PageSession, name: &str) -> anyhow::Result<()> {
    let script = format!(
        r#"document.cookie = "{}=; path=/; expires=Thu, 01 Jan 1970 00:00:00 GMT";"#,
        name
    );
    page.evaluate_script(&script)?;
    Ok(())
}

fn cookie_value(page: &core_runtime::PageSession, name: &str) -> anyhow::Result<Option<String>> {
    let raw = page
        .evaluate_script("document.cookie")?
        .value
        .and_then(|v| v.as_str().map(ToOwned::to_owned))
        .context("document.cookie should return a string")?;

    Ok(raw.split(';').find_map(|entry| {
        let mut parts = entry.trim().splitn(2, '=');
        let key = parts.next()?;
        let value = parts.next().unwrap_or_default();
        (key == name).then(|| value.to_string())
    }))
}

#[derive(Debug, Default)]
struct KmsCallLog {
    encrypt_key_ids: Vec<String>,
    decrypt_key_ids: Vec<String>,
}

struct RecordingKms {
    inner: SoftwareKms,
    log: Arc<Mutex<KmsCallLog>>,
}

impl RecordingKms {
    fn new(key: [u8; 32], key_id: String, log: Arc<Mutex<KmsCallLog>>) -> Self {
        Self {
            inner: SoftwareKms::new(key, key_id),
            log,
        }
    }
}

#[async_trait::async_trait]
impl KmsAdapter for RecordingKms {
    async fn encrypt(&self, plaintext: &[u8]) -> anyhow::Result<(Vec<u8>, String)> {
        let (ciphertext, key_id) = self.inner.encrypt(plaintext).await?;
        self.log
            .lock()
            .expect("kms log lock should not be poisoned")
            .encrypt_key_ids
            .push(key_id.clone());
        Ok((ciphertext, key_id))
    }

    async fn decrypt(&self, ciphertext: &[u8], key_id: &str) -> anyhow::Result<Vec<u8>> {
        self.log
            .lock()
            .expect("kms log lock should not be poisoned")
            .decrypt_key_ids
            .push(key_id.to_string());
        self.inner.decrypt(ciphertext, key_id).await
    }

    fn current_key_id(&self) -> String {
        self.inner.current_key_id()
    }

    fn add_key(&mut self, key: [u8; 32], key_id: String, make_current: bool) {
        self.inner.add_key(key, key_id, make_current);
    }
}

fn clear_decrypt_log(log: &Arc<Mutex<KmsCallLog>>) {
    log.lock()
        .expect("kms log lock should not be poisoned")
        .decrypt_key_ids
        .clear();
}

fn last_encrypt_key_id(log: &Arc<Mutex<KmsCallLog>>) -> Option<String> {
    log.lock()
        .expect("kms log lock should not be poisoned")
        .encrypt_key_ids
        .last()
        .cloned()
}

fn last_decrypt_key_id(log: &Arc<Mutex<KmsCallLog>>) -> Option<String> {
    log.lock()
        .expect("kms log lock should not be poisoned")
        .decrypt_key_ids
        .last()
        .cloned()
}
