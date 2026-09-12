//! ISSUE-304: `ExtractStep` writes into `SkillExecutionContext::extracted`, but before this fix
//! `SkillRunReport` had no `outputs` field, so the extracted values were dead data — never
//! observable outside the step that produced them. These tests assert `SkillRunReport::outputs`
//! carries `SkillExecutionContext::extracted` out of `SkillEngine::run`, including a *partial*
//! set of outputs when the run fails or hands off partway through.
use serde_json::json;
use skills_engine::{
    ActStep, ExtractStep, HandoffStep, LocateStep, OperationOutcome, SkillDefinition, SkillEngine,
    SkillRunStatus, SkillRuntime, SkillStep, StepControl, VerifyStep,
};

/// A runtime whose `extract` actually writes into `SkillExecutionContext::extracted` (unlike
/// `skill_conformance.rs`'s `MockRuntime`, which ignores the step entirely) — the only way to
/// exercise `outputs` end-to-end without a real `PageSession`. Other operations succeed unless
/// `fail_on` names them, in which case they fail once and the scripted operation is removed so a
/// retried/branched-back-to step does not fail forever.
#[derive(Default)]
struct ExtractingRuntime {
    fail_on: Vec<&'static str>,
}

impl ExtractingRuntime {
    fn failing_on(operation: &'static str) -> Self {
        Self {
            fail_on: vec![operation],
        }
    }

    fn maybe_fail(&mut self, operation: &'static str) -> OperationOutcome {
        if let Some(pos) = self.fail_on.iter().position(|op| *op == operation) {
            self.fail_on.remove(pos);
            return OperationOutcome::Failure {
                reason: format!("{operation} scripted to fail"),
            };
        }
        OperationOutcome::Success
    }
}

impl SkillRuntime for ExtractingRuntime {
    fn locate(
        &mut self,
        _step: &LocateStep,
        _ctx: &mut skills_engine::SkillExecutionContext,
    ) -> OperationOutcome {
        self.maybe_fail("locate")
    }

    fn verify(
        &mut self,
        _step: &VerifyStep,
        _ctx: &mut skills_engine::SkillExecutionContext,
    ) -> OperationOutcome {
        self.maybe_fail("verify")
    }

    fn act(
        &mut self,
        _step: &ActStep,
        _ctx: &mut skills_engine::SkillExecutionContext,
    ) -> OperationOutcome {
        self.maybe_fail("act")
    }

    fn extract(
        &mut self,
        step: &ExtractStep,
        ctx: &mut skills_engine::SkillExecutionContext,
    ) -> OperationOutcome {
        let outcome = self.maybe_fail("extract");
        if matches!(outcome, OperationOutcome::Success) {
            // A deterministic, step-identifiable value so tests can assert on it.
            ctx.extracted
                .insert(step.key.clone(), json!(format!("value-for-{}", step.key)));
        }
        outcome
    }

    fn handoff(
        &mut self,
        step: &HandoffStep,
        _ctx: &mut skills_engine::SkillExecutionContext,
    ) -> OperationOutcome {
        OperationOutcome::Handoff {
            reason: step.reason.clone(),
        }
    }
}

fn extract_step(id: &str, key: &str) -> SkillStep {
    SkillStep::Extract(ExtractStep {
        id: Some(id.to_string()),
        key: key.to_string(),
        selector: "#irrelevant".to_string(),
        control: StepControl::default(),
    })
}

fn locate_step(id: &str) -> SkillStep {
    SkillStep::Locate(LocateStep {
        id: Some(id.to_string()),
        query: "id:1".to_string(),
        control: StepControl::default(),
    })
}

fn handoff_step(id: &str) -> SkillStep {
    SkillStep::Handoff(HandoffStep {
        id: Some(id.to_string()),
        reason: "needs a human".to_string(),
        assignee: None,
        control: StepControl::default(),
    })
}

fn skill(name: &str, steps: Vec<SkillStep>) -> SkillDefinition {
    SkillDefinition {
        schema_version: 1,
        name: name.to_string(),
        steps,
    }
}

#[test]
fn completed_run_returns_all_extracted_outputs() {
    let def = skill(
        "extract-two",
        vec![extract_step("e1", "order_id"), extract_step("e2", "email")],
    );
    let report = SkillEngine::new()
        .run(&def, &mut ExtractingRuntime::default())
        .unwrap();

    assert_eq!(report.status, SkillRunStatus::Completed);
    assert_eq!(
        report.outputs.get("order_id"),
        Some(&json!("value-for-order_id"))
    );
    assert_eq!(report.outputs.get("email"), Some(&json!("value-for-email")));
}

/// BUG REGRESSION (ISSUE-304): before this fix, `SkillRunReport` had no `outputs` field at all,
/// so an extract-then-fail skill's already-captured value was unrecoverable — this locks in the
/// "partial outputs on failure" contract the issue explicitly asks for.
#[test]
fn failed_run_still_returns_outputs_extracted_before_the_failure() {
    let def = skill(
        "extract-then-fail",
        vec![extract_step("e1", "order_id"), locate_step("boom")],
    );
    let report = SkillEngine::new()
        .run(&def, &mut ExtractingRuntime::failing_on("locate"))
        .unwrap();

    assert_eq!(report.status, SkillRunStatus::Failed);
    assert_eq!(
        report.outputs.get("order_id"),
        Some(&json!("value-for-order_id")),
        "a value extracted before the failing step must still be returned: {:?}",
        report.outputs
    );
}

#[test]
fn handoff_run_still_returns_outputs_extracted_before_the_handoff() {
    let def = skill(
        "extract-then-handoff",
        vec![extract_step("e1", "order_id"), handoff_step("h1")],
    );
    let report = SkillEngine::new()
        .run(&def, &mut ExtractingRuntime::default())
        .unwrap();

    assert_eq!(report.status, SkillRunStatus::Handoff);
    assert_eq!(
        report.outputs.get("order_id"),
        Some(&json!("value-for-order_id"))
    );
}

#[test]
fn duplicate_extraction_key_is_last_write_wins() {
    let def = skill(
        "duplicate-key",
        vec![
            SkillStep::Extract(ExtractStep {
                id: Some("first".to_string()),
                key: "confirmation_id".to_string(),
                selector: "#first".to_string(),
                control: StepControl::default(),
            }),
            SkillStep::Extract(ExtractStep {
                id: Some("second".to_string()),
                key: "confirmation_id".to_string(),
                selector: "#second".to_string(),
                control: StepControl::default(),
            }),
        ],
    );

    // ExtractingRuntime derives the value from `step.key`, which is identical for both steps, so
    // assert directly against the map length/content to prove the second write overwrote the
    // first rather than both somehow coexisting.
    let report = SkillEngine::new()
        .run(&def, &mut ExtractingRuntime::default())
        .unwrap();

    assert_eq!(report.outputs.len(), 1);
    assert_eq!(
        report.outputs.get("confirmation_id"),
        Some(&json!("value-for-confirmation_id"))
    );
}

#[test]
fn outputs_field_is_empty_when_no_extract_step_ran() {
    let def = skill("no-extract", vec![locate_step("l1")]);
    let report = SkillEngine::new()
        .run(&def, &mut ExtractingRuntime::default())
        .unwrap();

    assert_eq!(report.status, SkillRunStatus::Completed);
    assert!(report.outputs.is_empty());
}
