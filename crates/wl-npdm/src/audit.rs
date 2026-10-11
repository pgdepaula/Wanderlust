//! Strict, read-only ExeFS readiness audit for decrypted retail Switch executable inputs.
//!
//! Reuses existing `wl-nso`, `wl-loader`, and `wl-npdm` parsers to evaluate whether
//! decrypted game modules and process metadata are structurally valid, relocatable,
//! and ready for AOT binary recompilation.
use crate::error::NpdmError;
use crate::{Npdm, ProcessMetadata};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use wl_common::GuestAddr;
use wl_loader::{LinkMode, Process};
use wl_nso::Module;

pub const STANDARD_EXEFS_MODULES: [&str; 13] = [
    "rtld", "main", "subsdk0", "subsdk1", "subsdk2", "subsdk3", "subsdk4", "subsdk5", "subsdk6",
    "subsdk7", "subsdk8", "subsdk9", "sdk",
];

/// Known Horizon runtime services currently modeled by Wanderlust.
pub const SUPPORTED_RUNTIME_SERVICES: &[&str] = &[];

/// Error encountered during an ExeFS audit.
#[derive(Debug)]
pub enum AuditError {
    Io(std::io::Error),
    Npdm(NpdmError),
    Nso(String),
    NotADirectory(PathBuf),
}

impl std::fmt::Display for AuditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::Npdm(e) => write!(f, "NPDM error: {e}"),
            Self::Nso(e) => write!(f, "NSO error: {e}"),
            Self::NotADirectory(p) => write!(f, "Path is not a directory: {}", p.display()),
        }
    }
}

impl std::error::Error for AuditError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Npdm(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for AuditError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<NpdmError> for AuditError {
    fn from(e: NpdmError) -> Self {
        Self::Npdm(e)
    }
}

/// Structural inspection of an individual NSO executable module.
#[derive(Clone, Debug, Serialize)]
pub struct ModuleAudit {
    pub name: String,
    pub path: String,
    pub file_size: u64,
    pub sha256: String,
    pub build_id: String,
    pub compressed: bool,
    pub text_size: u32,
    pub rodata_size: u32,
    pub data_size: u32,
    pub bss_size: u32,
    pub has_mod0: bool,
    pub mod0_offset: Option<u64>,
    pub dynamic_symbols_count: usize,
    pub dynamic_relocations_count: usize,
}

/// Comprehensive readiness assessment of an ExeFS executable package.
#[derive(Clone, Debug, Serialize)]
pub struct ExeFsReadinessReport {
    pub directory: String,
    pub present_modules: Vec<String>,
    pub missing_required_inputs: Vec<String>,
    pub modules: Vec<ModuleAudit>,
    pub npdm_present: bool,
    pub program_metadata: Option<ProcessMetadata>,
    pub program_name: Option<String>,
    pub product_code: Option<String>,
    pub unsupported_metadata: Vec<String>,
    pub link_mode: String,
    pub unresolved_dynamic_symbols: Vec<String>,
    pub init_order: Vec<String>,
    pub execution_blocker: Option<String>,
    pub candidate_code_pointers_count: usize,
    pub aot_analysis_ready: bool,
    pub runtime_execution_occurred: bool,
    pub missing_runtime_services: Vec<String>,
}

fn compute_sha256(path: &Path) -> Result<String, std::io::Error> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// Performs a strict, read-only ExeFS readiness audit on the specified directory.
pub fn audit_exefs(dir: &Path) -> Result<ExeFsReadinessReport, AuditError> {
    if !dir.is_dir() {
        return Err(AuditError::NotADirectory(dir.to_path_buf()));
    }

    let mut present_modules = Vec::new();
    let mut missing_required_inputs = Vec::new();
    let mut module_audits = Vec::new();
    let mut loader_inputs = Vec::new();
    let mut execution_blocker = None;

    // 1. Discover standard and present NSO modules
    for &mod_name in &STANDARD_EXEFS_MODULES {
        let candidate_plain = dir.join(mod_name);
        let candidate_nso = dir.join(format!("{mod_name}.nso"));
        let target_path = if candidate_plain.is_file() {
            Some(candidate_plain)
        } else if candidate_nso.is_file() {
            Some(candidate_nso)
        } else {
            None
        };

        if let Some(path) = target_path {
            let metadata = fs::metadata(&path)?;
            let file_size = metadata.len();
            let sha256 = compute_sha256(&path)?;

            let bytes = fs::read(&path)?;
            match Module::parse(&bytes) {
                Ok(nso) => {
                    let build_id = nso.module_id.clone();
                    let compressed = nso.segments.iter().any(|s| s.compressed);
                    let text_size = nso.segments.first().map(|s| s.memory_size).unwrap_or(0);
                    let rodata_size = nso.segments.get(1).map(|s| s.memory_size).unwrap_or(0);
                    let data_size = nso.segments.get(2).map(|s| s.memory_size).unwrap_or(0);
                    let bss_size = nso.bss_size;

                    let has_mod0 = nso.mod0.is_some();
                    let mod0_offset = nso.mod0.as_ref().map(|m| m.offset.0);

                    let dynamic_symbols_count = nso
                        .mod0
                        .as_ref()
                        .map(|m| m.dynamic.symbols.len())
                        .unwrap_or(0);
                    let dynamic_relocations_count = nso
                        .mod0
                        .as_ref()
                        .map(|m| m.dynamic.relocations.len())
                        .unwrap_or(0);

                    module_audits.push(ModuleAudit {
                        name: mod_name.to_string(),
                        path: path.display().to_string(),
                        file_size,
                        sha256,
                        build_id,
                        compressed,
                        text_size,
                        rodata_size,
                        data_size,
                        bss_size,
                        has_mod0,
                        mod0_offset,
                        dynamic_symbols_count,
                        dynamic_relocations_count,
                    });

                    present_modules.push(mod_name.to_string());
                    loader_inputs.push((mod_name.to_string(), nso));
                }
                Err(e) => {
                    execution_blocker = Some(format!("Module {mod_name} parse failed: {e}"));
                }
            }
        }
    }

    // Main executable is required for retail Switch execution
    if !present_modules.iter().any(|m| m == "main") {
        missing_required_inputs.push("main".to_string());
    }

    // 2. Audit NPDM metadata
    let npdm_path = dir.join("main.npdm");
    let mut npdm_present = false;
    let mut program_metadata = None;
    let mut program_name = None;
    let mut product_code = None;
    let mut unsupported_metadata = Vec::new();
    let mut required_services = BTreeSet::new();

    if npdm_path.is_file() {
        npdm_present = true;
        match fs::read(&npdm_path) {
            Ok(npdm_bytes) => match Npdm::parse(&npdm_bytes) {
                Ok(npdm) => {
                    let meta = npdm.meta();
                    program_name = Some(meta.program_name.to_string());
                    product_code = Some(meta.product_code.to_string());

                    if !meta.is_64_bit_instruction {
                        unsupported_metadata
                            .push("32-bit ARM instruction mode is unsupported".to_string());
                    }

                    match npdm.to_process_metadata() {
                        Ok(md) => {
                            for sac in npdm.aci0_services().flatten() {
                                if !sac.is_server {
                                    required_services.insert(sac.name.to_string());
                                }
                            }
                            program_metadata = Some(md);
                        }
                        Err(e) => {
                            unsupported_metadata
                                .push(format!("Process metadata extraction error: {e}"));
                        }
                    }
                }
                Err(e) => {
                    unsupported_metadata.push(format!("NPDM parse error: {e}"));
                }
            },
            Err(e) => {
                unsupported_metadata.push(format!("Failed to read {}: {e}", npdm_path.display()));
            }
        }
    } else {
        missing_required_inputs.push("main.npdm".to_string());
    }

    // Identify missing runtime services
    let mut missing_runtime_services = Vec::new();
    for srv in required_services {
        if !srv.ends_with('*') && !SUPPORTED_RUNTIME_SERVICES.contains(&srv.as_str()) {
            missing_runtime_services.push(srv);
        }
    }

    // 3. Process Linking and Relocation Invariants
    let mut unresolved_dynamic_symbols = Vec::new();
    let mut init_order = Vec::new();
    let mut candidate_code_pointers_count = 0;

    if execution_blocker.is_none() && !loader_inputs.is_empty() {
        let guest_base = GuestAddr(0x800_0000);
        match Process::build(&loader_inputs, guest_base, LinkMode::Guest) {
            Ok(proc) => {
                if let Some(blocker) = proc.execution_blocker {
                    execution_blocker = Some(blocker);
                }
                candidate_code_pointers_count = proc.code_pointers.len();

                for diag in &proc.link.diagnostics {
                    if diag.kind == "unresolved-symbol" {
                        unresolved_dynamic_symbols
                            .push(format!("{}: {}", diag.module, diag.detail));
                    }
                }

                for init in &proc.link.init_order {
                    init_order.push(format!("{} @ {:#x}", init.module, init.address.0));
                }
            }
            Err(e) => {
                execution_blocker = Some(format!("Process load build error: {e}"));
            }
        }
    }

    let aot_analysis_ready = missing_required_inputs.is_empty()
        && unsupported_metadata.is_empty()
        && !present_modules.is_empty()
        && execution_blocker.is_none();

    let runtime_execution_occurred = false;

    Ok(ExeFsReadinessReport {
        directory: dir.display().to_string(),
        present_modules,
        missing_required_inputs,
        modules: module_audits,
        npdm_present,
        program_metadata,
        program_name,
        product_code,
        unsupported_metadata,
        link_mode: "Guest".to_string(),
        unresolved_dynamic_symbols,
        init_order,
        execution_blocker,
        candidate_code_pointers_count,
        aot_analysis_ready,
        runtime_execution_occurred,
        missing_runtime_services,
    })
}
