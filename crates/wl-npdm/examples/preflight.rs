//! Standalone executable preflight inspection tool for Nintendo Switch NPDM files.
//!
//! Usage:
//! ```bash
//! cargo run --example preflight path/to/main.npdm
//! ```

use std::env;
use std::fs;
use std::process;
use wl_npdm::preflight_npdm;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <path-to-main.npdm>", args[0]);
        process::exit(1);
    }

    let path = &args[1];
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Failed to read file '{}': {}", path, e);
            process::exit(1);
        }
    };

    match preflight_npdm(&bytes) {
        Ok(preflight) => {
            println!("{}", preflight);
        }
        Err(e) => {
            eprintln!("NPDM structural validation failed: {}", e);
            process::exit(2);
        }
    }
}
