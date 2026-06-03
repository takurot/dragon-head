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

    /// Human-readable task description for the report
    #[arg(long)]
    task: Option<String>,
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

    Ok(())
}
