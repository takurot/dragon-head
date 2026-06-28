# Known Constraints

**Last updated:** 2026-06-28

This document lists current operational limitations that users should account
for when deciding whether Dragon Head is the right browser runtime for a
workflow.

## First-call token overhead

Dragon Head's first `get_state` payload can be larger than a hand-written
Playwright custom extract because each semantic element carries metadata such
as `stable_key`, `alias`, `backend_node_id`, and attributes used for reliable
follow-up actions.

The current benchmark report shows that Dragon Head's primary token advantage
is expected in multi-step workflows, where the first full state is followed by
small RFC 6902 delta payloads.

Benchmark link: [`docs/bench-playwright-comparison.md`](bench-playwright-comparison.md).

Use Dragon Head when selector stability, policy enforcement, HITL, audit, or
delta delivery matter. For one-off extraction from a simple page, a minimal
Playwright custom extract can be smaller.

## Pages requiring `LoadProfile::Interactive`

Some pages require `LoadProfile::Interactive` or a forced refresh to produce a
usable semantic state:

- heavy SPAs that populate controls after client-side hydration;
- login-gated flows that render meaningful controls only after authentication;
- pages whose controls are disabled until asynchronous validation completes;
- pages where hidden or virtualized controls become available only after user
  interaction.

Lighter profiles are better for cost and latency, but they can under-report
interactive controls on these page classes. Use `verify` before risky actions
and prefer `Interactive` for workflows where missing a control is worse than a
larger state payload.

## Prompt injection detection is phrase-list based

Prompt injection detection is a defense-in-depth signal, not a complete
defense. The sanitizer scans labels, aliases, and attributes for built-in
known-risk phrases plus optional `prompt_injection.additional_phrases` literals
from `config.toml`.

Detection normalizes a copy of the scanned text for HTML entities, NFKC,
zero-width/control characters, and common confusables. It does not provide
custom regular expressions, ML classification, or a guarantee that all indirect
prompt injections will be found.

Operational guidance:

- Keep `prompt_injection.mode = "report_only"` unless a workflow has been
  tested with redacted page text.
- Treat `security_flags` as a risk signal for the agent and surrounding policy,
  not as proof that the page is safe.
- Add workflow-specific literals to `prompt_injection.additional_phrases` when
  a deployment repeatedly sees domain-specific attack strings.

## Wasm plugin signatures

Wasm plugins are expected to pass manifest validation, capability declaration,
SBOM checks, and signature verification before execution. This is intentional:
plugins run at a trust boundary that can observe state or influence actions.

Unsigned or incorrectly signed plugins should be rejected rather than bypassed
in production. During development, use test fixtures or a development signing
key and keep that key out of production trust roots.

## Chrome/Chromium compatibility

Dragon Head launches Chrome/Chromium through CDP via the `headless_chrome`
crate. Compatibility depends on both the installed browser and the protocol
surface exercised by a workflow.

Supported operating assumptions:

- Use a current stable Chrome or Chromium build where possible.
- Run `dragon-head-mcp --doctor` on every host before configuring an MCP
  client.
- Set `CHROME_PATH` when a host has multiple browser builds or a non-standard
  install location.
- Browser-dependent tests are skipped unless Chrome is available and
  `CHROME_INSTALLED=true` is set for the test run.

Known risk: a browser update can change CDP behavior or timing. If a workflow
starts failing after a Chrome update, rerun `--doctor`, reproduce with a narrow
`cargo test -p core-runtime --test <test_name>` when possible, and pin or
rollback the browser while investigating.
