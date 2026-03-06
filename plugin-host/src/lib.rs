use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeCompatibility {
    pub runtime_api: String,
    pub min_runtime_version: String,
    #[serde(default)]
    pub max_runtime_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MarketplaceDependency {
    pub package: String,
    pub version_req: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DomainPackMarketplaceMetadata {
    pub publisher_id: String,
    pub runtime_compatibility: RuntimeCompatibility,
    #[serde(default)]
    pub dependencies: Vec<MarketplaceDependency>,
    #[serde(default)]
    pub signature: Option<SignatureBlock>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DomainPackManifest {
    pub pack_id: String,
    pub version: String,
    pub plugin_id: String,
    pub skill_ids: Vec<String>,
    pub marketplace: DomainPackMarketplaceMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DomainPackSkill {
    pub skill_id: String,
    pub version: String,
    pub definition: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DomainPackBundle {
    pub manifest: DomainPackManifest,
    pub plugin: PluginPackage,
    pub skills: Vec<DomainPackSkill>,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PluginError {
    #[error("plugin manifest validation failed: {message}")]
    ManifestValidation { message: String },
    #[error("domain pack validation failed: {message}")]
    DomainPackValidation { message: String },
    #[error("unsigned plugin is not allowed")]
    UnsignedPlugin,
    #[error("unsigned domain pack is not allowed")]
    UnsignedDomainPack,
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
    #[error("invalid runtime version for {field}: {value}")]
    InvalidRuntimeVersion { field: &'static str, value: String },
    #[error(
        "runtime version {runtime_version} is outside supported range [{min_supported}, {max_supported}]"
    )]
    IncompatibleRuntimeVersion {
        runtime_version: String,
        min_supported: String,
        max_supported: String,
    },
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

#[derive(Debug, Clone, Serialize)]
struct DomainPackSignaturePayload<'a> {
    pack_id: &'a str,
    version: &'a str,
    plugin_id: &'a str,
    skill_ids: &'a [String],
    publisher_id: &'a str,
    runtime_compatibility: &'a RuntimeCompatibility,
    dependencies: &'a [MarketplaceDependency],
    plugin_signature_payload_sha256: String,
    skills: Vec<DomainPackSkillDigest>,
}

#[derive(Debug, Clone, Serialize)]
struct DomainPackSkillDigest {
    skill_id: String,
    version: String,
    definition_sha256: String,
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

pub fn domain_pack_signature_payload(bundle: &DomainPackBundle) -> Result<Vec<u8>, PluginError> {
    let plugin_payload = signature_payload(&bundle.plugin.manifest, &bundle.plugin.wasm_module)?;
    let plugin_signature_payload_sha256 = hex::encode(Sha256::digest(&plugin_payload));

    let mut skills = bundle
        .skills
        .iter()
        .map(|skill| {
            serde_json::to_vec(&skill.definition)
                .map(|serialized| DomainPackSkillDigest {
                    skill_id: skill.skill_id.clone(),
                    version: skill.version.clone(),
                    definition_sha256: hex::encode(Sha256::digest(serialized)),
                })
                .map_err(|err| PluginError::SignaturePayloadSerialization {
                    message: err.to_string(),
                })
        })
        .collect::<Result<Vec<_>, PluginError>>()?;

    skills.sort_by(|left, right| left.skill_id.cmp(&right.skill_id));

    let payload = DomainPackSignaturePayload {
        pack_id: &bundle.manifest.pack_id,
        version: &bundle.manifest.version,
        plugin_id: &bundle.manifest.plugin_id,
        skill_ids: &bundle.manifest.skill_ids,
        publisher_id: &bundle.manifest.marketplace.publisher_id,
        runtime_compatibility: &bundle.manifest.marketplace.runtime_compatibility,
        dependencies: &bundle.manifest.marketplace.dependencies,
        plugin_signature_payload_sha256,
        skills,
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

    pub fn load_domain_pack(
        &self,
        bundle: &DomainPackBundle,
        runtime_version: &str,
    ) -> Result<LoadedDomainPack, PluginError> {
        validate_domain_pack(bundle)?;
        validate_runtime_compatibility(
            runtime_version,
            &bundle.manifest.marketplace.runtime_compatibility,
        )?;
        self.verify_domain_pack_signature(bundle)?;
        let loaded_plugin = self.load_plugin(&bundle.plugin)?;

        Ok(LoadedDomainPack {
            manifest: bundle.manifest.clone(),
            plugin: loaded_plugin,
            skills: bundle.skills.clone(),
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

    fn verify_domain_pack_signature(&self, bundle: &DomainPackBundle) -> Result<(), PluginError> {
        let signature = bundle
            .manifest
            .marketplace
            .signature
            .as_ref()
            .ok_or(PluginError::UnsignedDomainPack)?;

        let verifying_key = self.key_registry.get(&signature.key_id).ok_or_else(|| {
            PluginError::UnknownSigningKey {
                key_id: signature.key_id.clone(),
            }
        })?;

        let payload = domain_pack_signature_payload(bundle)?;
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

#[derive(Debug, Clone)]
pub struct LoadedDomainPack {
    manifest: DomainPackManifest,
    plugin: LoadedPlugin,
    skills: Vec<DomainPackSkill>,
}

impl LoadedDomainPack {
    pub fn manifest(&self) -> &DomainPackManifest {
        &self.manifest
    }

    pub fn plugin(&self) -> &LoadedPlugin {
        &self.plugin
    }

    pub fn skills(&self) -> &[DomainPackSkill] {
        &self.skills
    }

    pub fn skill_ids(&self) -> Vec<String> {
        self.skills
            .iter()
            .map(|skill| skill.skill_id.clone())
            .collect()
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

fn validate_domain_pack(bundle: &DomainPackBundle) -> Result<(), PluginError> {
    let manifest = &bundle.manifest;

    if manifest.pack_id.trim().is_empty() {
        return Err(PluginError::DomainPackValidation {
            message: "pack_id must not be empty".to_string(),
        });
    }

    if manifest.version.trim().is_empty() {
        return Err(PluginError::DomainPackValidation {
            message: "version must not be empty".to_string(),
        });
    }

    if manifest.plugin_id.trim().is_empty() {
        return Err(PluginError::DomainPackValidation {
            message: "plugin_id must not be empty".to_string(),
        });
    }

    if manifest.plugin_id != bundle.plugin.manifest.plugin_id {
        return Err(PluginError::DomainPackValidation {
            message: format!(
                "domain pack plugin_id '{}' does not match plugin package id '{}'",
                manifest.plugin_id, bundle.plugin.manifest.plugin_id
            ),
        });
    }

    if manifest.skill_ids.is_empty() {
        return Err(PluginError::DomainPackValidation {
            message: "skill_ids must contain at least one skill".to_string(),
        });
    }

    if bundle.skills.is_empty() {
        return Err(PluginError::DomainPackValidation {
            message: "skills bundle must contain at least one skill".to_string(),
        });
    }

    if manifest.marketplace.publisher_id.trim().is_empty() {
        return Err(PluginError::DomainPackValidation {
            message: "publisher_id must not be empty".to_string(),
        });
    }

    for dependency in &manifest.marketplace.dependencies {
        if dependency.package.trim().is_empty() || dependency.version_req.trim().is_empty() {
            return Err(PluginError::DomainPackValidation {
                message: "dependencies must include non-empty package and version_req".to_string(),
            });
        }
    }

    let mut declared_skill_ids = HashSet::new();
    for skill_id in &manifest.skill_ids {
        if skill_id.trim().is_empty() {
            return Err(PluginError::DomainPackValidation {
                message: "skill_ids must not contain empty values".to_string(),
            });
        }
        if !declared_skill_ids.insert(skill_id.clone()) {
            return Err(PluginError::DomainPackValidation {
                message: format!("duplicate skill_id declared: {skill_id}"),
            });
        }
    }

    let mut bundled_skill_ids = HashSet::new();
    for skill in &bundle.skills {
        if skill.skill_id.trim().is_empty() || skill.version.trim().is_empty() {
            return Err(PluginError::DomainPackValidation {
                message: "skill entries must include non-empty skill_id and version".to_string(),
            });
        }
        if !bundled_skill_ids.insert(skill.skill_id.clone()) {
            return Err(PluginError::DomainPackValidation {
                message: format!("duplicate skill bundle entry: {}", skill.skill_id),
            });
        }
    }

    for skill_id in &manifest.skill_ids {
        if !bundled_skill_ids.contains(skill_id) {
            return Err(PluginError::DomainPackValidation {
                message: format!("skill_id '{skill_id}' declared but not bundled"),
            });
        }
    }

    for skill_id in bundled_skill_ids {
        if !declared_skill_ids.contains(&skill_id) {
            return Err(PluginError::DomainPackValidation {
                message: format!("skill '{skill_id}' bundled but missing from skill_ids"),
            });
        }
    }

    Ok(())
}

fn validate_runtime_compatibility(
    runtime_version: &str,
    compatibility: &RuntimeCompatibility,
) -> Result<(), PluginError> {
    if compatibility.runtime_api.trim().is_empty() {
        return Err(PluginError::DomainPackValidation {
            message: "runtime_compatibility.runtime_api must not be empty".to_string(),
        });
    }

    let runtime = parse_runtime_version("runtime_version", runtime_version)?;
    let min_supported = parse_runtime_version(
        "runtime_compatibility.min_runtime_version",
        &compatibility.min_runtime_version,
    )?;

    let max_supported = compatibility
        .max_runtime_version
        .as_ref()
        .map(|raw| {
            parse_runtime_version("runtime_compatibility.max_runtime_version", raw)
                .map(|parsed| (raw.clone(), parsed))
        })
        .transpose()?;

    if runtime < min_supported
        || max_supported
            .as_ref()
            .map(|(_, max)| runtime > *max)
            .unwrap_or(false)
    {
        return Err(PluginError::IncompatibleRuntimeVersion {
            runtime_version: runtime_version.to_string(),
            min_supported: compatibility.min_runtime_version.clone(),
            max_supported: max_supported
                .map(|(raw, _)| raw)
                .unwrap_or_else(|| "unbounded".to_string()),
        });
    }

    Ok(())
}

fn parse_runtime_version(field: &'static str, value: &str) -> Result<Version, PluginError> {
    Version::parse(value).map_err(|_| PluginError::InvalidRuntimeVersion {
        field,
        value: value.to_string(),
    })
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
