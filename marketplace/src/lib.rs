use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use plugin_host::PluginPackage;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use skills_engine::SkillDefinition;

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
    #[error("incompatible version: {0}")]
    IncompatibleVersion(String),
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

    // Hash payload for signature: pack_id, author, version
    let mut hasher = Sha256::new();
    hasher.update(pack.metadata.pack_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(pack.metadata.author.as_bytes());
    hasher.update(b"\0");
    hasher.update(pack.metadata.version.as_bytes());
    let payload = hasher.finalize();

    verifying_key
        .verify(&payload, &parsed_signature)
        .map_err(|_| MarketplaceError::InvalidSignature)?;

    Ok(())
}
