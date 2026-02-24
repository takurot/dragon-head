use core_runtime::sre::StableKeyGenerator;

#[test]
fn test_stable_key_hash_compatibility_vectors() {
    // NOTE: These vectors lock current hash behavior to detect incompatible changes.
    let mut generator = StableKeyGenerator::new();

    let (first_key, first_ambiguous) = generator.generate_key(
        "button",
        Some(" Submit "),
        "root/#document/html/body/button",
        "Top_Left",
    );
    assert_eq!(
        first_key,
        "14b454ad4839240ded4161ce1cc11568b1b7b1531d970a6b8905f9aad648dbe9"
    );
    assert!(!first_ambiguous);

    let (second_key, second_ambiguous) = generator.generate_key(
        "button",
        Some(" Submit "),
        "root/#document/html/body/button",
        "Top_Left",
    );
    assert_eq!(
        second_key,
        "0077d9753c502853a96acdfdb031e6f48e6f30fc9681273b3224f075e749e032"
    );
    assert!(second_ambiguous);

    let (third_key, third_ambiguous) = generator.generate_key(
        "button",
        Some(" Submit "),
        "root/#document/html/body/button",
        "Top_Left",
    );
    assert_eq!(
        third_key,
        "b655a548c46f03ca01c9d10cf0eda7f774cb278a6a3ff499735a7d459750ca01"
    );
    assert!(third_ambiguous);
}

#[test]
fn test_stable_key_hash_compatibility_none_label_vector() {
    let mut generator = StableKeyGenerator::new();
    let (key, ambiguous) = generator.generate_key(
        "input",
        None,
        "root/#document/html/body/form/input",
        "Bottom_Right",
    );
    assert_eq!(
        key,
        "2a91f3b04cc2ea295b036f89c810fde7314880b0f60810d9a1db55b9fab4561e"
    );
    assert!(!ambiguous);
}

#[test]
fn test_stable_key_hash_compatibility_non_ascii_label_vector() {
    // Guard Unicode lowercasing/trim path from accidental compatibility regressions.
    let mut generator = StableKeyGenerator::new();
    let (key, ambiguous) = generator.generate_key(
        "button",
        Some("  İSTANBUL Straße  "),
        "root/#document/html/body/button[1]",
        "Top_Right",
    );
    assert_eq!(
        key,
        "c64f7d577b24a91e8879f60c3a2e4b88e1b3c1fbbdac8dbc09ee6757af252286"
    );
    assert!(!ambiguous);
}
