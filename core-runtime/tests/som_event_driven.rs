use std::{fs, path::PathBuf};

use core_runtime::{
    sre::{normalize_dom, LoadProfile, SemanticNode, SemanticState},
    BrowserClient, SomTrigger, VerifyError,
};

#[test]
fn test_som_not_generated_without_trigger() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    let html = r#"
        <html>
            <body>
                <button id='btn'>No Trigger</button>
            </body>
        </html>
    "#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url)?;

    let _ = page.get_content()?;
    let _ = page.get_document_node()?;

    assert_eq!(page.som_generation_count(), 0);
    assert!(page.last_visual_capture().is_none());

    Ok(())
}

#[test]
fn test_som_generated_by_get_visual_trigger() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    let html = r#"
        <html>
            <body style='margin:0'>
                <button id='purchase' style='width:160px;height:48px'>Purchase</button>
            </body>
        </html>
    "#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url)?;

    let capture = page.get_visual()?;
    assert_eq!(capture.trigger, SomTrigger::GetVisual);
    assert!(
        !capture.image_png.is_empty(),
        "SoM screenshot should not be empty"
    );
    assert_marks_integrity(&capture.marks);
    assert_eq!(page.som_generation_count(), 1);

    maybe_dump_png("get_visual", &capture.image_png)?;

    Ok(())
}

#[test]
fn test_som_generated_by_act_ambiguous_trigger() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    let html = r#"
        <html><body><button id='submit'>Submit</button></body></html>
    "#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url)?;

    let fake_id = 999999;
    let fake_key = "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";

    let result = page.act(Some(fake_id), Some(fake_key), "click", None);
    assert!(result.is_err(), "act should fail with invalid id/key");

    let err = result.unwrap_err();
    let action_err = err
        .downcast_ref::<core_runtime::ActionError>()
        .expect("error should be ActionError");
    // With Self-Healing recovery (PR-21): if stable_key is provided but cache
    // has no prior signature, recovery fails and returns AskHumanRequired.
    // Both are valid terminal errors for this scenario.
    assert!(
        matches!(
            action_err,
            core_runtime::ActionError::VerifyRequired
                | core_runtime::ActionError::AskHumanRequired { .. }
        ),
        "expected VerifyRequired or AskHumanRequired, got: {action_err:?}"
    );

    assert_eq!(page.som_generation_count(), 1);
    let capture = page
        .last_visual_capture()
        .expect("capture must exist after ambiguous act");
    assert_eq!(capture.trigger, SomTrigger::ActAmbiguous);
    assert_marks_integrity(&capture.marks);

    maybe_dump_png("act_ambiguous", &capture.image_png)?;

    Ok(())
}

#[test]
fn test_som_generated_by_act_ambiguous_without_stable_key() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    let html = r#"
        <html><body><button id='submit'>Submit</button></body></html>
    "#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url)?;

    let fake_id = 999999;
    let result = page.act(Some(fake_id), None, "click", None);
    assert!(result.is_err(), "act should fail with invalid id");

    let err = result.unwrap_err();
    let action_err = err
        .downcast_ref::<core_runtime::ActionError>()
        .expect("error should be ActionError::VerifyRequired");
    assert!(matches!(
        action_err,
        core_runtime::ActionError::VerifyRequired
    ));

    assert_eq!(page.som_generation_count(), 1);
    let capture = page
        .last_visual_capture()
        .expect("capture must exist after ambiguous act");
    assert_eq!(capture.trigger, SomTrigger::ActAmbiguous);
    assert_marks_integrity(&capture.marks);

    maybe_dump_png("act_ambiguous_without_stable_key", &capture.image_png)?;

    Ok(())
}

#[test]
fn test_som_generated_by_verify_failure_trigger() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    let html = r#"
        <html>
            <body>
                <button id='btn_login'>Login</button>
            </body>
        </html>
    "#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url)?;

    let root = page.get_document_node()?;
    let sem = normalize_dom(LoadProfile::Minimal, &root)?;
    let state = SemanticState::new(sem, LoadProfile::Minimal);
    let (button_id, _) = find_button_info(state.root()).expect("button should exist");

    let result = page.verify_text(button_id, "Register");
    assert!(result.is_err(), "verify should fail for mismatched text");

    let err = result.unwrap_err();
    let verify_err = err
        .downcast_ref::<VerifyError>()
        .expect("error should be VerifyError");
    assert!(matches!(
        verify_err,
        VerifyError::ExpectationMismatch { .. }
    ));

    assert_eq!(page.som_generation_count(), 1);
    let capture = page
        .last_visual_capture()
        .expect("capture must exist after verify failure");
    assert_eq!(capture.trigger, SomTrigger::VerifyFailed);
    assert_marks_integrity(&capture.marks);

    maybe_dump_png("verify_failed", &capture.image_png)?;

    Ok(())
}

fn assert_marks_integrity(marks: &[core_runtime::SomMark]) {
    assert!(!marks.is_empty(), "marks should not be empty");

    let has_valid_mark = marks.iter().any(|mark| {
        mark.id > 0
            && mark
                .stable_key
                .as_ref()
                .is_some_and(|key| !key.trim().is_empty())
            && mark.bbox[2].is_finite()
            && mark.bbox[3].is_finite()
            && mark.bbox[2] > 0.0
            && mark.bbox[3] > 0.0
    });

    assert!(
        has_valid_mark,
        "at least one mark should have id/stable_key/non-zero bbox"
    );
}

fn find_button_info(node: &SemanticNode) -> Option<(i64, String)> {
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

fn maybe_dump_png(name: &str, png: &[u8]) -> anyhow::Result<()> {
    let dir = match std::env::var("SOM_ARTIFACT_DIR") {
        Ok(value) => PathBuf::from(value),
        Err(_) => return Ok(()),
    };

    fs::create_dir_all(&dir)?;
    fs::write(dir.join(format!("{name}.png")), png)?;
    Ok(())
}
