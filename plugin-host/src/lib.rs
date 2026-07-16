pub mod schema_registry;
pub use schema_registry::{ExtractionMode, ExtractionRule, ExtractionRuleError, SchemaRegistry};

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;
use thiserror::Error;
use wasmtime::{Engine, Instance, Linker, Module, Store, StoreLimits, StoreLimitsBuilder};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionPoint {
    OnState,
    BeforeAct,
    Connector,
}

impl ExtensionPoint {
    pub fn export_name(self) -> &'static str {
        match self {
            Self::OnState => "on_state",
            Self::BeforeAct => "before_act",
            Self::Connector => "connector",
        }
    }

    fn required_capability(self) -> Option<Capability> {
        match self {
            Self::OnState => Some(Capability::ReadState),
            Self::BeforeAct => None,
            Self::Connector => Some(Capability::NetworkOut),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    ReadState,
    NetworkOut,
    VaultAccess,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SignatureBlock {
    pub key_id: String,
    pub signature_hex: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SbomComponent {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub license: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SbomDocument {
    pub format: String,
    pub components: Vec<SbomComponent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginManifest {
    pub plugin_id: String,
    pub version: String,
    pub entry_points: Vec<ExtensionPoint>,
    pub capabilities: Vec<Capability>,
    #[serde(default)]
    pub signature: Option<SignatureBlock>,
    pub sbom: SbomDocument,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginPackage {
    pub manifest: PluginManifest,
    pub wasm_module: Vec<u8>,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PluginError {
    #[error("plugin manifest validation failed: {message}")]
    ManifestValidation { message: String },
    #[error("unsigned plugin is not allowed")]
    UnsignedPlugin,
    #[error("unknown signing key: {key_id}")]
    UnknownSigningKey { key_id: String },
    #[error("invalid ed25519 public key: {reason}")]
    InvalidPublicKey { reason: String },
    #[error("invalid signature encoding: {reason}")]
    InvalidSignatureEncoding { reason: String },
    #[error("plugin signature verification failed")]
    InvalidSignature,
    #[error("invalid sbom: {reason}")]
    InvalidSbom { reason: String },
    #[error("wasm validation failed: {message}")]
    WasmValidation { message: String },
    #[error("plugin does not export required extension point: {extension:?}")]
    MissingExport { extension: ExtensionPoint },
    #[error("capability violation: required={required:?}")]
    CapabilityViolation { required: Capability },
    #[error("failed to serialize signature payload: {message}")]
    SignaturePayloadSerialization { message: String },
    #[error("wasm execution failed: {message}")]
    ExecutionFailed { message: String },
    #[error(
        "plugin payload too large for {operation}: {size} bytes exceeds {limit} bytes ABI limit"
    )]
    PayloadTooLarge {
        operation: String,
        size: usize,
        limit: usize,
    },
    #[error("invalid wasm output: {message}")]
    InvalidOutput { message: String },
    #[error("wasm execution timed out (fuel exhausted or epoch interrupted)")]
    Timeout,
    #[error("wasm memory limit exceeded")]
    MemoryExhausted,
}

#[derive(Debug, Clone, Serialize)]
struct SignaturePayload<'a> {
    plugin_id: &'a str,
    version: &'a str,
    entry_points: &'a [ExtensionPoint],
    capabilities: &'a [Capability],
    sbom: &'a SbomDocument,
    wasm_sha256: String,
}

pub fn signature_payload(
    manifest: &PluginManifest,
    wasm_module: &[u8],
) -> Result<Vec<u8>, PluginError> {
    let payload = SignaturePayload {
        plugin_id: &manifest.plugin_id,
        version: &manifest.version,
        entry_points: &manifest.entry_points,
        capabilities: &manifest.capabilities,
        sbom: &manifest.sbom,
        wasm_sha256: hex::encode(Sha256::digest(wasm_module)),
    };

    serde_json::to_vec(&payload).map_err(|err| PluginError::SignaturePayloadSerialization {
        message: err.to_string(),
    })
}

#[derive(Debug, Clone, Default)]
pub struct KeyRegistry {
    keys: HashMap<String, VerifyingKey>,
}

impl KeyRegistry {
    pub fn register_hex_ed25519(
        &mut self,
        key_id: &str,
        public_key_hex: &str,
    ) -> Result<(), PluginError> {
        let decoded = hex::decode(public_key_hex).map_err(|err| PluginError::InvalidPublicKey {
            reason: err.to_string(),
        })?;

        let bytes: [u8; 32] = decoded
            .try_into()
            .map_err(|_| PluginError::InvalidPublicKey {
                reason: "expected 32-byte ed25519 public key".to_string(),
            })?;

        let key =
            VerifyingKey::from_bytes(&bytes).map_err(|err| PluginError::InvalidPublicKey {
                reason: err.to_string(),
            })?;

        self.keys.insert(key_id.to_string(), key);
        Ok(())
    }

    fn get(&self, key_id: &str) -> Option<&VerifyingKey> {
        self.keys.get(key_id)
    }
}

// ---------------------------------------------------------------------------
// EpochDriver — background thread that increments the wasmtime epoch counter.
// ---------------------------------------------------------------------------

/// How often the epoch counter advances. Each tick grants one unit of time budget.
const EPOCH_TICK_INTERVAL: Duration = Duration::from_millis(10);

/// Number of epoch ticks allowed per Wasm call: 5 × 10 ms = 50 ms wall-clock budget.
const EPOCH_TICKS_PER_CALL: u64 = 5;

struct EpochDriver {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl EpochDriver {
    fn start(engine: Arc<Engine>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_clone = Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            while !stop_clone.load(Ordering::Relaxed) {
                std::thread::sleep(EPOCH_TICK_INTERVAL);
                engine.increment_epoch();
            }
        });
        Self {
            stop,
            handle: Some(handle),
        }
    }
}

impl Drop for EpochDriver {
    fn drop(&mut self) {
        // SAFETY: Relaxed is sufficient — no data is guarded by this flag; it is a
        // pure cancellation signal. The join() below provides the happens-before
        // barrier that ensures the thread has stopped before Engine is dropped.
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

// ---------------------------------------------------------------------------
// PluginHost — owns the shared Engine, module cache, and epoch driver.
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct PluginHost {
    engine: Arc<Engine>,
    key_registry: KeyRegistry,
    /// Compiled modules keyed by SHA-256 hex of their Wasm bytes.
    module_cache: Arc<Mutex<HashMap<String, Arc<Module>>>>,
    /// Keeps the epoch-increment thread alive for the lifetime of the host.
    _epoch_driver: Arc<EpochDriver>,
}

impl Default for PluginHost {
    fn default() -> Self {
        Self::new(KeyRegistry::default())
    }
}

impl PluginHost {
    pub fn new(key_registry: KeyRegistry) -> Self {
        let mut config = wasmtime::Config::new();
        config.consume_fuel(true);
        config.epoch_interruption(true);
        let engine = Arc::new(Engine::new(&config).expect("failed to create wasmtime engine"));
        let epoch_driver = Arc::new(EpochDriver::start(Arc::clone(&engine)));
        Self {
            engine,
            key_registry,
            module_cache: Arc::new(Mutex::new(HashMap::new())),
            _epoch_driver: epoch_driver,
        }
    }

    pub fn load_plugin(&self, package: &PluginPackage) -> Result<LoadedPlugin, PluginError> {
        validate_manifest(&package.manifest)?;
        validate_sbom(&package.manifest.sbom)?;
        self.verify_signature(package)?;

        let module = self.get_or_compile_module(&package.wasm_module)?;

        for extension in &package.manifest.entry_points {
            if module.get_export(extension.export_name()).is_none() {
                return Err(PluginError::MissingExport {
                    extension: *extension,
                });
            }
        }

        Ok(LoadedPlugin {
            manifest: package.manifest.clone(),
            module,
            engine: Arc::clone(&self.engine),
        })
    }

    /// Return a cached compiled module, or compile and cache on first use.
    ///
    /// The lock is held across miss + compile + insert to prevent two concurrent
    /// callers from both compiling the same module (expensive, redundant work).
    fn get_or_compile_module(&self, wasm_bytes: &[u8]) -> Result<Arc<Module>, PluginError> {
        let cache_key = hex::encode(Sha256::digest(wasm_bytes));
        let mut cache = match self.module_cache.lock() {
            Ok(cache) => cache,
            Err(poisoned) => {
                let mut cache = poisoned.into_inner();
                cache.clear();
                self.module_cache.clear_poison();
                cache
            }
        };

        if let Some(module) = cache.get(&cache_key) {
            return Ok(Arc::clone(module));
        }

        let module = Arc::new(Module::new(&self.engine, wasm_bytes).map_err(|err| {
            PluginError::WasmValidation {
                message: err.to_string(),
            }
        })?);
        cache.insert(cache_key, Arc::clone(&module));
        Ok(module)
    }

    fn verify_signature(&self, package: &PluginPackage) -> Result<(), PluginError> {
        let signature = package
            .manifest
            .signature
            .as_ref()
            .ok_or(PluginError::UnsignedPlugin)?;

        let verifying_key = self.key_registry.get(&signature.key_id).ok_or_else(|| {
            PluginError::UnknownSigningKey {
                key_id: signature.key_id.clone(),
            }
        })?;

        let payload = signature_payload(&package.manifest, &package.wasm_module)?;
        let signature_bytes = hex::decode(&signature.signature_hex).map_err(|err| {
            PluginError::InvalidSignatureEncoding {
                reason: err.to_string(),
            }
        })?;

        let parsed_signature = Signature::from_slice(&signature_bytes).map_err(|err| {
            PluginError::InvalidSignatureEncoding {
                reason: err.to_string(),
            }
        })?;

        verifying_key
            .verify(&payload, &parsed_signature)
            .map_err(|_| PluginError::InvalidSignature)
    }
}

// ---------------------------------------------------------------------------
// LoadedPlugin — verified plugin with a pre-compiled module ready for instantiation.
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct LoadedPlugin {
    manifest: PluginManifest,
    /// Pre-compiled module from verified Wasm bytes. Shared via Arc to avoid re-compilation.
    module: Arc<Module>,
    /// Shared engine from the PluginHost that verified this plugin.
    engine: Arc<Engine>,
}

impl std::fmt::Debug for LoadedPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadedPlugin")
            .field("manifest", &self.manifest)
            .finish_non_exhaustive()
    }
}

impl LoadedPlugin {
    pub fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    /// Create a `PluginRuntime` from this verified plugin.
    ///
    /// Uses the pre-compiled module that was built from verified Wasm bytes,
    /// preventing signature bypass and eliminating per-call compilation overhead.
    pub fn create_runtime(&self) -> Result<PluginRuntime, PluginError> {
        PluginRuntime::from_verified(self)
    }

    pub fn authorize_extension(&self, extension: ExtensionPoint) -> Result<(), PluginError> {
        if !self.manifest.entry_points.contains(&extension) {
            return Err(PluginError::MissingExport { extension });
        }

        if let Some(required) = extension.required_capability() {
            self.ensure_capability(required)?;
        }

        Ok(())
    }

    pub fn ensure_capability(&self, required: Capability) -> Result<(), PluginError> {
        if self.manifest.capabilities.contains(&required) {
            return Ok(());
        }

        Err(PluginError::CapabilityViolation { required })
    }
}

// ---------------------------------------------------------------------------
// PluginRuntime — executes extension points in a sandboxed Wasm instance
// ---------------------------------------------------------------------------

/// Maximum fuel units granted per call.  Each Wasm instruction costs 1 unit.
const FUEL_PER_CALL: u64 = 1_000_000_000;

/// Maximum Wasm memory in bytes (64 MiB).
const MAX_MEMORY_BYTES: usize = 64 * 1024 * 1024;

/// Input JSON is written at offset 0.  Maximum input size: 16 KiB.
const MAX_INPUT_SIZE: usize = 16 * 1024;
/// Output buffer starts right after the max input area.
const OUTPUT_OFFSET: i32 = MAX_INPUT_SIZE as i32;
/// Maximum output buffer size: 16 KiB.
const MAX_OUTPUT_SIZE: usize = 16 * 1024;
/// The 4-byte little-endian output length is stored right after the output buffer.
const OUTPUT_LEN_OFFSET: i32 = OUTPUT_OFFSET + MAX_OUTPUT_SIZE as i32;

struct PluginState {
    limits: StoreLimits,
}

/// `PluginRuntime` wraps a verified [`LoadedPlugin`] with a live Wasm instance
/// and exposes execution of the declared extension points.
pub struct PluginRuntime {
    store: Store<PluginState>,
    instance: Instance,
    manifest: PluginManifest,
}

impl PluginRuntime {
    /// Create a `PluginRuntime` from a verified [`LoadedPlugin`].
    ///
    /// Prefer [`LoadedPlugin::create_runtime`] over calling this directly.
    /// Uses the pre-compiled module that is tied to the verified manifest signature.
    fn from_verified(plugin: &LoadedPlugin) -> Result<Self, PluginError> {
        let engine = Arc::clone(&plugin.engine);
        let module = Arc::clone(&plugin.module);

        let limits = StoreLimitsBuilder::new()
            .memory_size(MAX_MEMORY_BYTES)
            .build();

        let mut store = Store::new(&engine, PluginState { limits });
        store.limiter(|state| &mut state.limits);
        store
            .set_fuel(FUEL_PER_CALL)
            .map_err(|err| PluginError::ExecutionFailed {
                message: err.to_string(),
            })?;
        // Set the epoch deadline before instantiation so that a Wasm `(start ...)` function
        // is also subject to the wall-clock budget and does not trap with deadline=0.
        store.set_epoch_deadline(EPOCH_TICKS_PER_CALL);

        let linker: Linker<PluginState> = Linker::new(&engine);
        let instance = linker.instantiate(&mut store, &module).map_err(|err| {
            PluginError::ExecutionFailed {
                message: err.to_string(),
            }
        })?;

        Ok(Self {
            store,
            instance,
            manifest: plugin.manifest().clone(),
        })
    }

    /// Execute the `on_state` extension point with the given JSON state.
    ///
    /// Fails closed: returns [`PluginError::MissingExport`] if `on_state` is not declared,
    /// or [`PluginError::CapabilityViolation`] if the plugin lacks [`Capability::ReadState`].
    pub fn on_state(&mut self, state_json: &str) -> Result<String, PluginError> {
        self.authorize(ExtensionPoint::OnState)?;
        self.call_json_fn("on_state", state_json)
    }

    /// Execute the `before_act` extension point with the given intent JSON.
    ///
    /// Fails closed: returns [`PluginError::MissingExport`] if `before_act` is not declared.
    /// Returns a policy-decision JSON string such as `{"allow":true}`.
    pub fn before_act(&mut self, intent_json: &str) -> Result<String, PluginError> {
        self.authorize(ExtensionPoint::BeforeAct)?;
        self.call_json_fn("before_act", intent_json)
    }

    /// Execute the `connector` extension point with the given request JSON.
    ///
    /// Fails closed: returns [`PluginError::MissingExport`] if `connector` is not declared,
    /// or [`PluginError::CapabilityViolation`] if the plugin lacks [`Capability::NetworkOut`].
    pub fn connector(&mut self, request_json: &str) -> Result<String, PluginError> {
        self.authorize(ExtensionPoint::Connector)?;
        self.call_json_fn("connector", request_json)
    }

    // ------------------------------------------------------------------
    // Private helpers
    // ------------------------------------------------------------------

    fn authorize(&self, extension: ExtensionPoint) -> Result<(), PluginError> {
        if !self.manifest.entry_points.contains(&extension) {
            return Err(PluginError::MissingExport { extension });
        }
        if let Some(required) = extension.required_capability()
            && !self.manifest.capabilities.contains(&required)
        {
            return Err(PluginError::CapabilityViolation { required });
        }
        Ok(())
    }

    /// Write `input` into Wasm memory at offset 0, call `fn_name(0, len, out_ptr, len_ptr)`,
    /// read the output, and validate it as UTF-8 JSON.
    fn call_json_fn(&mut self, fn_name: &str, input: &str) -> Result<String, PluginError> {
        let input_bytes = input.as_bytes();
        let input_len = input_bytes.len() as i32;

        // Replenish fuel and set the epoch deadline before every call.
        self.store
            .set_fuel(FUEL_PER_CALL)
            .map_err(|err| PluginError::ExecutionFailed {
                message: err.to_string(),
            })?;
        self.store.set_epoch_deadline(EPOCH_TICKS_PER_CALL);

        // Write input into Wasm linear memory at offset 0.
        {
            let memory = self
                .instance
                .get_memory(&mut self.store, "memory")
                .ok_or_else(|| PluginError::ExecutionFailed {
                    message: "wasm module has no exported 'memory'".to_string(),
                })?;

            let data = memory.data_mut(&mut self.store);

            if input_bytes.len() > MAX_INPUT_SIZE {
                return Err(PluginError::PayloadTooLarge {
                    operation: fn_name.to_string(),
                    size: input_bytes.len(),
                    limit: MAX_INPUT_SIZE,
                });
            }
            data[..input_bytes.len()].copy_from_slice(input_bytes);
        }

        // Call the Wasm function: (in_ptr=0, in_len, out_ptr, out_len_ptr)
        let func = self
            .instance
            .get_typed_func::<(i32, i32, i32, i32), ()>(&mut self.store, fn_name)
            .map_err(|err| PluginError::ExecutionFailed {
                message: format!("failed to get export '{fn_name}': {err}"),
            })?;

        func.call(
            &mut self.store,
            (0, input_len, OUTPUT_OFFSET, OUTPUT_LEN_OFFSET),
        )
        .map_err(|err| {
            // Use wasmtime's typed Trap enum for precise classification.
            if let Some(trap) = err.downcast_ref::<wasmtime::Trap>() {
                match trap {
                    wasmtime::Trap::OutOfFuel | wasmtime::Trap::Interrupt => {
                        return PluginError::Timeout;
                    }
                    wasmtime::Trap::MemoryOutOfBounds | wasmtime::Trap::HeapMisaligned => {
                        return PluginError::MemoryExhausted;
                    }
                    _ => {}
                }
            }
            let msg = err.to_string();
            if msg.contains("memory") {
                PluginError::MemoryExhausted
            } else {
                PluginError::ExecutionFailed { message: msg }
            }
        })?;

        // Read output length and bytes from Wasm memory.
        let output = {
            let memory = self
                .instance
                .get_memory(&mut self.store, "memory")
                .ok_or_else(|| PluginError::ExecutionFailed {
                    message: "wasm module has no exported 'memory'".to_string(),
                })?;

            let data = memory.data(&self.store);

            // Validate the memory is large enough to hold the length field.
            let len_slot_start = OUTPUT_LEN_OFFSET as usize;
            let len_slot_end = len_slot_start
                .checked_add(4)
                .filter(|&e| e <= data.len())
                .ok_or_else(|| PluginError::InvalidOutput {
                    message: "wasm memory too small to contain output length field".to_string(),
                })?;

            // Read length as u32 to reject negative values.
            let out_len =
                u32::from_le_bytes(data[len_slot_start..len_slot_end].try_into().unwrap()) as usize;

            if out_len > MAX_OUTPUT_SIZE {
                return Err(PluginError::PayloadTooLarge {
                    operation: format!("{fn_name} output"),
                    size: out_len,
                    limit: MAX_OUTPUT_SIZE,
                });
            }

            let out_start = OUTPUT_OFFSET as usize;
            let out_end = out_start
                .checked_add(out_len)
                .filter(|&e| e <= data.len())
                .ok_or_else(|| PluginError::InvalidOutput {
                    message: "output region extends beyond wasm memory bounds".to_string(),
                })?;

            data[out_start..out_end].to_vec()
        };

        // Validate UTF-8 then valid JSON.
        let output_str =
            std::str::from_utf8(&output).map_err(|err| PluginError::InvalidOutput {
                message: format!("output is not valid UTF-8: {err}"),
            })?;

        serde_json::from_str::<serde_json::Value>(output_str).map_err(|err| {
            PluginError::InvalidOutput {
                message: format!("output is not valid JSON: {err}"),
            }
        })?;

        Ok(output_str.to_string())
    }
}

fn validate_manifest(manifest: &PluginManifest) -> Result<(), PluginError> {
    if manifest.plugin_id.trim().is_empty() {
        return Err(PluginError::ManifestValidation {
            message: "plugin_id must not be empty".to_string(),
        });
    }

    if manifest.version.trim().is_empty() {
        return Err(PluginError::ManifestValidation {
            message: "version must not be empty".to_string(),
        });
    }

    if manifest.entry_points.is_empty() {
        return Err(PluginError::ManifestValidation {
            message: "at least one extension point must be declared".to_string(),
        });
    }

    Ok(())
}

fn validate_sbom(sbom: &SbomDocument) -> Result<(), PluginError> {
    if sbom.format.trim().is_empty() {
        return Err(PluginError::InvalidSbom {
            reason: "sbom format must not be empty".to_string(),
        });
    }

    if sbom.components.is_empty() {
        return Err(PluginError::InvalidSbom {
            reason: "sbom must contain at least one component".to_string(),
        });
    }

    for (index, component) in sbom.components.iter().enumerate() {
        if component.name.trim().is_empty() {
            return Err(PluginError::InvalidSbom {
                reason: format!("component[{index}] name must not be empty"),
            });
        }

        if component.version.trim().is_empty() {
            return Err(PluginError::InvalidSbom {
                reason: format!("component[{index}] version must not be empty"),
            });
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poisoned_module_cache_is_rebuilt_and_remains_usable() {
        let host = PluginHost::default();
        let wasm = wat::parse_str("(module)").unwrap();
        let original = host.get_or_compile_module(&wasm).unwrap();

        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = host.module_cache.lock().unwrap();
            panic!("poison module cache");
        }));
        assert!(panic.is_err());
        assert!(host.module_cache.is_poisoned());

        let rebuilt = host.get_or_compile_module(&wasm).unwrap();
        assert!(!Arc::ptr_eq(&original, &rebuilt));
        assert!(!host.module_cache.is_poisoned());

        let cached = host.get_or_compile_module(&wasm).unwrap();
        assert!(Arc::ptr_eq(&rebuilt, &cached));
    }
}
