use mcp_server::config::{ENV_AUDIT_LOG_STDERR_MIRROR, HONORED_CONFIG_ENV_VARS};
use std::collections::BTreeSet;

const CONTRACT_START: &str = "<!-- config-env-contract:start -->";
const CONTRACT_END: &str = "<!-- config-env-contract:end -->";

fn contract_section(body: &str) -> &str {
    let (_, after_start) = body
        .split_once(CONTRACT_START)
        .expect("README config env table must have a start marker");
    let (section, _) = after_start
        .split_once(CONTRACT_END)
        .expect("README config env table must have an end marker");
    section
}

#[test]
fn readme_config_env_table_exactly_matches_runtime_contract() {
    let readme = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../README.md"),
    )
    .expect("README.md must be readable");
    let documented = contract_section(&readme)
        .lines()
        .filter_map(|line| {
            let cells = line.split('|').map(str::trim).collect::<Vec<_>>();
            let env_cell = cells.get(2)?;
            let (_, after_tick) = env_cell.split_once('`')?;
            let (name, _) = after_tick.split_once('`')?;
            Some(name.to_string())
        })
        .collect::<Vec<_>>();
    let unique = documented.iter().cloned().collect::<BTreeSet<_>>();
    let runtime = HONORED_CONFIG_ENV_VARS
        .iter()
        .copied()
        .map(str::to_string)
        .collect::<BTreeSet<_>>();

    assert_eq!(documented.len(), unique.len(), "duplicate README env row");
    assert_eq!(unique, runtime);
}

#[test]
fn audit_stderr_mirror_registry_name_is_consumed_by_production_lookup() {
    use std::cell::RefCell;

    let queried = RefCell::new(Vec::new());
    let _logger = core_runtime::audit::AuditLogger::from_env_with(|key| {
        queried.borrow_mut().push(key.to_string());
        None
    });

    assert!(
        queried
            .borrow()
            .iter()
            .any(|key| key == ENV_AUDIT_LOG_STDERR_MIRROR),
        "AuditLogger::from_env_with must consume the registered stderr mirror env name"
    );
}
