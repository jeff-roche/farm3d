//! P4: the Library commands' registration (ruling M6). `P4_COMMANDS` is the
//! single place each P4 task adds its command names; the count is asserted
//! against the pre-P4 total rather than a hard-coded sum.

/// `COMMAND_NAMES.len()` before P4 (P3 merged).
const PRE_P4: usize = 41;

const P4_COMMANDS: &[&str] = &[
    "pick_model_files",
    "inspect_import_selection",
    "cancel_import_selection",
    "import_models",
];

#[test]
fn every_p4_command_is_registered_with_a_contract() {
    assert_eq!(farm3d_lib::COMMAND_NAMES.len(), PRE_P4 + P4_COMMANDS.len());
    let manifest = farm3d_lib::contracts::inventory::command_contract_inventory();
    assert_eq!(manifest.len(), farm3d_lib::COMMAND_NAMES.len());
    for command in P4_COMMANDS {
        assert!(
            farm3d_lib::COMMAND_NAMES.contains(command),
            "{command} missing from COMMAND_NAMES"
        );
        assert!(
            manifest.iter().any(|entry| entry.command == *command),
            "{command} missing from COMMAND_CONTRACTS"
        );
    }
}
