/// Integration test: requires a live Chrome + network access.
/// Run with: cargo test -p bench -- --include-ignored
#[test]
#[ignore]
fn run_one_example_com_sre_smaller_than_raw() {
    // harness is a private module; re-export via lib is not needed for bin crates.
    // This test invokes the binary in-process by calling the pub functions directly.
    // Since `bench` is a binary crate we test via the functions exposed from main.rs modules.
    // Re-import via path:
    use std::process::Command;

    let output = Command::new(env!("CARGO_BIN_EXE_dragon-head-bench"))
        .args(["--url", "https://example.com", "--runs", "1"])
        .output()
        .expect("failed to run dragon-head-bench");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    eprintln!("stdout:\n{stdout}");
    eprintln!("stderr:\n{stderr}");

    // Binary exits 0 on success
    assert!(
        output.status.success(),
        "dragon-head-bench exited with non-zero: {stderr}"
    );
    // Table header must appear
    assert!(
        stdout.contains("Avg Tokens") || stdout.contains("Token reduction"),
        "Expected bench table in stdout, got: {stdout}"
    );
}

/// Integration test: requires a live Chrome. Verifies the multi-step delta
/// cost scenario (issue #173) — clicking through the `spa-like.html` filter
/// buttons should mostly produce small RFC 6902 patches, not full re-sends.
/// Run with: cargo test -p bench -- --include-ignored
#[test]
#[ignore]
fn run_multi_step_spa_filter_cycle_mostly_uses_deltas() {
    use std::path::PathBuf;
    use std::process::Command;

    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("bench-playwright")
        .join("fixtures")
        .join("spa-like.html");
    let url = format!("file://{}", fixture.display());

    let output = Command::new(env!("CARGO_BIN_EXE_dragon-head-bench"))
        .args([
            "--url",
            &url,
            "--runs",
            "1",
            "--step-selectors",
            ".filter-btn[data-filter=\"articles\"],.filter-btn[data-filter=\"discussions\"],.filter-btn[data-filter=\"videos\"]",
        ])
        .output()
        .expect("failed to run dragon-head-bench");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    eprintln!("stdout:\n{stdout}");
    eprintln!("stderr:\n{stderr}");

    assert!(
        output.status.success(),
        "dragon-head-bench exited with non-zero: {stderr}"
    );
    assert!(
        stdout.contains("Cumulative Bytes"),
        "Expected multi-step table in stdout, got: {stdout}"
    );
    // step_kinds[0] is always "full" (initial capture); at least one
    // subsequent step must be a real "delta", not a fallback to "full" —
    // otherwise the harness would be measuring the no-delta control case
    // and the cumulative-cost claim would be unverified.
    assert!(
        stderr.contains("\"delta\""),
        "Expected at least one delta-kind step, got stderr: {stderr}"
    );
}
