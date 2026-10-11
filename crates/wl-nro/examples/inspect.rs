use std::env;
use std::fs;
use std::process;
use wl_nro::Nro;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <path-to-file.nro> [--json]", args[0]);
        process::exit(1);
    }

    let path = &args[1];
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Error reading file '{path}': {e}");
            process::exit(1);
        }
    };

    let nro = match Nro::parse(&bytes) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("Failed to parse NRO '{path}': {e}");
            process::exit(1);
        }
    };

    let report = nro.inspect();

    if args.iter().any(|arg| arg == "--json") {
        match serde_json::to_string_pretty(&report) {
            Ok(json) => println!("{json}"),
            Err(e) => {
                eprintln!("Failed to serialize report to JSON: {e}");
                process::exit(1);
            }
        }
    } else {
        println!("=== NRO Executable Preflight Inspection ===");
        println!("File: {path}");
        print!("{report}");
    }
}
