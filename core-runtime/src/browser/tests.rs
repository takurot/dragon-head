use super::*;
use helpers::{node_is_enabled, normalized_poll_interval};

#[derive(Default)]
struct RecordingInterceptionControl {
    operations: Mutex<Vec<&'static str>>,
    fail_at: Option<&'static str>,
}

impl RecordingInterceptionControl {
    fn failing_at(operation: &'static str) -> Self {
        Self {
            operations: Mutex::new(Vec::new()),
            fail_at: Some(operation),
        }
    }

    fn record(&self, operation: &'static str) -> Result<()> {
        self.operations.lock().unwrap().push(operation);
        if self.fail_at == Some(operation) {
            anyhow::bail!("injected {operation} failure");
        }
        Ok(())
    }

    fn recorded(&self) -> Vec<&'static str> {
        self.operations.lock().unwrap().clone()
    }
}

impl NavigationInterceptionControl for RecordingInterceptionControl {
    fn install_interceptor(&self, _interceptor: Arc<NavigationRequestInterceptor>) -> Result<()> {
        self.record("install_interceptor")
    }

    fn enable_fetch(
        &self,
        _patterns: &[headless_chrome::protocol::cdp::Fetch::RequestPattern],
    ) -> Result<()> {
        self.record("enable_fetch")
    }

    fn restore_default_interceptor(&self) -> Result<()> {
        self.record("restore_default_interceptor")
    }

    fn disable_fetch(&self) -> Result<()> {
        self.record("disable_fetch")
    }
}

fn recording_interceptor() -> Arc<impl headless_chrome::browser::tab::RequestInterceptor> {
    Arc::new(
        |_: Arc<headless_chrome::browser::transport::Transport>,
         _: headless_chrome::browser::transport::SessionId,
         _: headless_chrome::protocol::cdp::Fetch::events::RequestPausedEvent| {
            headless_chrome::browser::tab::RequestPausedDecision::Continue(None)
        },
    )
}

#[test]
fn navigation_interception_partial_enable_failure_restores_and_disables() {
    let control = Arc::new(RecordingInterceptionControl::failing_at("enable_fetch"));
    let result = NavigationInterceptionGuard::install_with_control(
        control.clone(),
        recording_interceptor(),
        &[],
    );

    assert!(result.is_err());
    assert_eq!(
        control.recorded(),
        vec![
            "install_interceptor",
            "enable_fetch",
            "restore_default_interceptor",
            "disable_fetch"
        ]
    );
}

#[test]
fn navigation_interception_success_finishes_restore_before_disable_once() {
    let control = Arc::new(RecordingInterceptionControl::default());
    let mut guard = NavigationInterceptionGuard::install_with_control(
        control.clone(),
        recording_interceptor(),
        &[],
    )
    .expect("install interception");

    guard.finish().expect("finish interception");
    guard.finish().expect("second finish is idempotent");
    drop(guard);

    assert_eq!(
        control.recorded(),
        vec![
            "install_interceptor",
            "enable_fetch",
            "restore_default_interceptor",
            "disable_fetch"
        ]
    );
}

#[test]
fn navigation_interception_finish_attempts_disable_after_cleanup_errors() {
    for failure in ["restore_default_interceptor", "disable_fetch"] {
        let control = Arc::new(RecordingInterceptionControl::failing_at(failure));
        let mut guard = NavigationInterceptionGuard::install_with_control(
            control.clone(),
            recording_interceptor(),
            &[],
        )
        .expect("install interception");

        assert!(guard.finish().is_err(), "failure point: {failure}");
        assert_eq!(
            control.recorded(),
            vec![
                "install_interceptor",
                "enable_fetch",
                "restore_default_interceptor",
                "disable_fetch"
            ],
            "failure point: {failure}"
        );
    }
}

#[test]
fn test_state_contains_intent_exact_match() {
    let node = SemanticNode {
        role: "body".to_string(),
        attributes: Some(std::collections::BTreeMap::from([(
            "data-intent".to_string(),
            "checkout_complete".to_string(),
        )])),
        ..Default::default()
    };

    assert!(state_contains_intent(&node, "checkout_complete"));
    assert!(!state_contains_intent(&node, "signup_complete"));
}

#[test]
fn test_node_is_enabled_with_disabled_attribute() {
    let disabled = SemanticNode {
        role: "button".to_string(),
        attributes: Some(std::collections::BTreeMap::from([(
            "disabled".to_string(),
            "".to_string(),
        )])),
        ..Default::default()
    };
    let enabled = SemanticNode {
        role: "button".to_string(),
        attributes: Some(std::collections::BTreeMap::from([(
            "id".to_string(),
            "btn_login".to_string(),
        )])),
        ..Default::default()
    };

    assert!(!node_is_enabled(&disabled));
    assert!(node_is_enabled(&enabled));
}

#[test]
fn test_transient_capture_error_detection() {
    let transient = anyhow::anyhow!("Execution context was destroyed while loading");
    let non_transient = anyhow::anyhow!("Unsupported action: drag");
    let closed = anyhow::anyhow!("Target closed");

    assert!(is_transient_capture_error(&transient));
    assert!(!is_transient_capture_error(&non_transient));
    assert!(!is_transient_capture_error(&closed));
}

#[test]
fn test_state_contains_intent_is_exact_match() {
    let node = SemanticNode {
        role: "text".to_string(),
        label: Some("checkout_complete_failed".to_string()),
        ..Default::default()
    };

    assert!(!state_contains_intent(&node, "checkout_complete"));
}

#[test]
fn test_normalized_poll_interval_uses_default_for_zero() {
    assert_eq!(
        normalized_poll_interval(Duration::ZERO),
        DEFAULT_WAIT_POLL_INTERVAL
    );
}

#[test]
fn test_normalized_poll_interval_keeps_non_zero_value() {
    let interval = Duration::from_millis(123);
    assert_eq!(normalized_poll_interval(interval), interval);
}

#[test]
fn test_transient_error_backoff_is_capped() {
    assert_eq!(
        transient_error_backoff(Duration::from_secs(5)),
        MAX_TRANSIENT_ERROR_BACKOFF
    );
    assert_eq!(
        transient_error_backoff(Duration::from_millis(80)),
        Duration::from_millis(80)
    );
}

#[test]
fn test_navigation_fallback_condition_met_when_requested_url_reached() {
    assert!(navigation_fallback_condition_met(
        "https://example.com/next",
        Some("https://example.com/current"),
        Some("https://example.com/next"),
        Some("complete"),
        true,
    ));
}

#[test]
fn test_navigation_fallback_condition_met_when_url_changes_and_dom_available() {
    assert!(navigation_fallback_condition_met(
        "https://example.com/requested",
        Some("https://example.com/current"),
        Some("https://example.com/redirected"),
        None,
        true,
    ));
}

#[test]
fn test_navigation_fallback_condition_not_met_when_url_unchanged() {
    assert!(!navigation_fallback_condition_met(
        "https://example.com/target",
        Some("https://example.com/current"),
        Some("https://example.com/current"),
        Some("complete"),
        true,
    ));
}

#[test]
fn test_navigation_fallback_condition_not_met_without_dom_readiness() {
    assert!(!navigation_fallback_condition_met(
        "https://example.com/requested",
        Some("https://example.com/current"),
        Some("https://example.com/redirected"),
        Some("loading"),
        false,
    ));
}

#[test]
fn test_normalize_dirty_paths_deduplicates_and_trims() {
    let dirty_paths = vec![
        " root/#document/html/body/div ".to_string(),
        "root/#document/html/body/div".to_string(),
        String::new(),
        "   ".to_string(),
    ];

    let normalized = normalize_dirty_paths(&dirty_paths);
    assert_eq!(normalized.len(), 1);
    assert!(normalized.contains("root/#document/html/body/div"));
}

#[test]
fn test_build_semantic_path_index_uses_unique_paths_only() {
    let tree = SemanticNode {
        role: "#document".to_string(),
        children: vec![SemanticNode {
            role: "html".to_string(),
            children: vec![SemanticNode {
                role: "body".to_string(),
                children: vec![
                    SemanticNode {
                        role: "button".to_string(),
                        label: Some("first".to_string()),
                        ..Default::default()
                    },
                    SemanticNode {
                        role: "button".to_string(),
                        label: Some("second".to_string()),
                        ..Default::default()
                    },
                    SemanticNode {
                        role: "section".to_string(),
                        children: vec![SemanticNode {
                            role: "input".to_string(),
                            ..Default::default()
                        }],
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    };

    let index = build_semantic_path_index(&tree);
    assert_eq!(index.get("root/#document"), Some(&Vec::<usize>::new()));
    assert!(!index.contains_key("root/#document/html/body/button"));
    assert_eq!(
        index.get("root/#document/html/body/section/input"),
        Some(&vec![0, 0, 2, 0])
    );
}

#[test]
fn is_browser_disconnected_detects_connection_closed_error() {
    let err = anyhow::Error::new(headless_chrome::browser::ConnectionClosed {});
    assert!(is_browser_disconnected(&err));
}

#[test]
fn is_browser_disconnected_detects_connection_closed_in_chain() {
    let err = anyhow::Error::new(headless_chrome::browser::ConnectionClosed {})
        .context("Failed to capture semantic state");
    assert!(is_browser_disconnected(&err));
}

#[test]
fn is_browser_disconnected_detects_io_markers() {
    for marker in [
        "Connection is closed",
        "Broken pipe",
        "connection reset by peer",
    ] {
        let err = anyhow::anyhow!("{marker}");
        assert!(
            is_browser_disconnected(&err),
            "expected disconnect marker '{marker}' to be detected"
        );
    }
}

#[test]
fn is_browser_disconnected_ignores_page_level_errors() {
    let err = anyhow::anyhow!("Could not find node with given id");
    assert!(!is_browser_disconnected(&err));
}

// ISSUE-260: "not connected" alone is ambiguous (a single tab can lose
// its CDP session while Chrome itself stays alive), so it must not by
// itself trigger a full session teardown.
#[test]
fn is_browser_disconnected_ignores_bare_not_connected_marker() {
    let err = anyhow::anyhow!("transport not connected");
    assert!(!is_browser_disconnected(&err));
}

#[test]
fn is_browser_disconnected_detects_not_connected_with_corroboration() {
    let err = anyhow::anyhow!("tab not connected: broken pipe while writing frame");
    assert!(is_browser_disconnected(&err));
}

// ISSUE-260: "broken pipe" / "connection reset" are transport-level
// blips that callers should retry before treating as a disconnect.
#[test]
fn is_transport_error_detects_broken_pipe_and_connection_reset() {
    for marker in ["broken pipe", "connection reset by peer"] {
        let err = anyhow::anyhow!("{marker}");
        assert!(
            is_transport_error(&err),
            "expected transport marker '{marker}' to be detected"
        );
    }
}

#[test]
fn is_transport_error_ignores_definite_disconnect_markers() {
    let err = anyhow::anyhow!("connection is closed");
    assert!(!is_transport_error(&err));
}

#[test]
fn is_transport_error_ignores_page_level_errors() {
    let err = anyhow::anyhow!("Could not find node with given id");
    assert!(!is_transport_error(&err));
}

#[test]
fn confirm_alive_with_returns_true_when_probe_succeeds_quickly() {
    assert!(confirm_alive_with(|| true, Duration::from_millis(500)));
}

#[test]
fn confirm_alive_with_returns_false_when_probe_fails_quickly() {
    assert!(!confirm_alive_with(|| false, Duration::from_millis(500)));
}

#[test]
fn confirm_alive_with_returns_false_and_respects_bound_when_probe_hangs() {
    // Simulates a genuinely wedged (open-but-unresponsive) CDP call:
    // the probe never returns within the bound, so the health check
    // must report "not confirmed alive" without blocking past
    // `timeout`, independent of any internal transport default.
    let timeout = Duration::from_millis(200);
    let started = Instant::now();
    let alive = confirm_alive_with(
        || {
            thread::sleep(Duration::from_secs(5));
            true
        },
        timeout,
    );
    assert!(!alive);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "confirm_alive_with should not block past its bounded timeout, took {:?}",
        started.elapsed()
    );
}

#[test]
fn find_node_by_key_matches_full_64_char_key() {
    let full_key = "a".repeat(64);
    let node = SemanticNode {
        stable_key: Some(full_key.clone()),
        ..Default::default()
    };
    assert!(find_node_by_key(&node, &full_key).is_some());
}

#[test]
fn find_node_by_key_matches_shortened_16_char_key_against_full_key() {
    // get_state now returns only the first 16 chars; enforce_policy must still
    // resolve the node from the SemanticTree which stores full 64-char keys.
    let full_key = "abcdef1234567890".repeat(4); // 64 chars
    let short_key: String = full_key.chars().take(STABLE_KEY_SHORT_LEN).collect();
    let node = SemanticNode {
        stable_key: Some(full_key.clone()),
        ..Default::default()
    };
    assert!(
        find_node_by_key(&node, &short_key).is_some(),
        "shortened key should resolve the node for policy evaluation"
    );
}

#[test]
fn find_node_by_key_does_not_match_wrong_prefix() {
    let full_key = "abcdef1234567890".repeat(4);
    let wrong_short = "0000000000000000";
    let node = SemanticNode {
        stable_key: Some(full_key.clone()),
        ..Default::default()
    };
    assert!(find_node_by_key(&node, wrong_short).is_none());
}

#[test]
fn find_node_by_key_empty_target_key_never_matches() {
    // An empty target_key would produce cmp_len=0 and ""[..0]==""[..0] which
    // is always true — guard against that authorization bypass.
    let node = SemanticNode {
        stable_key: Some("abcdef1234567890".to_string()),
        ..Default::default()
    };
    assert!(
        find_node_by_key(&node, "").is_none(),
        "empty target_key must not match any node"
    );
}

#[test]
fn resolve_policy_target_node_resolves_via_shortened_stable_key() {
    let full_key = "deadbeefcafe0123".repeat(4);
    let short_key: String = full_key.chars().take(STABLE_KEY_SHORT_LEN).collect();

    let child = SemanticNode {
        role: "button".to_string(),
        stable_key: Some(full_key.clone()),
        ..Default::default()
    };
    let root = SemanticNode {
        role: "body".to_string(),
        children: vec![child],
        ..Default::default()
    };

    let found = resolve_policy_target_node(&root, None, Some(&short_key));
    assert!(
        found.is_some(),
        "policy target resolution must work with the 16-char short key returned by get_state"
    );
    assert_eq!(found.unwrap().role, "button");
}
