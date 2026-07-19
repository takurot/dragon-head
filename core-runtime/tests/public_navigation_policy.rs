use core_runtime::{
    audit::AuditEvent,
    plugin_hooks::{PluginHookConfig, PolicyPlugin},
    policy::{ApprovalScope, PolicyAction, PolicyRule},
    sre::{normalize_dom, LoadProfile, SemanticState},
    ActionError, BrowserClient, PageSession,
};
use std::{
    collections::HashMap,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Copy)]
enum Route {
    Html(&'static str),
    Redirect(&'static str),
    RedirectLocalhost(&'static str),
}

struct TestHttpServer {
    base_url: String,
    counts: Arc<Mutex<HashMap<String, usize>>>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl TestHttpServer {
    fn start(routes: HashMap<&'static str, Route>) -> anyhow::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let counts = Arc::new(Mutex::new(HashMap::new()));
        let thread_counts = Arc::clone(&counts);
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let handle = thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        if thread_stop.load(Ordering::Relaxed) {
                            break;
                        }
                        let _ = serve_request(&mut stream, &routes, &thread_counts, address.port());
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            base_url: format!("http://{address}"),
            counts,
            stop,
            thread: Some(handle),
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    fn request_count(&self, path: &str) -> usize {
        self.counts
            .lock()
            .expect("request counts lock")
            .get(path)
            .copied()
            .unwrap_or(0)
    }
}

impl Drop for TestHttpServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(self.base_url.trim_start_matches("http://"));
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

fn serve_request(
    stream: &mut TcpStream,
    routes: &HashMap<&'static str, Route>,
    counts: &Arc<Mutex<HashMap<String, usize>>>,
    port: u16,
) -> anyhow::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    let mut request = [0_u8; 4096];
    let read = stream.read(&mut request)?;
    let request = String::from_utf8_lossy(&request[..read]);
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|target| target.split('?').next())
        .unwrap_or("/")
        .to_string();
    *counts
        .lock()
        .expect("request counts lock")
        .entry(path.clone())
        .or_default() += 1;

    match routes.get(path.as_str()).copied() {
        Some(Route::Html(body)) => write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )?,
        Some(Route::Redirect(location)) => write!(
            stream,
            "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )?,
        Some(Route::RedirectLocalhost(path)) => write!(
            stream,
            "HTTP/1.1 302 Found\r\nLocation: http://localhost:{port}{path}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )?,
        None => write!(
            stream,
            "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )?,
    }
    stream.flush()?;
    Ok(())
}

struct BlockRedirectPlugin;

impl PolicyPlugin for BlockRedirectPlugin {
    fn plugin_id(&self) -> &str {
        "block-redirect-plugin"
    }

    fn before_act(&self, intent_json: &str) -> Result<String, String> {
        let intent: serde_json::Value =
            serde_json::from_str(intent_json).map_err(|error| error.to_string())?;
        let blocked = intent["url"]
            .as_str()
            .is_some_and(|url| url.contains("/plugin-blocked"));
        Ok(if blocked {
            r#"{"allow":false,"reason":"blocked redirect"}"#.to_string()
        } else {
            r#"{"allow":true}"#.to_string()
        })
    }
}

fn navigation_rule(id: &str, path_prefix: Option<&str>, action: PolicyAction) -> PolicyRule {
    PolicyRule {
        id: id.to_string(),
        domain: None,
        path_prefix: path_prefix.map(str::to_string),
        role: None,
        text_regex: None,
        context_regex: None,
        action,
        scope: (action == PolicyAction::RequireHumanApproval).then_some(ApprovalScope::ActionOnly),
        outcome_projector: None,
    }
}

#[test]
fn public_navigation_initial_block_sends_zero_requests_and_audits_once() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let server = TestHttpServer::start(HashMap::from([(
        "/blocked",
        Route::Html("<html><body>must not load</body></html>"),
    )]))?;
    let client = BrowserClient::new()?;
    let page = client.new_page()?;
    page.set_policy_rules(vec![navigation_rule(
        "block-initial",
        Some("/blocked"),
        PolicyAction::Block,
    )])?;
    page.clear_audit_events();

    let error = page
        .navigate_public(&server.url("/blocked"), true)
        .expect_err("initial block must reject navigation");
    assert!(matches!(
        error.downcast_ref::<ActionError>(),
        Some(ActionError::Blocked { .. })
    ));
    assert_eq!(server.request_count("/blocked"), 0);

    let decisions = wait_for_policy_decisions(&page, "block", 1);
    assert_eq!(decisions, 1, "initial destination must be audited once");
    Ok(())
}

#[test]
fn public_navigation_hitl_grant_is_bound_to_identical_destination() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let server = TestHttpServer::start(HashMap::from([
        (
            "/approved",
            Route::Html("<html><body>approved</body></html>"),
        ),
        (
            "/different",
            Route::Html("<html><body>different</body></html>"),
        ),
    ]))?;
    let client = BrowserClient::new()?;
    let page = client.new_page()?;
    page.set_policy_rules(vec![navigation_rule(
        "approve-navigation",
        None,
        PolicyAction::RequireHumanApproval,
    )])?;

    let approved_url = server.url("/approved");
    let first = page
        .navigate_public(&approved_url, true)
        .expect_err("first navigation must require approval");
    assert!(matches!(
        first.downcast_ref::<ActionError>(),
        Some(ActionError::HumanApprovalRequired { .. })
    ));
    assert_eq!(server.request_count("/approved"), 0);
    page.approve_pending_policy_action()?;

    let different = page
        .navigate_public(&server.url("/different"), true)
        .expect_err("a different destination must not consume the grant");
    assert!(matches!(
        different.downcast_ref::<ActionError>(),
        Some(ActionError::HumanApprovalRequired { .. })
    ));
    assert_eq!(server.request_count("/different"), 0);

    let final_url = page.navigate_public(&approved_url, true)?;
    assert_eq!(final_url, approved_url);
    assert_eq!(server.request_count("/approved"), 1);
    Ok(())
}

#[test]
fn public_navigation_allows_redirect_and_subsequent_action_after_cleanup() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let server = TestHttpServer::start(HashMap::from([
        ("/start", Route::Redirect("/final")),
        (
            "/final",
            Route::Html(
                "<html><body><button onclick=\"document.body.dataset.clicked='yes'\">Ready</button></body></html>",
            ),
        ),
    ]))?;
    let client = BrowserClient::new()?;
    let page = client.new_page()?;
    page.set_policy_rules(Vec::new())?;

    let final_url = page.navigate_public(&server.url("/start"), true)?;
    assert_eq!(final_url, server.url("/final"));
    assert_eq!(server.request_count("/start"), 1);
    assert_eq!(server.request_count("/final"), 1);

    let (target_id, stable_key) = find_button_info(&page)?;
    page.act(Some(target_id), Some(&stable_key), "click", None)?;
    let clicked = page
        .evaluate_script("document.body.dataset.clicked")?
        .value
        .and_then(|value| value.as_str().map(str::to_string));
    assert_eq!(clicked.as_deref(), Some("yes"));
    Ok(())
}

#[test]
fn public_navigation_blocks_redirect_before_destination_request() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let server = TestHttpServer::start(HashMap::from([
        ("/start", Route::Redirect("/blocked")),
        (
            "/blocked",
            Route::Html("<html><body>must not load</body></html>"),
        ),
    ]))?;
    let client = BrowserClient::new()?;
    let page = client.new_page()?;
    page.set_policy_rules(vec![navigation_rule(
        "block-redirect",
        Some("/blocked"),
        PolicyAction::Block,
    )])?;

    let error = page
        .navigate_public(&server.url("/start"), true)
        .expect_err("redirect policy must block destination");
    assert!(matches!(
        error.downcast_ref::<ActionError>(),
        Some(ActionError::Blocked { .. })
    ));
    assert_eq!(server.request_count("/start"), 1);
    assert_eq!(server.request_count("/blocked"), 0);
    Ok(())
}

#[test]
fn public_navigation_redirect_hitl_grant_is_bound_to_original_and_destination() -> anyhow::Result<()>
{
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let server = TestHttpServer::start(HashMap::from([
        ("/start", Route::RedirectLocalhost("/approved")),
        (
            "/approved",
            Route::Html("<html><body>approved redirect</body></html>"),
        ),
    ]))?;
    let client = BrowserClient::new()?;
    let page = client.new_page()?;
    page.set_policy_rules(vec![PolicyRule {
        id: "approve-localhost-redirect".to_string(),
        domain: Some("localhost".to_string()),
        path_prefix: Some("/approved".to_string()),
        role: None,
        text_regex: None,
        context_regex: None,
        action: PolicyAction::RequireHumanApproval,
        scope: Some(ApprovalScope::ActionOnly),
        outcome_projector: None,
    }])?;

    let original_url = server.url("/start");
    let approved_url = original_url
        .replace("127.0.0.1", "localhost")
        .replace("/start", "/approved");
    let first = page
        .navigate_public(&original_url, true)
        .expect_err("redirect must require approval before destination I/O");
    assert!(matches!(
        first.downcast_ref::<ActionError>(),
        Some(ActionError::HumanApprovalRequired { .. })
    ));
    assert_eq!(server.request_count("/start"), 1);
    assert_eq!(server.request_count("/approved"), 0);
    page.approve_pending_policy_action()?;

    let direct = page
        .navigate_public(&approved_url, true)
        .expect_err("redirect grant must not approve a direct destination call");
    assert!(matches!(
        direct.downcast_ref::<ActionError>(),
        Some(ActionError::HumanApprovalRequired { .. })
    ));
    assert_eq!(server.request_count("/approved"), 0);

    let final_url = page.navigate_public(&original_url, true)?;
    assert_eq!(final_url, approved_url);
    assert_eq!(server.request_count("/start"), 2);
    assert_eq!(server.request_count("/approved"), 1);
    Ok(())
}

#[test]
fn public_navigation_plugin_veto_blocks_redirect_before_destination_request() -> anyhow::Result<()>
{
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let server = TestHttpServer::start(HashMap::from([
        ("/start", Route::Redirect("/plugin-blocked")),
        (
            "/plugin-blocked",
            Route::Html("<html><body>must not load</body></html>"),
        ),
    ]))?;
    let mut plugins = PluginHookConfig::default();
    plugins.policy_plugins.push(Box::new(BlockRedirectPlugin));
    let client = BrowserClient::new_with_plugin_hooks(plugins)?;
    let page = client.new_page()?;

    let error = page
        .navigate_public(&server.url("/start"), true)
        .expect_err("plugin must veto the redirect destination");
    assert!(matches!(
        error.downcast_ref::<ActionError>(),
        Some(ActionError::Blocked { rule_id }) if rule_id == "plugin:block-redirect-plugin"
    ));
    assert_eq!(server.request_count("/start"), 1);
    assert_eq!(server.request_count("/plugin-blocked"), 0);
    Ok(())
}

#[test]
fn public_navigation_does_not_apply_top_level_redirect_policy_to_iframes() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let server = TestHttpServer::start(HashMap::from([
        (
            "/start",
            Route::Html("<html><body><iframe src='/iframe'></iframe></body></html>"),
        ),
        ("/iframe", Route::Redirect("/iframe-final")),
        (
            "/iframe-final",
            Route::Html("<html><body>iframe loaded</body></html>"),
        ),
    ]))?;
    let client = BrowserClient::new()?;
    let page = client.new_page()?;
    page.set_policy_rules(vec![navigation_rule(
        "block-only-if-top-level",
        Some("/iframe-final"),
        PolicyAction::Block,
    )])?;

    let final_url = page.navigate_public(&server.url("/start"), true)?;
    assert_eq!(final_url, server.url("/start"));
    assert!(wait_for_request_count(&server, "/iframe-final", 1));
    assert_eq!(server.request_count("/iframe"), 1);
    assert_eq!(server.request_count("/iframe-final"), 1);
    Ok(())
}

fn find_button_info(page: &PageSession) -> anyhow::Result<(i64, String)> {
    let root = page.get_document_node()?;
    let semantic = normalize_dom(LoadProfile::Interactive, &root)?;
    let state = SemanticState::new(semantic, LoadProfile::Interactive);
    find_button_info_in_node(state.root()).ok_or_else(|| anyhow::anyhow!("button not found"))
}

fn find_button_info_in_node(
    node: &core_runtime::sre::state::SemanticNode,
) -> Option<(i64, String)> {
    if node.role == "button" {
        return Some((
            node.backend_node_id,
            node.stable_key.clone().unwrap_or_default(),
        ));
    }
    node.children.iter().find_map(find_button_info_in_node)
}

fn wait_for_policy_decisions(page: &PageSession, expected: &str, count: usize) -> usize {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(2) {
        let found = page
            .audit_events()
            .into_iter()
            .filter(|event| {
                matches!(
                    event,
                    AuditEvent::PolicyDecision {
                        action,
                        decision,
                        ..
                    } if action == "navigate" && decision == expected
                )
            })
            .count();
        if found >= count {
            return found;
        }
        thread::sleep(Duration::from_millis(10));
    }
    0
}

fn wait_for_request_count(server: &TestHttpServer, path: &str, count: usize) -> bool {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(2) {
        if server.request_count(path) >= count {
            return true;
        }
        thread::sleep(Duration::from_millis(10));
    }
    false
}
