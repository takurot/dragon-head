use bench::{harness, metrics, report};
use clap::Parser;
use core_runtime::chrome_available;
use std::path::PathBuf;

#[cfg(test)]
mod governance_cli_tests {
    use super::*;

    #[test]
    fn legacy_default_and_governance_default_are_separate() {
        let legacy = Args::try_parse_from(["bench", "--url", "https://example.com"]).unwrap();
        assert_eq!(legacy.run_count(), 3);
        let governance =
            Args::try_parse_from(["bench", "--governance", "--mcp-bin", "demo-mcp"]).unwrap();
        assert_eq!(governance.run_count(), 20);
        assert!(governance.validate().is_ok());
    }

    #[test]
    fn governance_rejects_missing_mcp_path_zero_runs_and_mixed_modes() {
        assert!(Args::try_parse_from(["bench", "--governance"]).is_err());
        assert!(Args::try_parse_from(["bench"]).is_err());
        assert!(Args::try_parse_from([
            "bench",
            "--governance",
            "--mcp-bin",
            "demo",
            "--url",
            "https://example.com"
        ])
        .is_err());
        assert!(Args::try_parse_from([
            "bench",
            "--governance",
            "--mcp-bin",
            "demo",
            "--step-selectors",
            "#submit"
        ])
        .is_err());
        let zero =
            Args::try_parse_from(["bench", "--governance", "--mcp-bin", "demo", "--runs", "0"])
                .unwrap();
        assert!(zero.validate().is_err());
    }

    #[test]
    fn legacy_multi_step_and_output_flags_remain_accepted() {
        let legacy = Args::try_parse_from([
            "bench",
            "--url",
            "https://example.com",
            "--runs",
            "2",
            "--step-selectors",
            "#one,#two",
            "--output",
            "old.md",
            "--output-json",
            "old.json",
        ])
        .unwrap();
        assert_eq!(legacy.run_count(), 2);
        assert_eq!(legacy.step_selectors.unwrap(), ["#one", "#two"]);
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "dragon-head-bench",
    about = "Side-by-side ROI and governance comparisons: Raw DOM vs Dragon Head"
)]
struct Args {
    /// URL to benchmark
    #[arg(
        long,
        required_unless_present = "governance",
        conflicts_with = "governance"
    )]
    url: Option<String>,

    /// Number of measurement runs (default: 3; governance: 20)
    #[arg(long)]
    runs: Option<u32>,

    /// Compare governed workflow outcomes on the controlled local fixture
    #[arg(long, requires = "mcp_bin", conflicts_with = "step_selectors")]
    governance: bool,

    /// Shipped dragon-head-mcp executable used by governance mode
    #[arg(long, requires = "governance")]
    mcp_bin: Option<PathBuf>,

    /// Write Markdown report to this file
    #[arg(long)]
    output: Option<PathBuf>,

    /// Write JSON report to this file (for bench-playwright comparison)
    #[arg(long)]
    output_json: Option<PathBuf>,

    /// Human-readable task description for the report
    #[arg(long)]
    task: Option<String>,

    /// CSS selectors to click through in sequence, one per interaction step
    /// (comma-separated). When set, runs the multi-step cumulative delta-cost
    /// scenario (issue #173) instead of the single-call comparison.
    #[arg(long, value_delimiter = ',')]
    step_selectors: Option<Vec<String>>,
}

impl Args {
    fn run_count(&self) -> u32 {
        self.runs.unwrap_or(if self.governance { 20 } else { 3 })
    }

    fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.governance || self.run_count() > 0,
            "governance --runs must be positive"
        );
        Ok(())
    }

    fn legacy_url(&self) -> &str {
        self.url
            .as_deref()
            .expect("clap requires --url outside governance mode")
    }
}

/// Installs a stderr-only `tracing` subscriber (ISSUE-206 follow-up, Codex review): without one,
/// every `tracing::info!`/`warn!`/`error!` event `core-runtime` emits (audit mirroring, plugin
/// hook decisions, policy blocks, etc.) is a silent no-op for this binary, the same gap that
/// motivated the fix in `dragon-head-mcp`. `dragon-head-bench` has no stdout-framing constraint
/// (it prints its own report to stdout), but keeping diagnostics on stderr still separates them
/// from that report output. Verbosity via `RUST_LOG`, defaulting to `info`.
fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(filter)
        .init();
}

fn main() -> anyhow::Result<()> {
    init_tracing();
    let args = Args::parse();
    args.validate()?;

    if !chrome_available() {
        eprintln!("Error: Chrome not found.");
        eprintln!("Set CHROME_PATH to point to a Chrome/Chromium binary.");
        std::process::exit(1);
    }

    if args.governance {
        return run_governance_mode(&args);
    }

    println!("Benchmarking: {}", args.legacy_url());
    println!("Runs: {}", args.run_count());
    if let Some(t) = &args.task {
        println!("Task: {t}");
    }
    println!();

    if let Some(selectors) = &args.step_selectors {
        return run_multi_step_mode(&args, selectors);
    }

    let mut results = Vec::with_capacity(args.run_count() as usize);
    for i in 0..args.run_count() {
        let r = harness::run_one(args.legacy_url(), i);
        eprintln!(
            "  Run {}/{}: raw={}B sre={}B",
            r.run + 1,
            args.run_count(),
            r.raw_html_bytes,
            r.sre_bytes
        );
        results.push(r);
    }

    let metrics = metrics::aggregate(&results);
    report::print_table(&metrics);

    if let Some(path) = &args.output {
        report::write_markdown(&metrics, args.legacy_url(), args.task.as_deref(), path)?;
        eprintln!("Report written to {}", path.display());
    }

    if let Some(path) = &args.output_json {
        report::write_json(&metrics, args.legacy_url(), path)?;
        eprintln!("JSON report written to {}", path.display());
    }

    Ok(())
}

fn run_governance_mode(args: &Args) -> anyhow::Result<()> {
    let mcp_bin = args
        .mcp_bin
        .as_deref()
        .expect("clap requires governance --mcp-bin");
    let mut results = Vec::with_capacity(args.run_count() as usize);
    for run in 0..args.run_count() {
        let result = harness::run_governance_pair(mcp_bin, run);
        eprintln!(
            "  Pair {}/{}: raw completed={} SRE completed={}",
            run + 1,
            args.run_count(),
            result.raw.completed,
            result.sre.completed
        );
        results.push(result);
    }
    let mut report = report::GovernanceReport::new(results);
    let (version, hash) = bench::governance::fixture_identity();
    report.fixture_version = Some(version.to_owned());
    report.fixture_sha256 = Some(hash);
    report::print_governance_table(&report);
    if let Some(path) = &args.output {
        report::write_governance_markdown(&report, path)?;
        eprintln!("Governance report written to {}", path.display());
    }
    if let Some(path) = &args.output_json {
        report::write_governance_json(&report, path)?;
        eprintln!("Governance JSON written to {}", path.display());
    }
    Ok(())
}

fn run_multi_step_mode(args: &Args, selectors: &[String]) -> anyhow::Result<()> {
    let selector_refs: Vec<&str> = selectors.iter().map(String::as_str).collect();

    let mut results = Vec::with_capacity(args.run_count() as usize);
    for i in 0..args.run_count() {
        let r = harness::run_multi_step(args.legacy_url(), &selector_refs, i);
        let step_bytes: Vec<usize> = r.steps.iter().map(|s| s.bytes).collect();
        let step_kinds: Vec<&str> = r.steps.iter().map(|s| s.kind.as_str()).collect();
        eprintln!(
            "  Run {}/{}: step_bytes={:?} step_kinds={:?}",
            r.run + 1,
            args.run_count(),
            step_bytes,
            step_kinds
        );
        results.push(r);
    }

    // Surface a representative run's per-step StateUpdate kinds so a delta
    // fallback to full mid-sequence is visible in the report, not just the
    // aggregated byte counts.
    let sample_kinds: Vec<&str> = results
        .iter()
        .find(|r| r.success)
        .map(|r| r.steps.iter().map(|s| s.kind.as_str()).collect())
        .unwrap_or_default();

    let metrics = metrics::aggregate_multi_step(&results);
    report::print_multi_step_table(&metrics, &sample_kinds);

    if let Some(path) = &args.output {
        report::write_multi_step_markdown(
            &metrics,
            &sample_kinds,
            args.legacy_url(),
            args.task.as_deref(),
            path,
        )?;
        eprintln!("Report written to {}", path.display());
    }

    if let Some(path) = &args.output_json {
        report::write_multi_step_json(&metrics, &sample_kinds, args.legacy_url(), path)?;
        eprintln!("JSON report written to {}", path.display());
    }

    Ok(())
}
