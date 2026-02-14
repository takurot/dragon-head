# Dragon Head: Neural-Browser Runtime

**Dragon Head** is an AI-Native Headless Browser Runtime designed specifically for Large Language Models (LLMs) and Vision Language Models (VLMs) to navigate and interact with the Web.

Unlike traditional headless browsers that automate "DOM for humans," Dragon Head functions as middleware that converts web pages into **"Semantic State" interpretatable by AI** in real-time.

## Core Value Propositions

- **Token Efficiency**: Reduces input tokens to LLMs by an average of 90% through a proprietary Semantic Rendering Engine (SRE) and differential updates.
- **Reliability**: Prevents AI hallucinations by synchronizing visual information (Set-of-Mark) with structural data and using `stable_key` for self-healing element identification.
- **Compliance**: Built-in Policy Engine and Audit Logs provide a "legitimate execution environment" that meets enterprise security requirements.
- **Speed**: Achieves a Time-to-First-Token (TTFT) of under 50ms using a 3-stage pipeline and aggressive blocking of unnecessary resources.

## System Architecture

The system consists of a 3-layer structure and an asynchronous internal pipeline within the Core Runtime.

### Layer Structure

1.  **Layer 1: Core Runtime (Rust/C++)**
    The foundation that wraps Chromium CDP (Chrome DevTools Protocol) to handle rendering, SRE conversion, and security controls.

2.  **Layer 2: Plugin Framework (WebAssembly)**
    A sandyox environment for extending State extraction and policies tailored to specific sites or business needs.

3.  **Layer 3: Skills Engine**
    An execution engine for declarative workflows defining tasks such as "Purchase Item" or "Job Search."

## Key Features

### Semantic Rendering Engine (SRE)
- **Deterministic State Generation**: Converts raw DOM into structured JSON (Semantic State).
- **Load Profiles**: `minimal` (fastest), `visual` (for SoM), `interactive` (for complex SPAs).
- **Semantic Delta**: Generates JSON Patch (RFC 6902) updates to minimize data transfer.

### Native Set-of-Mark (SoM) & Stable Identity
- **Stable Key**: Generates robust, immutable keys (`stable_key`) using SHA-256 to track elements across re-renders.
- **Event-Driven SoM**: Generates visual marks only when necessary (e.g., explicit request or ambiguity resolution).

### Enterprise Security
- **Context-Aware Policy Engine**: Rule-based screening before action execution.
- **Structured Audit Log**: Records full state snapshots, deltas, tool calls, and policy decisions.
- **Session Vault**: Securely persists authentication credentials (Cookie/Token) using AES-256.

## Developer Guide

### Prerequisites
- Rust (latest stable)
- Chromium installed

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
