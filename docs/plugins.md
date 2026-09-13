# Local Wasm Plugins (ISSUE-303)

`dragon-head-mcp` can load locally configured, **signed** Wasm plugins and wire them into the
`core_runtime` pipeline: a plugin's `on_state` export can transform the `SemanticState` JSON
emitted by `get_state`, and its `before_act` export can veto an action before it executes. The
underlying execution engine (`plugin-host`), the trait boundary (`core_runtime::plugin_hooks`),
and the Wasm ABI itself are not new — this document covers the piece that was missing until
ISSUE-303: how to get a plugin actually running inside the shipped `dragon-head-mcp` binary.

This is deliberately narrower than a plugin **marketplace**. There is no registry, no install
command, no revenue share — you point `config.toml` at a manifest + Wasm file already on disk.
Marketplace/domain-pack packaging and distribution are tracked separately in ISSUE-155; a future
marketplace installer would still end up writing the same `config.toml` shape this document
describes.

## `config.toml` reference

```toml
[[plugin_trust_keys]]
id = "vendor-key-1"
public_key_hex = "5f2c...  # 64 hex chars, ed25519 public key"

[[plugins]]
manifest = "plugins/example/manifest.json"   # relative to config.toml's own directory
wasm = "plugins/example/plugin.wasm"          # relative to config.toml's own directory
enabled = true                                 # optional, defaults to true
```

- `manifest` deserializes to `plugin_host::PluginManifest` — the same shape used by
  `plugin-host`'s own tests (see `plugin-host/tests/`): `plugin_id`, `version`, `abi_version`
  (the host↔plugin calling-convention version this plugin was built against — see below),
  `entry_points` (`"on_state"` / `"before_act"` / `"connector"`), `capabilities`
  (`"read_state"` / `"network_out"` / `"vault_access"`), a `signature` block, and an `sbom`
  document.
- `abi_version` must fall within `plugin_host::MIN_SUPPORTED_ABI_VERSION..=
  plugin_host::MAX_SUPPORTED_ABI_VERSION` (currently `1..=1`) or the plugin is rejected at load
  time with `PluginError::UnsupportedAbiVersion`, which names the supported range — this is
  deliberately a hard load-time failure, not a warning, so a plugin built against an
  incompatible host↔plugin calling convention never fails opaquely at call time instead
  (ISSUE-208). A manifest with no `abi_version` at all defaults to `0`, which is always outside
  the supported range, so an unversioned/legacy manifest is rejected the same way. `abi_version`
  is part of the signed payload (`signature_payload`), so tampering with it after signing
  invalidates the signature like any other manifest field.
- `wasm` is the plugin's compiled Wasm module bytes, referenced separately from the manifest so
  the manifest stays small and human-readable.
- Both paths may be absolute or relative; relative paths resolve against `config.toml`'s own
  parent directory (same rule as `[skills].files`).
- `enabled = false` removes an entry from the startup path **entirely** — its `manifest`/`wasm`
  files are never opened, so a disabled entry's own misconfiguration (missing file, malformed
  JSON, an oversized module) never blocks startup or `--doctor`. This is the supported way to
  take a broken plugin out of the loop without deleting its config.

### Trust keys are not secrets, but `config.toml` still is

`plugin_trust_keys` entries are ed25519 **public** keys, not credentials — unlike the HITL
bridge's Slack signing secret/bot token (which are deliberately env-var-only, never accepted in
`config.toml`), there is no reason to keep these out of the file. However: **anyone who can edit
`config.toml` can add their own trust key and then supply a self-signed plugin that verifies
against it.** Write access to `config.toml` is therefore equivalent to Wasm-code-execution access
on the `dragon-head-mcp` process (the same trust model `policy.file` already has). Treat
`config.toml` accordingly — don't template it from an untrusted source or share it as if it were
inert configuration.

## What gets verified, and when

Nothing is trusted by construction. Every *enabled* configured plugin goes through
`plugin_host::PluginHost::load_plugin` at both normal startup and `--doctor`:

1. Manifest schema validation.
2. SBOM validation.
3. Ed25519 signature verification against the configured `plugin_trust_keys` registry — an
   **unsigned plugin is rejected**, as is one signed by an unregistered key or with an invalid
   signature. There is no unsigned-by-default mode.
4. Wasm module compilation/validation.
5. Existence of every export the manifest's `entry_points` declare.

A plugin that passes all of that is then wired per declared extension point:

- `"on_state"` → `core_runtime::plugin_hooks::WasmStatePlugin`, added to the pipeline's
  `state_plugins` (invoked after each `normalize_dom` capture; failures are non-fatal and audited
  as `PluginStateTransform`, per `core_runtime::plugin_hooks`'s own doc comment).
- `"before_act"` → `core_runtime::plugin_hooks::WasmPolicyPlugin`, added to `policy_plugins`
  (invoked before an already-policy-approved action executes; any plugin returning
  `{"allow": false}` or failing blocks the action — fail-closed — and is audited as
  `PluginPolicyDecision`).
- `"connector"` is **not yet wired**. A plugin that declares `connector` alongside a supported
  extension point gets a `[PLUGIN][WARN]` line on stderr and keeps its other hooks active. A
  plugin that declares **only** `connector` (or no entry points at all) is a **hard startup
  error** — an enabled plugin that wires zero hooks would otherwise be a silent no-op, which
  ISSUE-303 explicitly requires this composition to reject rather than swallow.

**Any failure in this process — signature, schema, missing export, missing capability, a
Connector-only plugin — fails `dragon-head-mcp` startup and `--doctor` outright.** There is no
silent-skip behavior for an *enabled* plugin; `enabled = false` is the only supported opt-out.

## `--doctor`

`dragon-head-mcp --doctor` runs the exact same load-and-wire path as normal startup (it is not a
separate, lighter check) and reports `plugins.state_hooks=<N>, plugins.policy_hooks=<N>` in its
"Config file" summary line. Because this instantiates a live Wasm store for every wired plugin —
which runs that module's `start` section, if it declares one — `--doctor` is not a purely
read-only operation once plugins are configured. This is intentional: ISSUE-303 asks that
`--doctor` actually exercise "adapter construction succeeds," not just parse the manifest.

## Operational limits

These are hard limits, not soft advisories — a plugin exceeding them fails, it does not degrade:

- At most 16 configured plugin entries; manifest files capped at 64 KiB, Wasm modules at 2 MiB
  (`mcp_server::config::MAX_PLUGINS`/`MAX_PLUGIN_MANIFEST_BYTES`/`MAX_PLUGIN_WASM_BYTES`).
- Per Wasm call: 16 KiB max input (the serialized `SemanticState`/intent JSON) and 16 KiB max
  output, a ~50 ms wall-clock budget (5 epoch ticks × 10 ms), and 64 MiB of linear memory
  (`plugin-host/src/lib.rs`'s `MAX_INPUT_SIZE`/`MAX_OUTPUT_SIZE`/`EPOCH_TICKS_PER_CALL`/
  `MAX_MEMORY_BYTES`). A real page's `SemanticState` can easily exceed 16 KiB — when it does,
  `on_state` fails non-fatally (previous state is preserved, `PluginStateTransform{success:false}`
  is audited) rather than truncating or erroring the request.
- `OnState` and `BeforeAct` adapters for the same plugin are **independent Wasm instances** (each
  gets its own `Store`/linear memory via `WasmStatePlugin::new`/`WasmPolicyPlugin::new`). A
  plugin cannot rely on state persisting from an `on_state` call into a later `before_act` call,
  or vice versa.
- Declared `capabilities` (`read_state`, `network_out`, `vault_access`) gate which extension
  points a plugin may register for (`LoadedPlugin::authorize_extension`/`ensure_capability`) —
  they are **not** a Wasm-level sandbox boundary beyond that. The Wasm linker registers no host
  functions today, so a plugin cannot reach the network or the vault regardless of declared
  capabilities; do not read "capabilities" as an enforced runtime sandbox.

## See also

- `plugin-host/src/lib.rs` — signature verification, manifest/SBOM validation, the Wasm ABI and
  its execution limits.
- `core-runtime/src/plugin_hooks.rs` — the `StatePlugin`/`PolicyPlugin` traits, the
  `WasmStatePlugin`/`WasmPolicyPlugin` adapters, and the fail-open/fail-closed semantics of each
  hook type.
- `mcp-server/src/plugins.rs` — the composition-root wiring described above
  (`build_plugin_hook_config`).
- ISSUE-155 — marketplace/domain-pack packaging, registry, and distribution (out of scope here).
