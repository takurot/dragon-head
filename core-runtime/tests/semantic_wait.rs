use std::time::{Duration, Instant};

use core_runtime::{
    sre::{normalize_dom, LoadProfile, SemanticState},
    BrowserClient, SemanticTarget, SemanticWaitOptions, SemanticWaitState, WaitError,
};

#[test]
fn test_wait_for_semantic_enabled_on_delayed_button() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    let html = r#"
        <html>
            <body>
                <button id="btn_login" disabled>Login</button>
                <script>
                    setTimeout(() => {
                        document.getElementById("btn_login").removeAttribute("disabled");
                    }, 250);
                </script>
            </body>
        </html>
    "#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url)?;

    let root = page.get_document_node()?;
    let sem = normalize_dom(LoadProfile::Interactive, &root)?;
    let state = SemanticState::new(sem, LoadProfile::Interactive);
    let stable_key = find_button_key(state.root()).expect("button stable_key should exist");

    page.wait_for_semantic_with_options(
        SemanticTarget::StableKey(stable_key),
        SemanticWaitState::Enabled,
        Duration::from_secs(2),
        SemanticWaitOptions {
            load_profile: LoadProfile::Interactive,
            ..Default::default()
        },
    )?;

    let is_disabled = page
        .evaluate_script(r#"document.getElementById("btn_login").hasAttribute("disabled")"#)?
        .value
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    assert!(!is_disabled, "button must become enabled");

    Ok(())
}

#[test]
fn test_wait_for_intent_success() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    let html = r#"
        <html>
            <body>
                <div id="status">Pending</div>
                <script>
                    setTimeout(() => {
                        document.body.setAttribute("data-intent", "checkout_complete");
                    }, 200);
                </script>
            </body>
        </html>
    "#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url)?;

    page.wait_for_intent_with_options(
        "checkout_complete",
        Duration::from_secs(2),
        SemanticWaitOptions {
            load_profile: LoadProfile::Interactive,
            ..Default::default()
        },
    )?;
    Ok(())
}

#[test]
fn test_wait_for_intent_timeout() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    let html = r#"
        <html>
            <body>
                <div>No completion marker</div>
            </body>
        </html>
    "#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url)?;

    let start = Instant::now();
    let result = page.wait_for_intent("checkout_complete", Duration::from_millis(400));
    let elapsed = start.elapsed();

    assert!(result.is_err(), "wait_for_intent should time out");
    let err = result.unwrap_err();
    let wait_err = err
        .downcast_ref::<WaitError>()
        .expect("error should be WaitError");
    assert!(matches!(wait_err, WaitError::Timeout { .. }));

    assert!(
        elapsed >= Duration::from_millis(400) && elapsed < Duration::from_secs(2),
        "timeout must respect configured threshold"
    );

    Ok(())
}

#[test]
fn test_wait_for_intent_does_not_match_substring() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    let html = r#"
        <html>
            <body>
                <div>checkout_complete_failed</div>
            </body>
        </html>
    "#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url)?;

    let start = Instant::now();
    let result = page.wait_for_intent("checkout_complete", Duration::from_millis(300));
    let elapsed = start.elapsed();

    assert!(result.is_err(), "substring match must not satisfy intent");
    let wait_err = result
        .unwrap_err()
        .downcast::<WaitError>()
        .expect("error should be WaitError");
    assert!(matches!(wait_err, WaitError::Timeout { .. }));
    assert!(
        elapsed >= Duration::from_millis(300) && elapsed < Duration::from_secs(2),
        "timeout must respect configured threshold"
    );

    Ok(())
}

#[test]
fn test_wait_for_semantic_timeout_when_target_never_enabled() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    let html = r#"
        <html>
            <body>
                <button id="btn_login" disabled>Login</button>
            </body>
        </html>
    "#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url)?;

    let root = page.get_document_node()?;
    let sem = normalize_dom(LoadProfile::Minimal, &root)?;
    let state = SemanticState::new(sem, LoadProfile::Minimal);
    let stable_key = find_button_key(state.root()).expect("button stable_key should exist");

    let start = Instant::now();
    let result = page.wait_for_semantic(
        SemanticTarget::StableKey(stable_key),
        SemanticWaitState::Enabled,
        Duration::from_millis(350),
    );
    let elapsed = start.elapsed();

    assert!(result.is_err(), "wait_for_semantic should time out");
    let wait_err = result
        .unwrap_err()
        .downcast::<WaitError>()
        .expect("error should be WaitError");
    assert!(matches!(wait_err, WaitError::Timeout { .. }));
    assert!(
        elapsed >= Duration::from_millis(350) && elapsed < Duration::from_secs(2),
        "timeout must respect configured threshold"
    );

    Ok(())
}

#[test]
fn test_wait_for_semantic_id_fallback_with_stable_key() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    let html = r#"
        <html>
            <body>
                <div id="container">
                    <button id="btn_login" disabled>Login</button>
                </div>
                <script>
                    function rerenderAndEnable() {
                        const container = document.getElementById("container");
                        container.innerHTML = '<button id="btn_login" disabled>Login</button>';
                        setTimeout(() => {
                            document.getElementById("btn_login").removeAttribute("disabled");
                        }, 200);
                    }
                    setTimeout(rerenderAndEnable, 120);
                </script>
            </body>
        </html>
    "#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url)?;

    let root = page.get_document_node()?;
    let sem = normalize_dom(LoadProfile::Minimal, &root)?;
    let state = SemanticState::new(sem, LoadProfile::Minimal);
    let (old_id, stable_key) = find_button_info(state.root()).expect("button should exist");

    page.wait_for_semantic(
        SemanticTarget::IdWithStableKey {
            id: old_id,
            stable_key,
        },
        SemanticWaitState::Enabled,
        Duration::from_secs(2),
    )?;

    let is_disabled = page
        .evaluate_script(r#"document.getElementById("btn_login").hasAttribute("disabled")"#)?
        .value
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    assert!(!is_disabled, "button must become enabled after rerender");

    Ok(())
}

#[test]
fn test_wait_for_semantic_is_not_blocked_by_large_poll_interval() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    let html = r#"
        <html>
            <body>
                <button id="btn_login" disabled>Login</button>
                <script>
                    setTimeout(() => {
                        document.getElementById("btn_login").removeAttribute("disabled");
                    }, 700);
                </script>
            </body>
        </html>
    "#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url)?;

    let root = page.get_document_node()?;
    let sem = normalize_dom(LoadProfile::Interactive, &root)?;
    let state = SemanticState::new(sem, LoadProfile::Interactive);
    let stable_key = find_button_key(state.root()).expect("button stable_key should exist");

    let started = Instant::now();
    page.wait_for_semantic_with_options(
        SemanticTarget::StableKey(stable_key),
        SemanticWaitState::Enabled,
        Duration::from_secs(7),
        SemanticWaitOptions {
            load_profile: LoadProfile::Interactive,
            poll_interval: Duration::from_secs(5),
        },
    )?;

    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(3),
        "wait_for_semantic should not be blocked by poll_interval: elapsed={elapsed:?}"
    );

    Ok(())
}

#[test]
fn test_wait_for_intent_is_not_blocked_by_large_poll_interval() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    let html = r#"
        <html>
            <body>
                <div id="status">Pending</div>
                <script>
                    setTimeout(() => {
                        document.body.setAttribute("data-intent", "checkout_complete");
                    }, 700);
                </script>
            </body>
        </html>
    "#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url)?;

    let started = Instant::now();
    page.wait_for_intent_with_options(
        "checkout_complete",
        Duration::from_secs(7),
        SemanticWaitOptions {
            load_profile: LoadProfile::Interactive,
            poll_interval: Duration::from_secs(5),
        },
    )?;

    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(3),
        "wait_for_intent should not be blocked by poll_interval: elapsed={elapsed:?}"
    );

    Ok(())
}

#[test]
fn test_wait_for_semantic_recovers_from_polluted_bridge_state() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    let html = r#"
        <html>
            <body>
                <button id="btn_login" disabled>Login</button>
                <script>
                    setTimeout(() => {
                        document.getElementById("btn_login").removeAttribute("disabled");
                    }, 220);
                </script>
            </body>
        </html>
    "#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url)?;

    page.evaluate_script(
        r#"window[Symbol.for("neural_browser.runtime.sre_event_bridge")] = { version: "bad", waiters: "bad" }"#,
    )?;

    let root = page.get_document_node()?;
    let sem = normalize_dom(LoadProfile::Interactive, &root)?;
    let state = SemanticState::new(sem, LoadProfile::Interactive);
    let stable_key = find_button_key(state.root()).expect("button stable_key should exist");

    page.wait_for_semantic_with_options(
        SemanticTarget::StableKey(stable_key),
        SemanticWaitState::Enabled,
        Duration::from_secs(2),
        SemanticWaitOptions {
            load_profile: LoadProfile::Interactive,
            ..Default::default()
        },
    )?;

    let is_disabled = page
        .evaluate_script(r#"document.getElementById("btn_login").hasAttribute("disabled")"#)?
        .value
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    assert!(!is_disabled, "button must become enabled");

    Ok(())
}

fn find_button_key(node: &core_runtime::sre::SemanticNode) -> Option<String> {
    if node.role == "button" {
        return node.stable_key.clone();
    }
    for child in &node.children {
        if let Some(key) = find_button_key(child) {
            return Some(key);
        }
    }
    None
}

fn find_button_info(node: &core_runtime::sre::SemanticNode) -> Option<(i64, String)> {
    if node.role == "button" {
        return Some((
            node.backend_node_id,
            node.stable_key.clone().unwrap_or_default(),
        ));
    }
    for child in &node.children {
        if let Some(info) = find_button_info(child) {
            return Some(info);
        }
    }
    None
}
