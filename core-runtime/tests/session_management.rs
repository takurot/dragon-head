use std::sync::Arc;

use anyhow::Context;
use core_runtime::{BrowserClient, LocalSessionVault, SessionVault, SoftwareKms};

fn should_skip() -> bool {
    std::env::var("CI").is_ok() && std::env::var("CHROME_INSTALLED").is_err()
}

#[tokio::test]
async fn test_session_management_cross_domain_save_restore() -> anyhow::Result<()> {
    if should_skip() {
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
    if should_skip() {
        return Ok(());
    }

    let vault = Arc::new(LocalSessionVault::new(Box::new(SoftwareKms::new(
        [1u8; 32],
        "key-1".to_string(),
    ))));
    let client = BrowserClient::new_with_vault(vault.clone())?;
    let page = client.new_page()?;

    page.navigate("https://example.com")?;
    set_cookie(&page, "rotate_cookie", "before_rotation")?;
    page.save_to_vault("rotation-session").await?;

    vault.rotate_key([2u8; 32], "key-2".to_string()).await?;

    clear_cookie(&page, "rotate_cookie")?;
    assert!(
        cookie_value(&page, "rotate_cookie")?.is_none(),
        "cookie should be absent before restore"
    );
    page.load_from_vault("rotation-session").await?;
    assert_eq!(
        cookie_value(&page, "rotate_cookie")?.as_deref(),
        Some("before_rotation")
    );

    set_cookie(&page, "rotate_cookie", "after_rotation")?;
    page.save_to_vault("rotation-session").await?;
    clear_cookie(&page, "rotate_cookie")?;
    page.load_from_vault("rotation-session").await?;
    assert_eq!(
        cookie_value(&page, "rotate_cookie")?.as_deref(),
        Some("after_rotation")
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
