use std::{
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process,
    time::Instant,
};
use wl_analysis::{Image, Program, RAW_BASE, Roots, discover_roots, survey};
use wl_common::{GuestAddr, Result};
use wl_loader::{DEFAULT_CODE_BASE, LinkMode, MODULE_ORDER, Process};
use wl_nso::{Module, read_input};

const HELP: &str = "Wanderlust — experimental AArch64 ahead-of-time recompiler

USAGE:
  wanderlust intake PATH [--verify-content]
  wanderlust inspect INPUT [--json]
  wanderlust link EXEFS [--link host|guest] [--base ADDRESS]
  wanderlust survey INPUT [--raw] [--base ADDRESS] [--examples N]
  wanderlust analyze INPUT [--raw] [--base ADDRESS] [--entry ADDRESS] [--seed ADDRESS]...
                     [--link host|guest] [--no-pointer-seeds] [--allow-unsupported]
                     [--roots-file FILE] [--summary]
  wanderlust recompile INPUT [analyze options] --out EXECUTABLE [--target TRIPLE] [--emit-only]
                       [--jobs N] [--cache-dir DIR] [--unit-shift BITS]
                       [--horizon [--memory-limit BYTES] [--native-gpu]]

intake identifies NSP/PFS0 packages, NCA headers, tickets, CNMT XML, extracted ExeFS
directories and NSO files without modifying them; --verify-content streams SHA-256 over
every packaged NCA and compares it with its content ID.

INPUT is an extracted, decrypted NSO0, raw little-endian A64 words (--raw), or an ExeFS
directory holding NSO modules named rtld, main, subsdk0-9, sdk (optional .nso suffix).
A directory is loaded as a process: modules are placed in loader order from 0x8000000
(--base overrides) and linked. --link host applies relocations in Wanderlust (default
without rtld); --link guest maps modules unrelocated for rtld (default with rtld).
The process entry is the start of the first module's text; --entry overrides it.
Relocated code pointers and dynamic function symbols become speculative analysis roots
unless --no-pointer-seeds is given. --allow-unsupported compiles unsupported reachable
instructions as precise runtime faults instead of rejecting the program. --roots-file
adds speculative roots (one address per line), e.g. from symbols, metadata or traces.

For single files the default load base is 0x10000 and --entry is required.
The executable crate is written beside EXECUTABLE as EXECUTABLE.rs with memory images as
EXECUTABLE.rs.<address>.bin. Translated blocks form one crate per 2^BITS-byte guest window
(default 14); the runtime and units are compiled as rlibs by N parallel rustc jobs (default
WL_JOBS or min(CPUs, 4)) and cached in DIR (default WL_CACHE_DIR or .wanderlust-cache beside
EXECUTABLE), keyed by source, toolchain, target and flags. --target uses rustc's installed
target and linker.

--horizon (ExeFS directory or NRO input) builds a Horizon process launcher instead of the
register-level test harness: the main thread starts at the first module with X0 = 0, X1 = its
handle (NRO: X0 = loader configuration, X1 = -1, X30 = loader return), an NPDM-sized stack
and TLS; Horizon SVCs, guest threads and the sm: service are served by the runtime. Process
parameters come from main.npdm when present; --memory-limit sets the process memory size
(default 512 MiB; NPDM applications default to 3296 MiB). An explicit limit is
never replaced by the NPDM application default. The executable accepts --steps and --quantum.
--native-gpu installs the bounded native NVIDIA backend and builds its dependencies with
Cargo for the selected target. It requires this source workspace and an available host GPU;
it does not add unsupported Maxwell instructions, services or game compatibility.";

fn number(v: &str) -> Result<u64> {
    if let Some(v) = v.strip_prefix("0x") {
        u64::from_str_radix(v, 16)
    } else {
        v.parse()
    }
    .map_err(|e| format!("invalid address {v:?}: {e}"))
}
struct Options {
    command: String,
    input: PathBuf,
    raw: bool,
    json: bool,
    base: Option<GuestAddr>,
    entry: Option<GuestAddr>,
    seeds: Vec<GuestAddr>,
    out: Option<PathBuf>,
    target: Option<String>,
    emit_only: bool,
    link: Option<LinkMode>,
    pointer_seeds: bool,
    allow_unsupported: bool,
    summary: bool,
    verify_content: bool,
    roots_file: Option<PathBuf>,
    examples: usize,
    jobs: Option<usize>,
    cache_dir: Option<PathBuf>,
    unit_shift: u32,
    horizon: bool,
    memory_limit: Option<u64>,
    native_gpu: bool,
}
fn utf8(value: &OsString) -> Result<&str> {
    value
        .to_str()
        .ok_or_else(|| "option/address must be valid UTF-8".into())
}
const COMMANDS: [&str; 6] = [
    "intake",
    "inspect",
    "link",
    "survey",
    "analyze",
    "recompile",
];
fn options(args: &[OsString]) -> Result<Options> {
    let command = utf8(args.first().ok_or("missing command")?)?.to_owned();
    if !COMMANDS.contains(&command.as_str()) {
        return Err(format!("unknown command {command:?}; use --help"));
    }
    let input = args.get(1).ok_or("missing input path")?.into();
    let mut o = Options {
        command,
        input,
        raw: false,
        json: false,
        base: None,
        entry: None,
        seeds: Vec::new(),
        out: None,
        target: None,
        emit_only: false,
        link: None,
        pointer_seeds: true,
        allow_unsupported: false,
        summary: false,
        verify_content: false,
        roots_file: None,
        examples: 32,
        jobs: None,
        cache_dir: None,
        unit_shift: wl_codegen_rust::EmitOptions::default().unit_shift,
        horizon: false,
        memory_limit: None,
        native_gpu: false,
    };
    let mut used = Vec::new();
    let mut i = 2;
    while i < args.len() {
        let key = utf8(&args[i])?;
        used.push(key.to_owned());
        match key {
            "--raw" => o.raw = true,
            "--json" => o.json = true,
            "--emit-only" => o.emit_only = true,
            "--no-pointer-seeds" => o.pointer_seeds = false,
            "--allow-unsupported" => o.allow_unsupported = true,
            "--summary" => o.summary = true,
            "--verify-content" => o.verify_content = true,
            "--horizon" => o.horizon = true,
            "--native-gpu" => o.native_gpu = true,
            "--base" | "--entry" | "--seed" | "--out" | "--target" | "--link" | "--roots-file"
            | "--examples" | "--jobs" | "--cache-dir" | "--unit-shift" | "--memory-limit" => {
                i += 1;
                let value = args
                    .get(i)
                    .ok_or_else(|| format!("missing value for {key}"))?;
                match key {
                    "--base" => o.base = Some(GuestAddr(number(utf8(value)?)?)),
                    "--entry" => o.entry = Some(GuestAddr(number(utf8(value)?)?)),
                    "--seed" => o.seeds.push(GuestAddr(number(utf8(value)?)?)),
                    "--out" => o.out = Some(value.into()),
                    "--roots-file" => o.roots_file = Some(value.into()),
                    "--examples" => o.examples = number(utf8(value)?)? as usize,
                    "--jobs" => o.jobs = Some(number(utf8(value)?)? as usize),
                    "--cache-dir" => o.cache_dir = Some(value.into()),
                    "--unit-shift" => o.unit_shift = number(utf8(value)?)? as u32,
                    "--memory-limit" => o.memory_limit = Some(number(utf8(value)?)?),
                    "--link" => {
                        o.link = Some(match utf8(value)? {
                            "host" => LinkMode::Host,
                            "guest" => LinkMode::Guest,
                            other => {
                                return Err(format!("--link must be host or guest, not {other:?}"));
                            }
                        })
                    }
                    _ => o.target = Some(utf8(value)?.into()),
                }
            }
            _ => return Err(format!("unknown option {key:?}")),
        }
        i += 1;
    }
    let allowed: &[&str] = match o.command.as_str() {
        "intake" => &["--verify-content"],
        "inspect" => &["--json"],
        "link" => &["--link", "--base"],
        "survey" => &["--raw", "--base", "--link", "--examples"],
        "analyze" => &[
            "--raw",
            "--base",
            "--entry",
            "--seed",
            "--link",
            "--no-pointer-seeds",
            "--allow-unsupported",
            "--roots-file",
            "--summary",
        ],
        _ => &[
            "--raw",
            "--base",
            "--entry",
            "--seed",
            "--link",
            "--no-pointer-seeds",
            "--allow-unsupported",
            "--roots-file",
            "--out",
            "--target",
            "--emit-only",
            "--jobs",
            "--cache-dir",
            "--unit-shift",
            "--horizon",
            "--memory-limit",
            "--native-gpu",
        ],
    };
    if let Some(bad) = used.iter().find(|k| !allowed.contains(&k.as_str())) {
        return Err(match (o.command.as_str(), bad.as_str()) {
            ("survey", "--entry" | "--seed") => {
                "survey uses a linear sweep and does not accept --entry or --seed".into()
            }
            (_, "--out" | "--target" | "--emit-only") => {
                "--out, --target and --emit-only require recompile".into()
            }
            (command, option) => format!("{command} does not accept {option}"),
        });
    }
    if o.native_gpu && !o.horizon {
        return Err("--native-gpu requires --horizon".into());
    }
    Ok(o)
}
fn source_path(output: &Path) -> PathBuf {
    let mut name = output.as_os_str().to_owned();
    name.push(".rs");
    PathBuf::from(name)
}
fn json(value: &impl serde::Serialize) -> Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(value).map_err(|e| e.to_string())?
    );
    Ok(())
}

/// Reads the NSO modules of an ExeFS-style directory, in loader order.
fn exefs_modules(dir: &Path) -> Result<Vec<(String, Module)>> {
    let mut modules = Vec::new();
    for name in MODULE_ORDER.iter().copied().chain(["sdk"]) {
        let candidates = [dir.join(name), dir.join(format!("{name}.nso"))];
        let present: Vec<_> = candidates.iter().filter(|p| p.is_file()).collect();
        match present.as_slice() {
            [] => {}
            [path] => {
                let module = Module::parse(&read_input(path)?)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                modules.push((name.to_string(), module));
            }
            _ => return Err(format!("{name}: both {name} and {name}.nso exist")),
        }
    }
    if modules.is_empty() {
        return Err(format!(
            "{}: no NSO modules named rtld, main, subsdk0-9 or sdk",
            dir.display()
        ));
    }
    Ok(modules)
}
/// Whether `bytes` is an NRO (magic `NRO0` at 0x10).
fn is_nro(bytes: &[u8]) -> bool {
    bytes.get(0x10..0x14) == Some(b"NRO0")
}
/// An NRO as a one-module process: segments from wl-nro's execution plan, MOD0/dynamic
/// discovery as for NSO, guest linking (libnx's crt0 relocates itself).
fn load_nro(o: &Options, bytes: &[u8]) -> Result<Process> {
    let nro = wl_nro::Nro::parse(bytes).map_err(|e| format!("NRO: {e}"))?;
    let plan = nro.plan().map_err(|e| format!("NRO: {e}"))?;
    if let wl_nro::plan::RelocationRequirement::HostRequired { .. } = plan.relocation {
        return Err("NRO requires host relocation; only self-relocating NROs are supported".into());
    }
    if plan.entry_offset != 0 || !plan.unsupported_features.is_empty() {
        return Err(format!(
            "NRO entry at {:#x} or unsupported features {:?}",
            plan.entry_offset, plan.unsupported_features
        ));
    }
    let image = &plan.image;
    let segment = |s: &wl_nro::image::NroSegment| {
        let mut data = s.bytes.to_vec();
        data.resize(s.memory_size as usize, 0);
        wl_nso::Segment {
            name: s.name,
            file_offset: wl_common::FileOffset(s.file_offset.0),
            guest_offset: s.memory_offset,
            file_size: s.file_size,
            memory_size: s.memory_size,
            compressed: false,
            hash_verified: false,
            readable: s.readable,
            writable: s.writable,
            executable: s.executable,
            bytes: data,
        }
    };
    let digest = wl_nso::sha256_hex(bytes);
    let module = Module::from_segments(
        "NRO0",
        digest,
        image.module_id_hex.clone(),
        vec![
            segment(&image.text),
            segment(&image.rodata),
            segment(&image.data),
        ],
        u32::try_from(image.bss_size).map_err(|_| "NRO BSS too large")?,
    )?;
    Process::build(
        &[("main".to_string(), module)],
        o.base.unwrap_or(GuestAddr(DEFAULT_CODE_BASE)),
        o.link.unwrap_or(LinkMode::Guest),
    )
}
fn load_process(o: &Options) -> Result<Process> {
    let modules = exefs_modules(&o.input)?;
    // A Horizon process links itself (rtld or a self-relocating crt0).
    let mode = o.link.unwrap_or(if modules[0].0 == "rtld" || o.horizon {
        LinkMode::Guest
    } else {
        LinkMode::Host
    });
    Process::build(
        &modules,
        o.base.unwrap_or(GuestAddr(DEFAULT_CODE_BASE)),
        mode,
    )
}
/// Process parameters: from `main.npdm` when the ExeFS has one, otherwise documented defaults.
fn horizon_options(
    o: &Options,
    p: &Process,
    homebrew: bool,
) -> Result<wl_codegen_rust::HorizonOptions> {
    let npdm_path = o.input.join("main.npdm");
    let mut options = wl_codegen_rust::HorizonOptions {
        code_start: p.base.0,
        code_end: p.base.0 + p.size,
        program_id: 0,
        main_thread_stack_size: 0x10_0000,
        main_thread_priority: 44,
        ideal_core: 0,
        core_mask: 1,
        priority_mask: u64::MAX,
        handle_table_size: 0,
        is_application: true,
        svc_permissions: [u64::MAX; 3],
        service_access: vec![(false, "*".into()), (true, "*".into())],
        memory_limit: o.memory_limit.unwrap_or(0x2000_0000),
        source: "defaults (no main.npdm)",
        homebrew,
    };
    if !npdm_path.is_file() {
        return Ok(options);
    }
    let bytes = fs::read(&npdm_path).map_err(|e| format!("{}: {e}", npdm_path.display()))?;
    let npdm = wl_npdm::Npdm::parse(&bytes).map_err(|e| format!("main.npdm: {e}"))?;
    let meta = npdm.meta();
    if !meta.is_64_bit_instruction
        || meta.address_space != wl_npdm::ProcessAddressSpace::AddressSpace64Bit39
    {
        return Err(format!(
            "main.npdm requests {:?} ({}); only 64-bit processes with a 39-bit address space are supported",
            meta.address_space,
            if meta.is_64_bit_instruction {
                "64-bit"
            } else {
                "32-bit"
            },
        ));
    }
    let md = npdm
        .to_process_metadata()
        .map_err(|e| format!("main.npdm: {e}"))?;
    let mut svc_permissions = [0u64; 3];
    for id in 0..0xC0u8 {
        if md.syscall_bitmap.is_allowed(id) {
            svc_permissions[usize::from(id / 64)] |= 1 << (id % 64);
        }
    }
    options.program_id = md.program_id;
    options.main_thread_stack_size = md.main_thread_stack_size;
    options.main_thread_priority = md.main_thread_priority;
    options.ideal_core = md.ideal_core;
    options.core_mask = md.core_mask;
    options.priority_mask = md.priority_mask;
    options.handle_table_size = md.handle_table_size;
    options.is_application = md.is_application;
    if md.is_application && o.memory_limit.is_none() {
        // Default application budget; an explicit user limit always wins.
        options.memory_limit = 0xCE00_0000;
    }
    options.svc_permissions = svc_permissions;
    options.service_access = npdm
        .aci0_services()
        .map(|e| e.map(|e| (e.is_server, e.name.to_string())))
        .collect::<std::result::Result<_, _>>()
        .map_err(|e| format!("main.npdm service access control: {e}"))?;
    options.source = "main.npdm";
    Ok(options)
}
fn analysis_summary(program: &Program) -> serde_json::Value {
    let largest = program
        .blocks
        .iter()
        .map(|b| b.instructions.len())
        .max()
        .unwrap_or(0);
    let unsupported: Vec<_> = program
        .blocks
        .iter()
        .filter_map(|b| b.unsupported.as_ref())
        .take(32)
        .collect();
    serde_json::json!({
        "entry": program.entry,
        "required_roots": program.required_roots,
        "flow_roots": program.flow_roots,
        "jump_tables": program.jump_tables.len(),
        "jump_table_entries": program.jump_tables.values().sum::<usize>(),
        "speculative_roots_accepted": program.speculative_roots_accepted,
        "speculative_roots_rejected": program.rejected_roots.len(),
        "rejected_root_examples": program.rejected_roots.iter().take(16).collect::<Vec<_>>(),
        "instructions": program.instruction_count,
        "unsupported_sites": program.unsupported_count,
        "unsupported_examples": unsupported,
        "blocks": program.blocks.len(),
        "largest_block_instructions": largest,
        "indirect_sites": program.indirect_sites.len(),
    })
}
fn inspect(module: &Module, as_json: bool) -> Result<()> {
    let dynamic = module.mod0.as_ref().map(|m| {
        let d = &m.dynamic;
        let mut relocations = std::collections::BTreeMap::new();
        for r in &d.relocations {
            *relocations
                .entry(wl_nso::dynamic::relocation_name(r.kind))
                .or_insert(0u64) += 1;
        }
        serde_json::json!({
            "needed": d.needed,
            "soname": d.soname,
            "symbols": d.symbols.len(),
            "defined_symbols": d.symbols.iter().filter(|s| s.is_definition()).count(),
            "relocations": relocations,
            "rela_count": d.rela_count,
            "rel_count": d.rel_count,
            "pltrel": d.pltrel,
            "relr": d.relr.is_some(),
            "init": d.init,
            "fini": d.fini,
            "init_array_bytes": d.init_array.map(|a| a.size),
            "uninterpreted_tags": d.uninterpreted_tags,
        })
    });
    if as_json {
        let mut value = serde_json::to_value(module).map_err(|e| e.to_string())?;
        if let Some(m) = value.get_mut("mod0").and_then(|m| m.as_object_mut()) {
            m.remove("dynamic");
        }
        value["dynamic_summary"] = dynamic.unwrap_or(serde_json::Value::Null);
        return json(&value);
    }
    println!(
        "{} SHA-256={}\nmodule-id={} flags={:#x}",
        module.format, module.sha256, module.module_id, module.flags
    );
    for s in &module.segments {
        println!(
            "{} file={:#x}+{:#x} guest-offset={:#x} size={:#x} compressed={} hash-verified={} permissions={}{}{}",
            s.name,
            s.file_offset.0,
            s.file_size,
            s.guest_offset.0,
            s.memory_size,
            s.compressed,
            s.hash_verified,
            if s.readable { "r" } else { "-" },
            if s.writable { "w" } else { "-" },
            if s.executable { "x" } else { "-" }
        );
    }
    println!(
        "BSS guest-offset={:#x} size={:#x}\nMOD0={}",
        module.bss_offset.0,
        module.bss_size,
        module.mod0.is_some(),
    );
    if let Some(d) = dynamic {
        println!("dynamic={d}");
    }
    println!("{}", module.constraints);
    Ok(())
}
fn run(args: &[OsString]) -> Result<()> {
    let o = options(args)?;
    if o.command == "intake" {
        return json(&wl_package::intake(&o.input, o.verify_content)?);
    }
    let directory = o.input.is_dir();
    if directory && o.raw {
        return Err("--raw requires a file input".into());
    }
    if !directory && (o.link.is_some() || o.command == "link") {
        return Err("process loading (link, --link) requires an ExeFS directory input".into());
    }
    if o.command == "inspect" {
        if directory {
            return Err("inspect takes one NSO file; use link for a directory".into());
        }
        return inspect(&Module::parse(&read_input(&o.input)?)?, o.json);
    }
    let nro = if directory {
        None
    } else {
        Some(read_input(&o.input)?).filter(|b| is_nro(b))
    };
    let process = if directory {
        Some(load_process(&o)?)
    } else if let Some(bytes) = &nro {
        Some(load_nro(&o, bytes)?)
    } else {
        None
    };
    if o.command == "link" {
        return json(&process);
    }
    let image = match &process {
        Some(p) => Image::process(p),
        None => {
            let bytes = read_input(&o.input)?;
            let base = o.base.unwrap_or(GuestAddr(RAW_BASE));
            if o.raw {
                Image::raw(bytes, base)?
            } else {
                Image::nso(&Module::parse(&bytes)?, base)?
            }
        }
    };
    if o.command == "survey" {
        return json(&survey(&image, o.examples)?);
    }
    let entry = match (&process, o.entry) {
        (_, Some(e)) => e,
        (Some(p), None) => p.entry,
        (None, None) => {
            return Err("--entry is required; NSO does not specify a process entry point".into());
        }
    };
    let mut speculative: Vec<GuestAddr> = match &process {
        Some(p) if o.pointer_seeds => p.code_pointers.iter().map(|c| c.address).collect(),
        _ => Vec::new(),
    };
    if let Some(path) = &o.roots_file {
        let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        for line in text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
        {
            speculative.push(GuestAddr(number(line)?));
        }
    }
    let roots = Roots {
        required: std::iter::once(entry)
            .chain(o.seeds.iter().copied())
            .collect(),
        speculative,
        allow_unsupported: o.allow_unsupported,
        instruction_limit: None,
    };
    let started = Instant::now();
    let program = discover_roots(&image, &roots)?;
    let analysis_seconds = started.elapsed().as_secs_f64();
    if o.command == "analyze" {
        return if o.summary {
            let mut s = analysis_summary(&program);
            s["analysis_seconds"] = analysis_seconds.into();
            json(&s)
        } else {
            json(&program)
        };
    }
    if let Some(blocker) = process.as_ref().and_then(|p| p.execution_blocker.as_ref()) {
        return Err(format!("process image is not executable: {blocker}"));
    }
    let output = o.out.clone().ok_or("recompile requires --out EXECUTABLE")?;
    let source = source_path(&output);
    let input = fs::canonicalize(&o.input).map_err(|e| e.to_string())?;
    for path in [&output, &source] {
        if fs::canonicalize(path).ok().as_ref() == Some(&input) {
            return Err("output would overwrite input".into());
        }
    }
    let input_name = o
        .input
        .file_name()
        .map_or("input".into(), |n| n.to_string_lossy().into_owned());
    let stem = source
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("output file name must be valid UTF-8")?;
    let modules: Vec<(u64, u64, String)> = match &process {
        Some(p) => p
            .modules
            .iter()
            .map(|m| (m.base.0, m.base.0 + m.size, m.name.clone()))
            .collect(),
        None => image
            .mappings
            .iter()
            .filter(|m| m.execute)
            .map(|m| {
                (
                    m.base.0,
                    m.base.0 + m.bytes.len() as u64,
                    input_name.clone(),
                )
            })
            .collect(),
    };
    let started = Instant::now();
    let emit = if o.native_gpu {
        wl_codegen_rust::emit_with_native_gpu
    } else {
        wl_codegen_rust::emit
    };
    let generated = emit(
        &image,
        &program,
        stem,
        &modules,
        wl_codegen_rust::EmitOptions {
            unit_shift: o.unit_shift,
            horizon: match (&process, o.horizon) {
                (Some(p), true) => Some(horizon_options(&o, p, nro.is_some())?),
                (None, true) => {
                    return Err("--horizon requires an ExeFS directory or NRO input".into());
                }
                _ => None,
            },
        },
    )?;
    let emit_seconds = started.elapsed().as_secs_f64();
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut blob_bytes = 0;
    for (name, bytes) in &generated.blobs {
        let path = parent.join(name);
        if fs::canonicalize(&path).ok().as_ref() == Some(&input) {
            return Err("output would overwrite input".into());
        }
        fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        blob_bytes += bytes.len();
    }
    fs::write(&source, &generated.main).map_err(|e| format!("{}: {e}", source.display()))?;
    let started = Instant::now();
    let report = if o.emit_only {
        // Emit-only writes the unit sources beside the executable crate for inspection.
        let mut dir = source.as_os_str().to_owned();
        dir.push(".units");
        let dir = PathBuf::from(dir);
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        for unit in &generated.units {
            fs::write(dir.join(format!("{}.rs", unit.name)), &unit.source)
                .map_err(|e| e.to_string())?;
        }
        None
    } else {
        let cache_dir = o
            .cache_dir
            .clone()
            .or_else(|| env::var_os("WL_CACHE_DIR").map(PathBuf::from))
            .unwrap_or_else(|| parent.join(".wanderlust-cache"));
        let jobs = o.jobs.unwrap_or_else(|| {
            env::var("WL_JOBS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(|| {
                    std::thread::available_parallelism().map_or(1, |n| n.get().min(4))
                })
        });
        let cfg = wl_codegen_rust::build::BuildConfig {
            target: o.target.clone(),
            cache_dir,
            jobs,
            opt_level: 2,
        };
        Some(if o.native_gpu {
            wl_codegen_rust::build::build_with_native_gpu(
                &generated,
                &source,
                &output,
                &cfg,
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../..")
                    .as_path(),
            )?
        } else {
            wl_codegen_rust::build::build(&generated, &source, &output, &cfg)?
        })
    };
    let compile_seconds = started.elapsed().as_secs_f64();
    let unit_source_bytes: usize = generated.units.iter().map(|u| u.source.len()).sum();
    let executable_bytes = fs::metadata(&output)
        .ok()
        .map(|m| m.len())
        .filter(|_| !o.emit_only);
    json(&serde_json::json!({
        "source": source,
        "executable": if o.emit_only { None } else { Some(&output) },
        "entry": entry,
        "instructions": program.instruction_count,
        "unsupported_sites": program.unsupported_count,
        "blocks": program.blocks.len(),
        "speculative_roots_accepted": program.speculative_roots_accepted,
        "speculative_roots_rejected": program.rejected_roots.len(),
        "flow_roots": program.flow_roots,
        "jump_tables": program.jump_tables.len(),
        "relocation": image.relocation,
        "generated_source_bytes": generated.main.len() + unit_source_bytes,
        "units": generated.units.len(),
        "build": report.as_ref().map(|r| serde_json::json!({
            "toolchain": r.toolchain,
            "jobs": r.jobs,
            "cache_hits": r.cache_hits,
            "cache_misses": r.cache_misses,
            "runtime_cached": r.runtime.cached,
            "runtime_seconds": r.runtime.seconds,
            "unit_compile_seconds": r.unit_compile_seconds,
            "units_wall_seconds": r.units_wall_seconds,
            "slowest_unit_seconds": r.units.iter().map(|u| u.seconds).fold(0.0, f64::max),
            "main_seconds": r.main_seconds,
            "native_gpu": r.native_gpu,
        })),
        "memory_image_bytes": blob_bytes,
        "executable_bytes": executable_bytes,
        "analysis_seconds": analysis_seconds,
        "emit_seconds": emit_seconds,
        "rustc_seconds": if o.emit_only { None } else { Some(compile_seconds) },
        "target": o.target,
        "module_id": image.module_id,
    }))
}
fn main() {
    let args: Vec<_> = env::args_os().skip(1).collect();
    if args.is_empty() || args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{HELP}");
        return;
    }
    if let Err(e) = run(&args) {
        eprintln!("wanderlust: {e}");
        process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_memory_limits_survive_application_npdm() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/horizon/probe");
        for (argument, expected) in [
            (None, 0xce00_0000),
            (Some("0x20000000"), 0x2000_0000),
            (Some("0x4000000"), 0x400_0000),
        ] {
            let mut args = vec![
                OsString::from("recompile"),
                fixture.as_os_str().to_owned(),
                OsString::from("--horizon"),
            ];
            if let Some(value) = argument {
                args.extend([OsString::from("--memory-limit"), OsString::from(value)]);
            }
            let options = options(&args).unwrap();
            let process = load_process(&options).unwrap();
            let config = horizon_options(&options, &process, false).unwrap();
            assert!(config.is_application);
            assert_eq!(config.memory_limit, expected);
        }
    }
}
