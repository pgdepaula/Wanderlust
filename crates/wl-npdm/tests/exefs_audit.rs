//! Integration tests for strict ExeFS readiness audit gate.

use std::fs;
use std::path::PathBuf;
use wl_npdm::{AuditError, audit_exefs};

fn probe_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("tests/horizon/probe")
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "wl_test_exefs_{}_{}_{}",
            name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).expect("create test tempdir");
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[test]
fn test_audit_retail_probe_exefs() {
    let dir = probe_dir();
    assert!(dir.is_dir(), "tests/horizon/probe directory must exist");

    let report = audit_exefs(&dir).expect("audit_exefs must succeed on tests/horizon/probe");

    // 1. Verify module discovery and identification
    assert_eq!(report.present_modules, vec!["main".to_string()]);
    assert!(report.missing_required_inputs.is_empty());
    assert_eq!(report.modules.len(), 1);

    let main_mod = &report.modules[0];
    assert_eq!(main_mod.name, "main");
    assert!(main_mod.compressed);
    assert!(main_mod.has_mod0);
    assert!(main_mod.mod0_offset.is_some());
    assert!(main_mod.dynamic_symbols_count > 0);
    assert!(main_mod.text_size > 0);
    assert!(main_mod.rodata_size > 0);
    assert!(main_mod.data_size > 0);
    assert!(!main_mod.sha256.is_empty());
    assert!(!main_mod.build_id.is_empty());

    // 2. Verify NPDM process metadata extraction
    assert!(report.npdm_present);
    assert_eq!(report.program_name.as_deref(), Some("WanderlustProbe"));
    assert_eq!(report.product_code.as_deref(), Some(""));
    assert!(report.unsupported_metadata.is_empty());

    let meta = report
        .program_metadata
        .as_ref()
        .expect("ProcessMetadata must be present");
    assert_eq!(meta.program_id, 0x0100_0000_0000_2000);
    assert_eq!(meta.main_thread_priority, 44);
    assert_eq!(meta.ideal_core, 0);

    // 3. Verify Loader linking and relocation invariants
    assert_eq!(report.link_mode, "Guest");
    assert!(report.execution_blocker.is_none());
    assert!(report.candidate_code_pointers_count > 0);

    // 4. Verify AOT compilation readiness and execution boundary
    assert!(
        report.aot_analysis_ready,
        "Authentic probe ExeFS must be marked AOT ready"
    );
    assert!(
        !report.runtime_execution_occurred,
        "Readiness audit must never execute guest code"
    );
}

#[test]
fn test_audit_nonexistent_directory() {
    let bad_dir = PathBuf::from("nonexistent_path_that_does_not_exist_42");
    match audit_exefs(&bad_dir) {
        Err(AuditError::NotADirectory(p)) => {
            assert_eq!(p, bad_dir);
        }
        other => panic!("Expected NotADirectory, got {other:?}"),
    }
}

#[test]
fn test_audit_missing_inputs_blocks_aot_readiness() {
    let tmp = TempDir::new("empty");
    let dir = &tmp.path;

    // In an empty directory, both 'main' and 'main.npdm' are missing
    let report = audit_exefs(dir).expect("audit succeeds with missing inputs");
    assert!(!report.aot_analysis_ready);
    assert!(report.missing_required_inputs.contains(&"main".to_string()));
    assert!(
        report
            .missing_required_inputs
            .contains(&"main.npdm".to_string())
    );
    assert!(!report.runtime_execution_occurred);

    // Copy only main.npdm into the tempdir
    let probe = probe_dir();
    fs::copy(probe.join("main.npdm"), dir.join("main.npdm")).expect("copy npdm");

    let report_with_npdm = audit_exefs(dir).expect("audit with only npdm");
    assert!(!report_with_npdm.aot_analysis_ready);
    assert_eq!(
        report_with_npdm.missing_required_inputs,
        vec!["main".to_string()]
    );
    assert!(report_with_npdm.npdm_present);
    assert_eq!(
        report_with_npdm.program_name.as_deref(),
        Some("WanderlustProbe")
    );

    // Copy only main into another tempdir
    let tmp2 = TempDir::new("only_main");
    let dir2 = &tmp2.path;
    fs::copy(probe.join("main"), dir2.join("main")).expect("copy main");

    let report_with_main = audit_exefs(dir2).expect("audit with only main");
    assert!(!report_with_main.aot_analysis_ready);
    assert_eq!(
        report_with_main.missing_required_inputs,
        vec!["main.npdm".to_string()]
    );
    assert_eq!(report_with_main.present_modules, vec!["main".to_string()]);
    assert!(!report_with_main.npdm_present);
}
