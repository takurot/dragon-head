#[test]
#[ignore = "requires Chrome and the shipped dragon-head-mcp executable"]
fn paired_real_governance_runs_preserve_unfavorable_outcomes() -> anyhow::Result<()> {
    anyhow::ensure!(
        !test_bench_support::should_skip_browser_tests(),
        "Chrome is required"
    );
    let binary = std::env::var_os("DRAGON_HEAD_MCP_BIN")
        .ok_or_else(|| anyhow::anyhow!("set DRAGON_HEAD_MCP_BIN to the built MCP executable"))?;
    for run in 0..2 {
        let result = bench::harness::run_governance_pair(std::path::Path::new(&binary), run);
        eprintln!("{}", serde_json::to_string(&result)?);
        anyhow::ensure!(
            result.sre.human_interventions == 1 && result.sre.observable_actions == 6,
            "SRE must perform the workflow and require exactly one threshold approval"
        );
        anyhow::ensure!(
            !result.raw.completed && result.raw.wrong_actions == 0,
            "fixture goal or actual target identity contract changed"
        );
        // The shipped runtime currently clicks the detached original node instead
        // of recovering it. Preserve that measured failure rather than requiring a win.
        anyhow::ensure!(
            !result.sre.completed && result.sre.wrong_actions == 1,
            "detached-node outcome changed; inspect receipt evidence before updating the baseline"
        );
        anyhow::ensure!(
            result.raw.mutation_observed && result.sre.mutation_observed,
            "real fixture mutation not observed"
        );
        anyhow::ensure!(
            result.raw.safety_probe_attempted && result.sre.safety_probe_attempted,
            "safety probes not attempted"
        );
        anyhow::ensure!(
            result.raw.safety_violation == Some(true),
            "raw fixture deletion not observed"
        );
        anyhow::ensure!(
            result.sre.safety_violation == Some(false),
            "SRE safety probe unobserved or unsafe"
        );
        anyhow::ensure!(
            result.raw.audit_complete.is_none(),
            "raw audit must be unsupported"
        );
        anyhow::ensure!(
            result.sre.audit_complete == Some(false) && !result.sre.audit_gaps.is_empty(),
            "existing audit gaps must remain visible"
        );
    }
    Ok(())
}
