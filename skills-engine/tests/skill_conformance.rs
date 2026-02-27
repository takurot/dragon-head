use skills_engine::{
    ActStep, ExtractStep, HandoffStep, LocateStep, OperationOutcome, SkillDefinition, SkillEngine,
    SkillEngineError, SkillRunStatus, SkillRuntime, SkillStep, StepControl, VerifyStep,
};
use std::collections::{HashMap, VecDeque};

#[derive(Default)]
struct MockRuntime {
    scripts: HashMap<&'static str, VecDeque<OperationOutcome>>,
}

impl MockRuntime {
    fn with_script(mut self, operation: &'static str, outcomes: Vec<OperationOutcome>) -> Self {
        self.scripts
            .insert(operation, outcomes.into_iter().collect());
        self
    }

    fn next(&mut self, operation: &'static str) -> OperationOutcome {
        self.scripts
            .get_mut(operation)
            .and_then(VecDeque::pop_front)
            .unwrap_or(OperationOutcome::Success)
    }
}

impl SkillRuntime for MockRuntime {
    fn locate(
        &mut self,
        _step: &LocateStep,
        _ctx: &mut skills_engine::SkillExecutionContext,
    ) -> OperationOutcome {
        self.next("locate")
    }

    fn verify(
        &mut self,
        _step: &VerifyStep,
        _ctx: &mut skills_engine::SkillExecutionContext,
    ) -> OperationOutcome {
        self.next("verify")
    }

    fn act(
        &mut self,
        _step: &ActStep,
        _ctx: &mut skills_engine::SkillExecutionContext,
    ) -> OperationOutcome {
        self.next("act")
    }

    fn extract(
        &mut self,
        _step: &ExtractStep,
        _ctx: &mut skills_engine::SkillExecutionContext,
    ) -> OperationOutcome {
        self.next("extract")
    }

    fn handoff(
        &mut self,
        _step: &HandoffStep,
        _ctx: &mut skills_engine::SkillExecutionContext,
    ) -> OperationOutcome {
        self.next("handoff")
    }
}

fn happy_path_skill() -> SkillDefinition {
    SkillDefinition {
        schema_version: 1,
        name: "search-and-extract".to_string(),
        steps: vec![
            SkillStep::Locate(LocateStep {
                id: Some("locate_product".to_string()),
                query: "search button".to_string(),
                control: StepControl::default(),
            }),
            SkillStep::Verify(VerifyStep {
                id: Some("verify_product".to_string()),
                target: "search button".to_string(),
                expected: "enabled".to_string(),
                control: StepControl::default(),
            }),
            SkillStep::Act(ActStep {
                id: Some("click_product".to_string()),
                action: "click".to_string(),
                target: "search button".to_string(),
                value: None,
                control: StepControl::default(),
            }),
            SkillStep::Extract(ExtractStep {
                id: Some("extract_product".to_string()),
                key: "product_name".to_string(),
                selector: "#product-name".to_string(),
                control: StepControl::default(),
            }),
        ],
    }
}

#[test]
fn test_skill_conformance_happy_path_order() -> Result<(), SkillEngineError> {
    let skill = happy_path_skill();
    let mut runtime = MockRuntime::default();
    let report = SkillEngine::new().run(&skill, &mut runtime)?;

    assert_eq!(report.status, SkillRunStatus::Completed);
    let operations: Vec<&str> = report
        .trace
        .iter()
        .map(|entry| entry.operation.as_str())
        .collect();
    assert_eq!(
        operations,
        vec![
            "locate",
            "verify",
            "policy_check",
            "act",
            "post_check",
            "extract"
        ]
    );
    Ok(())
}

#[test]
fn test_verify_failure_suppresses_act() -> Result<(), SkillEngineError> {
    let skill = happy_path_skill();
    let mut runtime = MockRuntime::default().with_script(
        "verify",
        vec![OperationOutcome::Failure {
            reason: "not enabled".to_string(),
        }],
    );

    let report = SkillEngine::new().run(&skill, &mut runtime)?;

    assert_eq!(report.status, SkillRunStatus::Failed);
    let operations: Vec<&str> = report
        .trace
        .iter()
        .map(|entry| entry.operation.as_str())
        .collect();
    assert!(operations.contains(&"verify"));
    assert!(!operations.contains(&"policy_check"));
    assert!(!operations.contains(&"act"));
    assert!(!operations.contains(&"post_check"));
    Ok(())
}

#[test]
fn test_retry_branch_and_handoff() -> Result<(), SkillEngineError> {
    let skill = SkillDefinition {
        schema_version: 1,
        name: "retry-branch-handoff".to_string(),
        steps: vec![
            SkillStep::Locate(LocateStep {
                id: Some("locate_first".to_string()),
                query: "submit button".to_string(),
                control: StepControl {
                    max_retries: 1,
                    on_success: None,
                    on_failure: None,
                },
            }),
            SkillStep::Verify(VerifyStep {
                id: Some("verify_first".to_string()),
                target: "submit button".to_string(),
                expected: "visible".to_string(),
                control: StepControl::default(),
            }),
            SkillStep::Act(ActStep {
                id: Some("act_first".to_string()),
                action: "click".to_string(),
                target: "submit button".to_string(),
                value: None,
                control: StepControl {
                    max_retries: 0,
                    on_success: Some("handoff_step".to_string()),
                    on_failure: None,
                },
            }),
            SkillStep::Extract(ExtractStep {
                id: Some("extract_should_skip".to_string()),
                key: "ignored".to_string(),
                selector: "#ignored".to_string(),
                control: StepControl::default(),
            }),
            SkillStep::Handoff(HandoffStep {
                id: Some("handoff_step".to_string()),
                reason: "manual approval".to_string(),
                assignee: Some("ops-team".to_string()),
                control: StepControl::default(),
            }),
        ],
    };

    let mut runtime = MockRuntime::default().with_script(
        "locate",
        vec![
            OperationOutcome::Failure {
                reason: "transient".to_string(),
            },
            OperationOutcome::Success,
        ],
    );

    let report = SkillEngine::new().run(&skill, &mut runtime)?;

    assert_eq!(report.status, SkillRunStatus::Handoff);
    let operations: Vec<&str> = report
        .trace
        .iter()
        .map(|entry| entry.operation.as_str())
        .collect();
    assert_eq!(operations.iter().filter(|op| **op == "locate").count(), 2);
    assert!(operations.contains(&"handoff"));
    assert!(!operations.contains(&"extract"));
    Ok(())
}

#[test]
fn test_act_requires_immediate_verify_predecessor() {
    let invalid = SkillDefinition {
        schema_version: 1,
        name: "invalid-order".to_string(),
        steps: vec![
            SkillStep::Locate(LocateStep {
                id: Some("locate_only".to_string()),
                query: "target".to_string(),
                control: StepControl::default(),
            }),
            SkillStep::Act(ActStep {
                id: Some("act_without_verify".to_string()),
                action: "click".to_string(),
                target: "target".to_string(),
                value: None,
                control: StepControl::default(),
            }),
        ],
    };

    let mut runtime = MockRuntime::default();
    let err = SkillEngine::new().run(&invalid, &mut runtime).unwrap_err();
    assert!(matches!(
        err,
        SkillEngineError::ActStepMissingVerifyPredecessor { .. }
    ));
}
