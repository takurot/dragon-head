mod harness;
mod metrics;
mod report;

use clap::Parser;
use core_runtime::chrome_available;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "dragon-head-bench",
    about = "Side-by-side ROI comparison: Raw DOM vs Dragon Head SRE"
)]
struct Args {
    /// URL to benchmark
    #[arg(long)]
    url: String,

    /// Number of measurement runs
    #[arg(long, default_value_t = 3)]
    runs: u32,

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

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    if !chrome_available() {
        eprintln!("Error: Chrome not found.");
        eprintln!("Set CHROME_PATH to point to a Chrome/Chromium binary.");
        std::process::exit(1);
    }

    println!("Benchmarking: {}", args.url);
    println!("Runs: {}", args.runs);
    if let Some(t) = &args.task {
        println!("Task: {t}");
    }
    println!();

    if let Some(selectors) = &args.step_selectors {
        return run_multi_step_mode(&args, selectors);
    }

    let mut results = Vec::with_capacity(args.runs as usize);
    for i in 0..args.runs {
        let r = harness::run_one(&args.url, i);
        eprintln!(
            "  Run {}/{}: raw={}B sre={}B",
            r.run + 1,
            args.runs,
            r.raw_html_bytes,
            r.sre_bytes
        );
        results.push(r);
    }

    let metrics = metrics::aggregate(&results);
    report::print_table(&metrics);

    if let Some(path) = &args.output {
        report::write_markdown(&metrics, &args.url, args.task.as_deref(), path)?;
        eprintln!("Report written to {}", path.display());
    }

    if let Some(path) = &args.output_json {
        report::write_json(&metrics, &args.url, path)?;
        eprintln!("JSON report written to {}", path.display());
    }

    Ok(())
}

fn run_multi_step_mode(args: &Args, selectors: &[String]) -> anyhow::Result<()> {
    let selector_refs: Vec<&str> = selectors.iter().map(String::as_str).collect();

    let mut results = Vec::with_capacity(args.runs as usize);
    for i in 0..args.runs {
        let r = harness::run_multi_step(&args.url, &selector_refs, i);
        eprintln!(
            "  Run {}/{}: step_bytes={:?} step_kinds={:?}",
            r.run + 1,
            args.runs,
            r.step_bytes,
            r.step_kinds
        );
        results.push(r);
    }

    // Surface a representative run's per-step StateUpdate kinds so a delta
    // fallback to full mid-sequence is visible in the report, not just the
    // aggregated byte counts.
    let sample_kinds: Vec<&str> = results
        .iter()
        .find(|r| r.success)
        .map(|r| r.step_kinds.clone())
        .unwrap_or_default();

    let metrics = metrics::aggregate_multi_step(&results);
    report::print_multi_step_table(&metrics, &sample_kinds);

    if let Some(path) = &args.output {
        report::write_multi_step_markdown(
            &metrics,
            &sample_kinds,
            &args.url,
            args.task.as_deref(),
            path,
        )?;
        eprintln!("Report written to {}", path.display());
    }

    if let Some(path) = &args.output_json {
        report::write_multi_step_json(&metrics, &sample_kinds, &args.url, path)?;
        eprintln!("JSON report written to {}", path.display());
    }

    Ok(())
}
