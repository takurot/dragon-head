use skills_engine::{
    ActStep, ExtractStep, HandoffStep, LocateStep, OperationOutcome,
    SUPPORTED_SKILL_SCHEMA_VERSION, SkillDefinition, SkillEngine, SkillEngineError, SkillRunStatus,
    SkillRuntime, SkillStep, StepControl, VerifyStep, validate_skill_definition,
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

struct PanicRuntime;

impl SkillRuntime for PanicRuntime {
    fn locate(
        &mut self,
        _step: &LocateStep,
        _ctx: &mut skills_engine::SkillExecutionContext,
    ) -> OperationOutcome {
        panic!("unsupported skill must be rejected before runtime calls")
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

fn single_locate_skill(schema_version: u32) -> SkillDefinition {
    SkillDefinition {
        schema_version,
        name: "version-boundary".to_string(),
        steps: vec![SkillStep::Locate(LocateStep {
            id: Some("locate".to_string()),
            query: "button".to_string(),
            control: StepControl::default(),
        })],
    }
}

#[test]
fn test_validate_rejects_unsupported_typed_schema_versions() {
    for version in [0, SUPPORTED_SKILL_SCHEMA_VERSION + 1] {
        let error = validate_skill_definition(&single_locate_skill(version))
            .expect_err("unsupported typed schema version must be rejected");
        assert!(matches!(
            error,
            SkillEngineError::UnsupportedSchemaVersion { .. }
        ));
    }
}

#[test]
fn test_run_rejects_unsupported_schema_versions_before_runtime_calls() {
    for version in [0, SUPPORTED_SKILL_SCHEMA_VERSION + 1] {
        let error = SkillEngine::new()
            .run(&single_locate_skill(version), &mut PanicRuntime)
            .expect_err("unsupported schema version must be rejected before execution");
        assert!(matches!(
            error,
            SkillEngineError::UnsupportedSchemaVersion { .. }
        ));
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

fn verify_step(id: &str, target: &str, control: StepControl) -> SkillStep {
    SkillStep::Verify(VerifyStep {
        id: Some(id.to_string()),
        target: target.to_string(),
        expected: "visible".to_string(),
        control,
    })
}

fn act_step(id: &str, target: &str, control: StepControl) -> SkillStep {
    SkillStep::Act(ActStep {
        id: Some(id.to_string()),
        action: "click".to_string(),
        target: target.to_string(),
        value: None,
        control,
    })
}

fn locate_step(id: &str, control: StepControl) -> SkillStep {
    SkillStep::Locate(LocateStep {
        id: Some(id.to_string()),
        query: "target".to_string(),
        control,
    })
}

fn assert_invalid_act_flow(steps: Vec<SkillStep>) {
    let skill = SkillDefinition {
        schema_version: 1,
        name: "invalid-act-flow".to_string(),
        steps,
    };
    let mut runtime = MockRuntime::default();

    let error = SkillEngine::new().run(&skill, &mut runtime).unwrap_err();
    assert!(matches!(
        error,
        SkillEngineError::ActStepMissingVerifyPredecessor { .. }
    ));
}

#[test]
fn test_branch_cannot_skip_verify_before_act() {
    assert_invalid_act_flow(vec![
        locate_step(
            "start",
            StepControl {
                on_success: Some("do_act".to_string()),
                ..StepControl::default()
            },
        ),
        verify_step("check", "target", StepControl::default()),
        act_step("do_act", "target", StepControl::default()),
    ]);
}

#[test]
fn test_verify_failure_cannot_authorize_act() {
    assert_invalid_act_flow(vec![
        verify_step(
            "check",
            "target",
            StepControl {
                on_failure: Some("do_act".to_string()),
                ..StepControl::default()
            },
        ),
        act_step("do_act", "target", StepControl::default()),
    ]);
}

#[test]
fn test_verify_must_match_act_target() {
    assert_invalid_act_flow(vec![
        verify_step("check", "first-target", StepControl::default()),
        act_step("do_act", "second-target", StepControl::default()),
    ]);
}

#[test]
fn test_act_cannot_retry_without_reverification() {
    assert_invalid_act_flow(vec![
        verify_step("check", "target", StepControl::default()),
        act_step(
            "do_act",
            "target",
            StepControl {
                max_retries: 1,
                ..StepControl::default()
            },
        ),
    ]);
}

#[test]
fn test_act_cannot_loop_without_reverification() {
    assert_invalid_act_flow(vec![
        verify_step("check", "target", StepControl::default()),
        act_step(
            "do_act",
            "target",
            StepControl {
                on_success: Some("do_act".to_string()),
                ..StepControl::default()
            },
        ),
    ]);
}

#[test]
fn test_unsafe_backward_edge_into_act_is_rejected() {
    assert_invalid_act_flow(vec![
        verify_step("check", "target", StepControl::default()),
        act_step("do_act", "target", StepControl::default()),
        locate_step(
            "loop",
            StepControl {
                on_success: Some("do_act".to_string()),
                ..StepControl::default()
            },
        ),
    ]);
}

#[test]
fn test_branched_verify_can_authorize_matching_act() -> Result<(), SkillEngineError> {
    let skill = SkillDefinition {
        schema_version: 1,
        name: "safe-verify-branch".to_string(),
        steps: vec![
            verify_step(
                "check",
                "target",
                StepControl {
                    on_success: Some("do_act".to_string()),
                    ..StepControl::default()
                },
            ),
            SkillStep::Handoff(HandoffStep {
                id: Some("skipped".to_string()),
                reason: "not reached".to_string(),
                assignee: None,
                control: StepControl::default(),
            }),
            act_step("do_act", "target", StepControl::default()),
        ],
    };
    let mut runtime = MockRuntime::default();

    let report = SkillEngine::new().run(&skill, &mut runtime)?;
    let operations: Vec<&str> = report
        .trace
        .iter()
        .map(|entry| entry.operation.as_str())
        .collect();
    assert_eq!(
        operations,
        vec!["verify", "policy_check", "act", "post_check"]
    );
    Ok(())
}

#[test]
fn test_safe_cycle_reverifies_before_each_act() -> Result<(), SkillEngineError> {
    let skill = SkillDefinition {
        schema_version: 1,
        name: "safe-reverify-cycle".to_string(),
        steps: vec![
            verify_step(
                "check",
                "target",
                StepControl {
                    on_success: Some("do_act".to_string()),
                    on_failure: Some("done".to_string()),
                    ..StepControl::default()
                },
            ),
            SkillStep::Handoff(HandoffStep {
                id: Some("done".to_string()),
                reason: "cycle complete".to_string(),
                assignee: None,
                control: StepControl::default(),
            }),
            act_step(
                "do_act",
                "target",
                StepControl {
                    on_success: Some("check".to_string()),
                    ..StepControl::default()
                },
            ),
        ],
    };
    let mut runtime = MockRuntime::default().with_script(
        "verify",
        vec![
            OperationOutcome::Success,
            OperationOutcome::Failure {
                reason: "done".to_string(),
            },
        ],
    );

    let report = SkillEngine::new().run(&skill, &mut runtime)?;
    let operations: Vec<&str> = report
        .trace
        .iter()
        .map(|entry| entry.operation.as_str())
        .collect();
    assert_eq!(
        operations,
        vec![
            "verify",
            "policy_check",
            "act",
            "post_check",
            "verify",
            "handoff"
        ]
    );
    Ok(())
}
