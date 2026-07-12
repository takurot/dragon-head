use std::collections::HashSet;

const FORBIDDEN_IGNORES: [&str; 2] = ["RUSTSEC-2026-0095", "RUSTSEC-2026-0096"];

#[test]
fn critical_wasmtime_advisories_cannot_be_ignored() {
    let deny_config = include_str!("../../deny.toml")
        .parse::<toml::Value>()
        .expect("deny.toml must be valid TOML");
    let ignored = deny_config["advisories"]["ignore"]
        .as_array()
        .expect("advisories.ignore must be an array")
        .iter()
        .filter_map(|entry| entry.get("id").and_then(toml::Value::as_str))
        .collect::<HashSet<_>>();

    for advisory in FORBIDDEN_IGNORES {
        assert!(
            !ignored.contains(advisory),
            "critical Wasmtime advisory {advisory} must not be ignored"
        );
    }
}
