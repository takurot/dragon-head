use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use plugin_host::{PluginPackage, SignatureBlock};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use skills_engine::{SkillDefinition, SkillStep, StepControl};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MarketplaceMetadata {
    pub pack_id: String,
    pub author: String,
    pub version: String,
    pub compatible_version: String,
    pub dependencies: Vec<String>,
    pub signature: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DomainPack {
    pub metadata: MarketplaceMetadata,
    pub plugin: Option<PluginPackage>,
    pub skills: Vec<SkillDefinition>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UsageEvent {
    pub pack_id: String,
    pub event_type: String,
    pub count: u64,
}

pub fn calculate_revenue_share(event: &UsageEvent) -> f64 {
    match event.event_type.as_str() {
        "state_generation" => (event.count as f64) * 0.005,
        "action_execution" => (event.count as f64) * 0.01,
        "skill_run" => (event.count as f64) * 0.02,
        _ => (event.count as f64) * 0.001,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MarketplaceError {
    #[error("domain pack missing signature")]
    UnsignedPack,
    #[error("invalid signature")]
    InvalidSignature,
    #[error("invalid ed25519 public key")]
    InvalidPublicKey,
    #[error("signature encoding error")]
    InvalidSignatureEncoding,
    #[error("unsupported domain pack signature version: {0}")]
    UnsupportedSignatureVersion(String),
    #[error("incompatible version: {0}")]
    IncompatibleVersion(String),
    #[error("failed to serialize domain pack signature payload: {0}")]
    SignaturePayloadSerialization(String),
}

const DOMAIN_PACK_SIGNATURE_DOMAIN: &str = "dragon-head.marketplace.domain-pack-signature";
const DOMAIN_PACK_SIGNATURE_VERSION: u32 = 1;
const DOMAIN_PACK_SIGNATURE_PREFIX: &str = "v1:";

#[derive(Serialize)]
struct SignedMarketplaceMetadata<'a> {
    pack_id: &'a str,
    author: &'a str,
    version: &'a str,
    compatible_version: &'a str,
    dependencies: &'a [String],
}

#[derive(Serialize)]
struct SignedPlugin<'a> {
    manifest: SignedPluginManifestV1<'a>,
    wasm_hash_algorithm: &'static str,
    wasm_sha256: String,
}

#[derive(Serialize)]
struct SignedPluginManifestV1<'a> {
    plugin_id: &'a str,
    version: &'a str,
    entry_points: Vec<&'static str>,
    capabilities: Vec<&'static str>,
    signature: Option<SignedPluginSignatureV1<'a>>,
    sbom: SignedSbomDocumentV1<'a>,
}

#[derive(Serialize)]
struct SignedPluginSignatureV1<'a> {
    key_id: &'a str,
    signature_hex: &'a str,
}

#[derive(Serialize)]
struct SignedSbomDocumentV1<'a> {
    format: &'a str,
    components: Vec<SignedSbomComponentV1<'a>>,
}

#[derive(Serialize)]
struct SignedSbomComponentV1<'a> {
    name: &'a str,
    version: &'a str,
    license: Option<&'a str>,
}

#[derive(Serialize)]
struct SignedSkillDefinitionV1<'a> {
    schema_version: u32,
    name: &'a str,
    steps: Vec<SignedSkillStepV1<'a>>,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SignedSkillStepV1<'a> {
    Locate {
        id: Option<&'a str>,
        query: &'a str,
        control: SignedStepControlV1<'a>,
    },
    Verify {
        id: Option<&'a str>,
        target: &'a str,
        expected: &'a str,
        control: SignedStepControlV1<'a>,
    },
    Act {
        id: Option<&'a str>,
        action: &'a str,
        target: &'a str,
        value: Option<&'a str>,
        control: SignedStepControlV1<'a>,
    },
    Wait {
        id: Option<&'a str>,
        condition: &'a str,
        timeout_ms: u64,
        control: SignedStepControlV1<'a>,
    },
    Extract {
        id: Option<&'a str>,
        key: &'a str,
        selector: &'a str,
        control: SignedStepControlV1<'a>,
    },
    Handoff {
        id: Option<&'a str>,
        reason: &'a str,
        assignee: Option<&'a str>,
        control: SignedStepControlV1<'a>,
    },
}

#[derive(Serialize)]
struct SignedStepControlV1<'a> {
    max_retries: u32,
    on_success: Option<&'a str>,
    on_failure: Option<&'a str>,
}

#[derive(Serialize)]
struct DomainPackSignaturePayload<'a> {
    domain: &'static str,
    version: u32,
    metadata: SignedMarketplaceMetadata<'a>,
    plugin: Option<SignedPlugin<'a>>,
    skills: Vec<SignedSkillDefinitionV1<'a>>,
}

fn signed_plugin_signature(signature: &SignatureBlock) -> SignedPluginSignatureV1<'_> {
    SignedPluginSignatureV1 {
        key_id: &signature.key_id,
        signature_hex: &signature.signature_hex,
    }
}

fn signed_extension_point_v1(extension_point: &plugin_host::ExtensionPoint) -> &'static str {
    match extension_point {
        plugin_host::ExtensionPoint::OnState => "on_state",
        plugin_host::ExtensionPoint::BeforeAct => "before_act",
        plugin_host::ExtensionPoint::Connector => "connector",
    }
}

fn signed_capability_v1(capability: &plugin_host::Capability) -> &'static str {
    match capability {
        plugin_host::Capability::ReadState => "read_state",
        plugin_host::Capability::NetworkOut => "network_out",
        plugin_host::Capability::VaultAccess => "vault_access",
    }
}

fn signed_step_control(control: &StepControl) -> SignedStepControlV1<'_> {
    SignedStepControlV1 {
        max_retries: control.max_retries,
        on_success: control.on_success.as_deref(),
        on_failure: control.on_failure.as_deref(),
    }
}

fn signed_skill_step(step: &SkillStep) -> SignedSkillStepV1<'_> {
    match step {
        SkillStep::Locate(step) => SignedSkillStepV1::Locate {
            id: step.id.as_deref(),
            query: &step.query,
            control: signed_step_control(&step.control),
        },
        SkillStep::Verify(step) => SignedSkillStepV1::Verify {
            id: step.id.as_deref(),
            target: &step.target,
            expected: &step.expected,
            control: signed_step_control(&step.control),
        },
        SkillStep::Act(step) => SignedSkillStepV1::Act {
            id: step.id.as_deref(),
            action: &step.action,
            target: &step.target,
            value: step.value.as_deref(),
            control: signed_step_control(&step.control),
        },
        SkillStep::Wait(step) => SignedSkillStepV1::Wait {
            id: step.id.as_deref(),
            condition: &step.condition,
            timeout_ms: step.timeout_ms,
            control: signed_step_control(&step.control),
        },
        SkillStep::Extract(step) => SignedSkillStepV1::Extract {
            id: step.id.as_deref(),
            key: &step.key,
            selector: &step.selector,
            control: signed_step_control(&step.control),
        },
        SkillStep::Handoff(step) => SignedSkillStepV1::Handoff {
            id: step.id.as_deref(),
            reason: &step.reason,
            assignee: step.assignee.as_deref(),
            control: signed_step_control(&step.control),
        },
    }
}

fn signed_skill(skill: &SkillDefinition) -> SignedSkillDefinitionV1<'_> {
    SignedSkillDefinitionV1 {
        schema_version: skill.schema_version,
        name: &skill.name,
        steps: skill.steps.iter().map(signed_skill_step).collect(),
    }
}

/// Returns the exact versioned bytes that authors must sign for a domain pack.
///
/// The fixed struct field order and serde JSON encoding are part of the v1
/// signature protocol. Collection order is significant. The marketplace
/// signature itself is intentionally excluded to avoid a recursive payload;
/// every other execution- or compatibility-relevant field is included.
pub fn domain_pack_signature_payload(pack: &DomainPack) -> Result<Vec<u8>, MarketplaceError> {
    let payload = DomainPackSignaturePayload {
        domain: DOMAIN_PACK_SIGNATURE_DOMAIN,
        version: DOMAIN_PACK_SIGNATURE_VERSION,
        metadata: SignedMarketplaceMetadata {
            pack_id: &pack.metadata.pack_id,
            author: &pack.metadata.author,
            version: &pack.metadata.version,
            compatible_version: &pack.metadata.compatible_version,
            dependencies: &pack.metadata.dependencies,
        },
        plugin: pack.plugin.as_ref().map(|plugin| {
            let manifest = &plugin.manifest;
            SignedPlugin {
                manifest: SignedPluginManifestV1 {
                    plugin_id: &manifest.plugin_id,
                    version: &manifest.version,
                    entry_points: manifest
                        .entry_points
                        .iter()
                        .map(signed_extension_point_v1)
                        .collect(),
                    capabilities: manifest
                        .capabilities
                        .iter()
                        .map(signed_capability_v1)
                        .collect(),
                    signature: manifest.signature.as_ref().map(signed_plugin_signature),
                    sbom: SignedSbomDocumentV1 {
                        format: &manifest.sbom.format,
                        components: manifest
                            .sbom
                            .components
                            .iter()
                            .map(|component| SignedSbomComponentV1 {
                                name: &component.name,
                                version: &component.version,
                                license: component.license.as_deref(),
                            })
                            .collect(),
                    },
                },
                wasm_hash_algorithm: "sha256",
                wasm_sha256: hex::encode(Sha256::digest(&plugin.wasm_module)),
            }
        }),
        skills: pack.skills.iter().map(signed_skill).collect(),
    };

    serde_json::to_vec(&payload)
        .map_err(|error| MarketplaceError::SignaturePayloadSerialization(error.to_string()))
}

/// Encodes an Ed25519 signature with the payload-version discriminator required
/// by [`verify_domain_pack`]. Legacy unversioned signatures are rejected.
pub fn encode_domain_pack_signature(signature: &Signature) -> String {
    format!(
        "{DOMAIN_PACK_SIGNATURE_PREFIX}{}",
        hex::encode(signature.to_bytes())
    )
}

pub fn verify_domain_pack(
    pack: &DomainPack,
    author_pubkey_hex: &str,
) -> Result<(), MarketplaceError> {
    let sig_hex = pack
        .metadata
        .signature
        .as_ref()
        .ok_or(MarketplaceError::UnsignedPack)?;

    let decoded_pubkey =
        hex::decode(author_pubkey_hex).map_err(|_| MarketplaceError::InvalidPublicKey)?;
    let pubkey_bytes: [u8; 32] = decoded_pubkey
        .try_into()
        .map_err(|_| MarketplaceError::InvalidPublicKey)?;
    let verifying_key =
        VerifyingKey::from_bytes(&pubkey_bytes).map_err(|_| MarketplaceError::InvalidPublicKey)?;

    let signature_hex = sig_hex
        .strip_prefix(DOMAIN_PACK_SIGNATURE_PREFIX)
        .ok_or_else(|| {
            MarketplaceError::UnsupportedSignatureVersion(
                sig_hex
                    .split_once(':')
                    .map_or("legacy", |(version, _)| version)
                    .to_string(),
            )
        })?;
    let signature_bytes =
        hex::decode(signature_hex).map_err(|_| MarketplaceError::InvalidSignatureEncoding)?;
    let parsed_signature = Signature::from_slice(&signature_bytes)
        .map_err(|_| MarketplaceError::InvalidSignatureEncoding)?;

    let payload = domain_pack_signature_payload(pack)?;

    verifying_key
        .verify(&payload, &parsed_signature)
        .map_err(|_| MarketplaceError::InvalidSignature)?;

    Ok(())
}
