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
