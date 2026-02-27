use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use thiserror::Error;
use wasmtime::{Engine, Module};

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

#[derive(Debug, Clone)]
pub struct PluginHost {
    engine: Engine,
    key_registry: KeyRegistry,
}

impl Default for PluginHost {
    fn default() -> Self {
        Self::new(KeyRegistry::default())
    }
}

impl PluginHost {
    pub fn new(key_registry: KeyRegistry) -> Self {
        Self {
            engine: Engine::default(),
            key_registry,
        }
    }

    pub fn load_plugin(&self, package: &PluginPackage) -> Result<LoadedPlugin, PluginError> {
        validate_manifest(&package.manifest)?;
        validate_sbom(&package.manifest.sbom)?;
        self.verify_signature(package)?;

        let module = Module::new(&self.engine, &package.wasm_module).map_err(|err| {
            PluginError::WasmValidation {
                message: err.to_string(),
            }
        })?;

        for extension in &package.manifest.entry_points {
            if module.get_export(extension.export_name()).is_none() {
                return Err(PluginError::MissingExport {
                    extension: *extension,
                });
            }
        }

        Ok(LoadedPlugin {
            manifest: package.manifest.clone(),
        })
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

#[derive(Debug, Clone)]
pub struct LoadedPlugin {
    manifest: PluginManifest,
}

impl LoadedPlugin {
    pub fn manifest(&self) -> &PluginManifest {
        &self.manifest
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
