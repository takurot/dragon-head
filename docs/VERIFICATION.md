# Verification Report

Date: 2026-03-20

Verified target: latest `origin/main` at commit `ab8dbe7be42925bba0759936bc65f41e163c24ac`

Working branch: `codex/e2e-verification`

Primary references:
- `README.md`
- `docs/SPEC.md`
- `docs/testing.md`
- `.github/workflows/ci.yml`
- `.github/workflows/e2e.yml`

Environment:
- Host timezone: `Asia/Tokyo`
- Workspace: `/Volumes/Storage/src/dragon-head`
- Browser: `/Applications/Google Chrome.app/Contents/MacOS/Google Chrome`

## Objective

Run a comprehensive verification pass against the latest `main`, covering:
- workspace quality gates
- browser-backed integration and end-to-end tests
- full evaluation bench scenarios across core crates
- full non-functional regression gates

## Commands Executed

```bash
cargo fmt --all -- --check
cargo clippy --workspace -- -D warnings

CHROME="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
  cargo test -p core-runtime --test cdp_connectivity -- --nocapture

CHROME="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
  cargo test --workspace

CHROME="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
  cargo test --workspace --no-run

CHROME="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
  CARGO_INCREMENTAL=0 \
  cargo test --workspace > target/verification-workspace-test.log 2>&1

CARGO_INCREMENTAL=0 \
  just evaluation-bench-full > target/verification-evaluation-bench.log 2>&1

export CHROME="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
export CARGO_INCREMENTAL=0
export NFR_BENCH_MODE=full
export NFR_METRICS_DIR="$PWD/core-runtime/target/nfr-metrics"
export NFR_DASHBOARD_PATH="$PWD/core-runtime/target/nfr-dashboard-full.md"
export TTFT_LONG_ITERATIONS=400
export TTFT_WARN_MS=40
export TTFT_FAIL_MS=50
export NFR_LATENCY_TRIALS=60
export NFR_LATENCY_P95_LIMIT_MS=100
export NFR_LATENCY_P99_LIMIT_MS=130
export NFR_BANDWIDTH_TRIALS=10
export NFR_BANDWIDTH_REDUCTION_MIN_PERCENT=95
export NFR_CAPACITY_TRIALS=4
export NFR_CAPACITY_MINIMAL_SESSIONS=75
export NFR_CAPACITY_VISUAL_SESSIONS=20
export NFR_CAPACITY_MIN_SUCCESS_RATE_PERCENT=100

cargo test -p core-runtime --test async_pipeline \
  test_async_pipeline_ttft_benchmark_long_gate -- --ignored \
  >> target/verification-nfr.log 2>&1

cargo test -p core-runtime --test nfr_latency \
  >> target/verification-nfr.log 2>&1

cargo test -p core-runtime --test nfr_bandwidth \
  >> target/verification-nfr.log 2>&1

cargo test -p core-runtime --test nfr_capacity \
  >> target/verification-nfr.log 2>&1

python3 scripts/nfr_dashboard.py \
  --metrics-dir "$NFR_METRICS_DIR" \
  --output "$NFR_DASHBOARD_PATH" \
  >> target/verification-nfr.log 2>&1
```

## Result Summary

| Area | Result | Notes |
| --- | --- | --- |
| Formatting | PASS | `cargo fmt --all -- --check` |
| Lint | PASS | `cargo clippy --workspace -- -D warnings` |
| Browser connectivity smoke test | PASS | `core-runtime/tests/cdp_connectivity.rs` |
| Default workspace test command | FAIL | Incremental compilation fails with `dep-graph.part.bin` errors |
| Workspace tests with `CARGO_INCREMENTAL=0` | PASS | Full workspace suite completed successfully |
| Full evaluation bench | PASS | All 16 full scenarios passed across 5 crates |
| Full NFR gates | FAIL | `nfr_latency` breached thresholds; other full NFR suites passed |

## Functional Coverage

### Workspace/browser-backed suites

The full workspace run completed successfully with `CARGO_INCREMENTAL=0`, including browser-backed and system-level tests such as:
- `cdp_connectivity`
- `semantic_wait`
- `session_management`
- `audit_logging`
- `som_event_driven`
- `spa_stable_key_stress`
- `som_visual_diff`
- `policy_enforcement`
- `policy_engine`
- `repro_issue`
- `mcp_hitl_flow`
- `mcp_client_contract`
- `mcp_protocol_compliance`

### Full evaluation bench coverage

`just evaluation-bench-full` passed end-to-end scenarios across the major product surfaces:

| Crate | Scenario | Result |
| --- | --- | --- |
| `core-runtime` | `semantic_state_capture` | PASS |
| `core-runtime` | `stable_key_recovery` | PASS |
| `core-runtime` | `semantic_wait` | PASS |
| `core-runtime` | `policy_hitl` | PASS |
| `core-runtime` | `audit_redaction` | PASS |
| `core-runtime` | `session_vault_roundtrip` | PASS |
| `mcp-server` | `tool_flow_state_and_act` | PASS |
| `mcp-server` | `hitl_flow` | PASS |
| `mcp-server` | `usage_report_plan_gating` | PASS |
| `plugin-host` | `unsigned_plugin_rejected` | PASS |
| `plugin-host` | `signed_plugin_capability_enforcement` | PASS |
| `skills-engine` | `skill_happy_path` | PASS |
| `skills-engine` | `verify_failure_suppresses_act` | PASS |
| `skills-engine` | `retry_branch_and_handoff` | PASS |
| `marketplace` | `domain_pack_signature_verification` | PASS |
| `marketplace` | `revenue_share_accounting` | PASS |

Interpretation: latest `main` functionally exercises the intended user-facing flows across runtime capture/waiting, HITL policy gates, audit handling, session persistence, MCP server orchestration, plugin trust/capability enforcement, skill orchestration, and marketplace verification/accounting.

## Non-Functional Results

Full benchmark artifacts were generated under `core-runtime/target/nfr-metrics/` and summarized in `core-runtime/target/nfr-dashboard-full.md`.

| NFR suite | Result | Key metrics |
| --- | --- | --- |
| `ttft-long` | PASS | `avg=0.067ms`, `p95=0.125ms`, `p99=0.158ms`, `max=0.182ms` |
| `nfr-bandwidth` | PASS | `reduction_avg_pct=99.165`, `reduction_min_pct=98.961` |
| `nfr-capacity-minimal` | PASS | `sessions_target=75`, `success_rate_min=1.000`, `capture_p95_ms=113.061` |
| `nfr-capacity-visual` | PASS | `sessions_target=20`, `success_rate_min=1.000`, `capture_p95_ms=107.037` |
| `nfr-latency` | FAIL | `avg_ms=112.443`, `p95_ms=124.123`, `p99_ms=220.186`, `max_ms=220.186` |

The blocking NFR failure was:

```text
NFR latency benchmark mode=full trials=60 avg=112.443ms p95=124.123ms p99=220.186ms max=220.186ms limits(p95<=100ms,p99<=130ms)
State Update Latency p95 regression: expected <= 100ms, got 124.123ms
```

## Findings And Registered Issues

### 1. Default workspace verification is unstable with incremental compilation

Issue: [#33](https://github.com/takurot/dragon-head/issues/33) `cargo test --workspace fails with incremental dep-graph.part.bin errors on latest main`

Reproduction:

```bash
cargo test --workspace
# or
cargo test --workspace --no-run
```

Observed failure pattern:

```text
failed to create dependency graph at .../target/debug/incremental/.../dep-graph.part.bin: No such file or directory (os error 2)
```

Impact:
- breaks the default local verification path on latest `main`
- affects multiple crates during a single run
- requires `CARGO_INCREMENTAL=0` as a workaround

Workaround validated during this verification:

```bash
CARGO_INCREMENTAL=0 cargo test --workspace
```

### 2. Full latency gate regressed beyond configured limits

Issue: [#32](https://github.com/takurot/dragon-head/issues/32) `Full nfr_latency benchmark fails p95/p99 thresholds on latest main`

Observed metrics:
- `p95_ms = 124.123` against `p95_ms_max = 100.0`
- `p99_ms = 220.186` against `p99_ms_max = 130.0`

Impact:
- full NFR verification is not green on latest `main`
- performance regression appears isolated to state update latency; other full NFR suites passed

## Overall Verdict

Functional E2E coverage on latest `main` is broadly healthy:
- full evaluation bench passed
- full workspace tests passed with incremental compilation disabled
- browser-backed flows, policy gates, MCP flows, plugin enforcement, skills flows, and marketplace scenarios were all exercised successfully

Latest `main` is not fully verification-clean because two issues remain:
- the default workspace Rust test flow is unstable under incremental compilation
- full latency NFR thresholds are currently not met

## Artifacts

- `target/verification-workspace-test.log`
- `target/verification-evaluation-bench.log`
- `target/evaluation-dashboard.md`
- `target/verification-nfr.log`
- `core-runtime/target/nfr-dashboard-full.md`
- `core-runtime/target/nfr-metrics/ttft-long.json`
- `core-runtime/target/nfr-metrics/nfr-latency.json`
- `core-runtime/target/nfr-metrics/nfr-bandwidth.json`
- `core-runtime/target/nfr-metrics/nfr-capacity-minimal.json`
- `core-runtime/target/nfr-metrics/nfr-capacity-visual.json`
