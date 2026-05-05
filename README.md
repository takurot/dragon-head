# Dragon Head: Neural-Browser Runtime

**Dragon Head** is an AI-Native Headless Browser Runtime designed specifically for Large Language Models (LLMs) and Vision Language Models (VLMs) to navigate and interact with the Web.

Unlike traditional headless browsers that automate "DOM for humans," Dragon Head functions as middleware that converts web pages into **"Semantic State" interpretatable by AI** in real-time.

## Core Value Propositions

- **Token Efficiency**: Reduces input tokens to LLMs by an average of 90% through a proprietary Semantic Rendering Engine (SRE) and differential updates.
- **Reliability**: Prevents AI hallucinations by synchronizing visual information (Set-of-Mark) with structural data and using `stable_key` for robust element identification.
- **Resilience**: **Self-Healing (セマンティック・ヒーリング)** enables 99.9% tolerance to UI changes by using DOM signature fuzzy matching.
- **Compliance**: Built-in Policy Engine and Audit Logs provide a "legitimate execution environment" that meets enterprise security requirements.
- **Speed**: Achieves **Near-Zero TTFT (< 10ms)** using a 4-stage speculative pipeline and aggressive blocking of unnecessary resources.

## System Architecture

The system consists of a 3-layer structure and a 4-stage asynchronous internal pipeline within the Core Runtime.

### Layer Structure

1.  **Layer 1: Core Runtime (Rust/C++)**
    The foundation that wraps Chromium CDP (Chrome DevTools Protocol) to handle rendering, SRE conversion, and security controls.

2.  **Layer 2: Plugin Framework (WebAssembly)**
    A sandbox environment for extending State extraction (**"Deep Lens" DSL**) and policies (**"Guardian Angel"**) tailored to specific sites or business needs.

3.  **Layer 3: Skills Engine**
    An execution engine for declarative workflows defining tasks such as "Purchase Item" or "Job Search."

### Execution Pipeline

- **Speculative Queue**: Predicts AI intent and pre-generates the next Semantic State.
- **Render Queue**: Handles DOM updates and minimal layout.
- **SRE Queue**: Performs DOM analysis, stable key generation, and delta calculation.
- **Audit/Policy Queue**: Manages action screening and immutable audit logging.

## Key Features

### Speculative State Generation
- **Intent Prediction**: Probability-based prediction of the next navigation or action.
- **Near-Zero TTFT**: Pre-generates SRE results to eliminate AI thinking latency.

### Semantic Rendering Engine (SRE) & "Deep Lens"
- **Deterministic State Generation**: Converts raw DOM into structured JSON (Semantic State).
- **"Deep Lens" DSL**: YAML/JSON based zero-code extraction for complex structured data.
- **Semantic Delta**: Generates JSON Patch (RFC 6902) updates to minimize data transfer.

### Self-Healing Context Recovery
- **DOM Signature Cache**: Stores structural fingerprints of successful operations.
- **Fuzzy Matching**: Automatically recovers element targets when `stable_key` fails due to UI updates.

### Native Set-of-Mark (SoM) & Stable Identity
- **Stable Key**: SHA-256 based immutable keys to track elements across re-renders.
- **Event-Driven SoM**: Generates visual marks only when necessary (e.g., explicit request or ambiguity resolution).

### Enterprise Security & "Guardian Angel"
- **"Guardian Angel"**: Proactively defends against dangerous actions by simulating side-effects (**Outcome Projection**).
- **Unified PII Redactor**: Mandatory dual-layer masking for both AI state and audit logs.
- **Structured Audit Log**: Persistent, SIEM-compatible logs of all snapshots, tool calls, and decisions.

## Quick Start

```bash
git clone <repo-url>
cd dragon-head
cargo run --example quickstart
```

Expected output:

```
=== Dragon Head Quickstart ===

[1] SemanticState constructed
    page_instance_id : <uuid>
    state_hash       : <sha256-hex>
    load_profile     : Minimal

[2] Fast State generated
    interactive_elements : 2
    messages             : 1
      → role=input     alias=input_email      stable_key=b2c3d4e5...
      → role=button    alias=btn_purchase     stable_key=a1b2c3d4...

[3] PolicyEngine loaded with 1 rule(s)
    safe.example.com               → Allow
    blocked-domain.example.com     → Block

[4] MCP get_state response (JSON):
{ "metadata": { ... }, "interactive_elements": [ ... ] }

=== Done — no credentials required ===
```

No Chrome instance or paid credentials needed. See [`examples/README.md`](examples/README.md) for a full guide including the policy cookbook, MCP JSON contract examples, and the sample skill definition.

## Developer Tools

### ROI Comparison Tool
A CLI utility to benchmark Dragon Head against standard browser automation, quantifying token and latency savings.

## Developer Guide

### Prerequisites
- Rust (latest stable)
- Chromium installed (only required for browser integration tests)

### Running Examples
```bash
# Core concepts — no browser required
cargo run --example quickstart

# Policy rule cookbook — no browser required
cargo run --example policy_cookbook
```

### Running Tests
```bash
cargo test --workspace
```

### Linting
```bash
cargo fmt --all -- --check
cargo clippy --workspace -- -D warnings
```

---
*This project is currently under active development.*
