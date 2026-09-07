use mcp_server::config::{ENV_AUDIT_LOG_STDERR_MIRROR, HONORED_CONFIG_ENV_VARS};
use std::collections::BTreeSet;

const CONTRACT_START: &str = "<!-- config-env-contract:start -->";
const CONTRACT_END: &str = "<!-- config-env-contract:end -->";
const ENV_VAR_HEADER: &str = "Env var (wins)";

fn contract_section(body: &str) -> &str {
    let (_, after_start) = body
        .split_once(CONTRACT_START)
        .expect("README config env table must have a start marker");
    let (section, _) = after_start
        .split_once(CONTRACT_END)
        .expect("README config env table must have an end marker");
    section
}

/// Parses every documented env var name out of the README contract table.
///
/// Fails loudly (panics with the offending line) on any row that isn't the
/// blank/header/separator boilerplate but doesn't parse as `` `ENV_NAME` ``
/// in the env-var column — previously a `filter_map` with `?` silently
/// dropped any row it couldn't parse, so a typo'd row (e.g. a missing
/// backtick) would just vanish from the documented set instead of failing
/// the test, and README/runtime drift could go undetected (issue #283).
///
/// The env-var column is located by its header text rather than a
/// hardcoded index, so reordering the table's columns doesn't silently
/// start reading the wrong column (issue #283).
fn documented_env_vars(section: &str) -> Vec<String> {
    let mut env_var_column = None;
    let mut rows = Vec::new();

    for line in section.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let cells = trimmed
            .trim_matches('|')
            .split('|')
            .map(str::trim)
            .collect::<Vec<_>>();

        if env_var_column.is_none() {
            // The first non-blank line is the header row; locate the env-var
            // column by name instead of assuming a fixed position.
            env_var_column = Some(
                cells
                    .iter()
                    .position(|cell| *cell == ENV_VAR_HEADER)
                    .unwrap_or_else(|| {
                        panic!("README contract table header must contain a {ENV_VAR_HEADER:?} column: {trimmed:?}")
                    }),
            );
            continue;
        }
        if cells.iter().all(|cell| cell.chars().all(|c| c == '-')) {
            // The `| --- | --- | --- |` Markdown table separator row.
            continue;
        }

        let column = env_var_column.expect("header row already parsed above");
        let env_cell = cells
            .get(column)
            .unwrap_or_else(|| panic!("README contract row has no column {column}: {trimmed:?}"));
        let name = env_cell
            .split_once('`')
            .and_then(|(_, after_tick)| after_tick.split_once('`'))
            .map(|(name, _)| name.to_string())
            .unwrap_or_else(|| {
                panic!(
                    "README contract row's env-var cell isn't backtick-quoted: {env_cell:?} (full row: {trimmed:?})"
                )
            });
        rows.push(name);
    }

    rows
}

#[test]
fn readme_config_env_table_exactly_matches_runtime_contract() {
    let readme = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../README.md"),
    )
    .expect("README.md must be readable");
    let documented = documented_env_vars(contract_section(&readme));
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

    // Cross-reference the two contracts directly rather than relying on
    // `readme_config_env_table_exactly_matches_runtime_contract` to
    // transitively catch drift via `HONORED_CONFIG_ENV_VARS`: assert this
    // specific constant's value is the one actually documented in the
    // README, so a change to either side without the other fails here too
    // (issue #283).
    let readme = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../README.md"),
    )
    .expect("README.md must be readable");
    let documented = documented_env_vars(contract_section(&readme));
    assert!(
        documented.iter().any(|name| name == ENV_AUDIT_LOG_STDERR_MIRROR),
        "README contract table must document ENV_AUDIT_LOG_STDERR_MIRROR's value ({ENV_AUDIT_LOG_STDERR_MIRROR:?})"
    );
}
