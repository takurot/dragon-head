use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use plugin_host::PluginPackage;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use skills_engine::SkillDefinition;

/// Current runtime compatibility version used for compatibility checks.
pub const RUNTIME_COMPATIBLE_VERSION: &str = "2.1";

// Revenue share rates per event type
const RATE_STATE_GENERATION: f64 = 0.005;
const RATE_ACTION_EXECUTION: f64 = 0.01;
const RATE_SKILL_RUN: f64 = 0.02;
const RATE_DEFAULT: f64 = 0.001;

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
    let rate = match event.event_type.as_str() {
        "state_generation" => RATE_STATE_GENERATION,
        "action_execution" => RATE_ACTION_EXECUTION,
        "skill_run" => RATE_SKILL_RUN,
        _ => RATE_DEFAULT,
    };
    (event.count as f64) * rate
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum MarketplaceError {
    #[error("domain pack missing signature")]
    UnsignedPack,
    #[error("invalid signature")]
    InvalidSignature,
    #[error("invalid ed25519 public key")]
    InvalidPublicKey,
    #[error("signature encoding error")]
    InvalidSignatureEncoding,
    #[error("incompatible version: pack requires {pack}, runtime is {runtime}")]
    IncompatibleVersion { pack: String, runtime: String },
}

/// Check that the pack's `compatible_version` matches the current runtime version.
pub fn check_compatibility(pack: &DomainPack) -> Result<(), MarketplaceError> {
    if pack.metadata.compatible_version != RUNTIME_COMPATIBLE_VERSION {
        return Err(MarketplaceError::IncompatibleVersion {
            pack: pack.metadata.compatible_version.clone(),
            runtime: RUNTIME_COMPATIBLE_VERSION.to_string(),
        });
    }
    Ok(())
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

    let signature_bytes =
        hex::decode(sig_hex).map_err(|_| MarketplaceError::InvalidSignatureEncoding)?;
    let parsed_signature = Signature::from_slice(&signature_bytes)
        .map_err(|_| MarketplaceError::InvalidSignatureEncoding)?;

    let payload = build_signature_payload(&pack.metadata);

    verifying_key
        .verify(&payload, &parsed_signature)
        .map_err(|_| MarketplaceError::InvalidSignature)?;

    Ok(())
}

/// Build the canonical hash payload for signature verification.
/// Includes all metadata fields to prevent tampering with any field.
pub fn build_signature_payload(metadata: &MarketplaceMetadata) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(metadata.pack_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(metadata.author.as_bytes());
    hasher.update(b"\0");
    hasher.update(metadata.version.as_bytes());
    hasher.update(b"\0");
    hasher.update(metadata.compatible_version.as_bytes());
    hasher.update(b"\0");
    for dep in &metadata.dependencies {
        hasher.update(dep.as_bytes());
        hasher.update(b"\0");
    }
    hasher.finalize().to_vec()
}
