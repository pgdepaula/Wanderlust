//! Standalone ExeFS readiness audit inspection tool for Nintendo Switch executable packages.
//!
//! Usage:
//! ```bash
//! cargo run --example exefs_readiness <path-to-exefs-dir> [--json]
//! ```

use std::env;
use std::path::Path;
use std::process;
use wl_npdm::audit_exefs;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <path-to-exefs-dir> [--json]", args[0]);
        process::exit(1);
    }

    let dir_path = Path::new(&args[1]);
    let json_output = args.iter().any(|arg| arg == "--json");

    match audit_exefs(dir_path) {
        Ok(report) => {
            if json_output {
                match serde_json::to_string_pretty(&report) {
                    Ok(json) => println!("{json}"),
                    Err(e) => {
                        eprintln!("Failed to serialize report to JSON: {e}");
                        process::exit(2);
                    }
                }
            } else {
                println!("=== ExeFS Readiness Audit Report ===");
                println!("Directory:                  {}", report.directory);
                println!(
                    "Present Modules:            {}",
                    if report.present_modules.is_empty() {
                        "none".to_string()
                    } else {
                        report.present_modules.join(", ")
                    }
                );
                if !report.missing_required_inputs.is_empty() {
                    println!(
                        "Missing Required Inputs:    {}",
                        report.missing_required_inputs.join(", ")
                    );
                }
                println!(
                    "NPDM Present:               {}",
                    if report.npdm_present { "yes" } else { "no" }
                );
                if let Some(name) = &report.program_name {
                    println!("Program Name:               {name}");
                }
                if let Some(code) = &report.product_code {
                    println!("Product Code:               {code}");
                }
                if let Some(meta) = &report.program_metadata {
                    println!("Program ID:                 {:#018x}", meta.program_id);
                    println!("Main Thread Priority:       {}", meta.main_thread_priority);
                    println!("Ideal CPU Core:             {}", meta.ideal_core);
                    println!(
                        "Stack Size:                 {:#x} bytes",
                        meta.main_thread_stack_size
                    );
                }
                println!("Link Mode:                  {}", report.link_mode);
                println!(
                    "Candidate Code Pointers:    {}",
                    report.candidate_code_pointers_count
                );
                if let Some(blocker) = &report.execution_blocker {
                    println!("Execution Blocker:          {blocker}");
                }
                if !report.missing_runtime_services.is_empty() {
                    println!(
                        "Missing Runtime Services:   {}",
                        report.missing_runtime_services.join(", ")
                    );
                }
                println!(
                    "AOT Analysis Ready:         {}",
                    if report.aot_analysis_ready {
                        "READY"
                    } else {
                        "BLOCKED"
                    }
                );
                println!(
                    "Runtime Execution Occurred: {}",
                    if report.runtime_execution_occurred {
                        "YES"
                    } else {
                        "NO (static audit only)"
                    }
                );
                println!();
                println!("Modules:");
                for m in &report.modules {
                    println!(
                        "  - {:<10} (size: {:>7} B, mod0: {}, symbols: {:>3}, relocs: {:>4}, sha256: {}...)",
                        m.name,
                        m.file_size,
                        if m.has_mod0 { "yes" } else { "no " },
                        m.dynamic_symbols_count,
                        m.dynamic_relocations_count,
                        &m.sha256[..12]
                    );
                }
            }

            if !report.aot_analysis_ready {
                process::exit(3);
            }
        }
        Err(e) => {
            eprintln!("ExeFS audit failed: {e}");
            process::exit(2);
        }
    }
}
