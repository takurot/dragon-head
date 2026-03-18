use skills_engine::{
    ActStep, ExtractStep, HandoffStep, LocateStep, OperationOutcome, SkillDefinition, SkillEngine,
    SkillRunStatus, SkillRuntime, SkillStep, StepControl, VerifyStep,
};
use std::collections::{HashMap, VecDeque};

use serde_json::{Value, json};
use test_bench_support::{EvaluationBench, EvaluationMode};

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

#[test]
fn test_skills_engine_comprehensive_evaluation_suite() -> anyhow::Result<()> {
    let mut bench = EvaluationBench::new(
        "skills-engine",
        "comprehensive_evaluation",
        EvaluationMode::from_env(),
    );

    bench.run_scenario("skill_happy_path", "workflow", scenario_skill_happy_path);
    bench.run_scenario(
        "verify_failure_suppresses_act",
        "workflow",
        scenario_verify_failure_suppresses_act,
    );
    bench.run_scenario(
        "retry_branch_and_handoff",
        "workflow",
        scenario_retry_branch_and_handoff,
    );

    bench.write_if_configured()?;
    bench.assert_required_scenarios(&[
        "skill_happy_path",
        "verify_failure_suppresses_act",
        "retry_branch_and_handoff",
    ])?;
    bench.assert_all_passed()?;
    Ok(())
}

fn scenario_skill_happy_path() -> anyhow::Result<Value> {
    let skill = happy_path_skill();
    let mut runtime = MockRuntime::default();
    let report = SkillEngine::new().run(&skill, &mut runtime)?;
    let operations = report
        .trace
        .iter()
        .map(|entry| entry.operation.as_str())
        .collect::<Vec<_>>();

    assert_eq!(report.status, SkillRunStatus::Completed);
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

    Ok(json!({
        "status": "completed",
        "trace_len": operations.len(),
    }))
}

fn scenario_verify_failure_suppresses_act() -> anyhow::Result<Value> {
    let skill = happy_path_skill();
    let mut runtime = MockRuntime::default().with_script(
        "verify",
        vec![OperationOutcome::Failure {
            reason: "not enabled".to_string(),
        }],
    );

    let report = SkillEngine::new().run(&skill, &mut runtime)?;
    let operations = report
        .trace
        .iter()
        .map(|entry| entry.operation.as_str())
        .collect::<Vec<_>>();

    assert_eq!(report.status, SkillRunStatus::Failed);
    assert!(operations.contains(&"verify"));
    assert!(!operations.contains(&"act"));

    Ok(json!({
        "status": "failed",
        "operations": operations,
    }))
}

fn scenario_retry_branch_and_handoff() -> anyhow::Result<Value> {
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
    let operations = report
        .trace
        .iter()
        .map(|entry| entry.operation.as_str())
        .collect::<Vec<_>>();

    assert_eq!(report.status, SkillRunStatus::Handoff);
    assert_eq!(operations.iter().filter(|op| **op == "locate").count(), 2);
    assert!(operations.contains(&"handoff"));
    assert!(!operations.contains(&"extract"));

    Ok(json!({
        "status": "handoff",
        "locate_retries": 2,
    }))
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
