//! Separate, cached and parallel compilation of generated crates.
//!
//! Crate graph: `wl_guest_runtime` (rlib) ← every translation unit `wlu_<start>` (rlib) ← the
//! executable crate. A unit only calls the runtime; control transfers between blocks return
//! to the runtime's dispatcher, so units never reference one another. Each rlib is stored in
//! the cache under the SHA-256 of everything that determines its bytes: its source, the
//! runtime key it links against, the `rustc -vV` identity, the target and the flags.
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Instant,
};
use wl_common::Result;

use crate::Emitted;
#[path = "native_gpu.rs"]
mod native_gpu;
pub use native_gpu::NativeGpuReport;
pub(crate) fn native_adapter_source() -> &'static str {
    native_gpu::ADAPTER
}

/// Runtime sources compiled into the `wl_guest_runtime` rlib.
pub const RUNTIME_SOURCES: [(&str, &str); 13] = [
    ("lib.rs", include_str!("../../wl-runtime/src/lib.rs")),
    ("crypto.rs", include_str!("../../wl-runtime/src/crypto.rs")),
    (
        "display.rs",
        include_str!("../../wl-runtime/src/display.rs"),
    ),
    ("fp.rs", include_str!("../../wl-runtime/src/fp.rs")),
    (
        "gpu_memory.rs",
        include_str!("../../wl-runtime/src/gpu_memory.rs"),
    ),
    (
        "horizon.rs",
        include_str!("../../wl-runtime/src/horizon.rs"),
    ),
    ("input.rs", include_str!("../../wl-runtime/src/input.rs")),
    ("ipc.rs", include_str!("../../wl-runtime/src/ipc.rs")),
    ("nvidia.rs", include_str!("../../wl-runtime/src/nvidia.rs")),
    ("sdl.rs", include_str!("../../wl-runtime/src/sdl.rs")),
    (
        "services.rs",
        include_str!("../../wl-runtime/src/services.rs"),
    ),
    ("simd.rs", include_str!("../../wl-runtime/src/simd.rs")),
    (
        "threads.rs",
        include_str!("../../wl-runtime/src/threads.rs"),
    ),
];

#[derive(Clone, Debug)]
pub struct BuildConfig {
    pub target: Option<String>,
    pub cache_dir: PathBuf,
    pub jobs: usize,
    pub opt_level: u8,
}
#[derive(Clone, Debug, serde::Serialize)]
pub struct CrateReport {
    pub name: String,
    pub key: String,
    pub cached: bool,
    pub seconds: f64,
    pub source_bytes: usize,
}
#[derive(Clone, Debug, serde::Serialize)]
pub struct BuildReport {
    pub toolchain: String,
    pub jobs: usize,
    pub runtime: CrateReport,
    pub units: Vec<CrateReport>,
    pub cache_hits: usize,
    pub cache_misses: usize,
    /// Sum of per-unit rustc wall times (exceeds elapsed time when compiled in parallel).
    pub unit_compile_seconds: f64,
    pub units_wall_seconds: f64,
    pub main_seconds: f64,
    pub native_gpu: Option<NativeGpuReport>,
}

/// A compiled (or cached) rlib: path, whether it came from the cache, compile seconds.
type Built = Result<(PathBuf, bool, f64)>;

fn rustc() -> Command {
    Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
}
fn hash(parts: &[&[u8]]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update((p.len() as u64).to_le_bytes());
        h.update(p);
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}
fn run(mut cmd: Command, what: &str) -> Result<()> {
    let out = cmd
        .output()
        .map_err(|e| format!("cannot launch rustc for {what}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "native compilation of {what} failed ({}):\n{}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(())
}
fn common(cmd: &mut Command, cfg: &BuildConfig) {
    cmd.args([
        "--edition=2024",
        "-C",
        &format!("opt-level={}", cfg.opt_level),
        "-C",
        "overflow-checks=yes",
        "-D",
        "warnings",
    ]);
    if let Some(target) = &cfg.target {
        cmd.args(["--target", target]);
    }
}
fn flags_identity(cfg: &BuildConfig) -> String {
    format!(
        "edition=2024;opt-level={};overflow-checks=yes;target={}",
        cfg.opt_level,
        cfg.target.as_deref().unwrap_or("host")
    )
}
/// Compiles `source` (written to `<dir>/src/<file>`) into `<dir>/lib<name>.rlib` unless the
/// keyed directory already exists. Population is atomic: a temporary directory is renamed.
fn cached_rlib(
    cfg: &BuildConfig,
    name: &str,
    key: &str,
    files: &[(&str, &str)],
    runtime: Option<&Path>,
) -> Built {
    let dir = cfg.cache_dir.join(format!("{name}-{}", &key[..20]));
    let rlib = dir.join(format!("lib{name}.rlib"));
    if rlib.is_file() {
        return Ok((rlib, true, 0.0));
    }
    let tmp = cfg
        .cache_dir
        .join(format!(".tmp-{name}-{}-{}", &key[..20], std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(tmp.join("src")).map_err(|e| format!("{}: {e}", tmp.display()))?;
    for (file, text) in files {
        fs::write(tmp.join("src").join(file), text).map_err(|e| e.to_string())?;
    }
    let started = Instant::now();
    let mut cmd = rustc();
    cmd.arg(tmp.join("src").join(files[0].0))
        .args(["--crate-type", "rlib", "--crate-name", name, "-C"])
        .arg(format!("metadata={}", &key[..16]));
    common(&mut cmd, cfg);
    if let Some(runtime) = runtime {
        cmd.arg("--extern")
            .arg(format!("wl_guest_runtime={}", runtime.display()));
    }
    cmd.arg("-o").arg(tmp.join(format!("lib{name}.rlib")));
    run(cmd, name)?;
    let seconds = started.elapsed().as_secs_f64();
    if fs::rename(&tmp, &dir).is_err() {
        // Another build populated the same key concurrently; its result is equivalent.
        let _ = fs::remove_dir_all(&tmp);
    }
    Ok((rlib, false, seconds))
}
fn link_or_copy(from: &Path, to: &Path) -> Result<()> {
    let _ = fs::remove_file(to);
    if fs::hard_link(from, to).is_err() {
        fs::copy(from, to).map_err(|e| format!("{}: {e}", to.display()))?;
    }
    Ok(())
}

/// Builds the executable at `output` from `main_source` (already written, with its memory
/// images beside it) and the emitted units.
pub fn build(
    emitted: &Emitted,
    main_source: &Path,
    output: &Path,
    cfg: &BuildConfig,
) -> Result<BuildReport> {
    build_inner(emitted, main_source, output, cfg, None)
}

/// Builds a GPU-enabled final executable against target-correct Cargo artifacts.
/// CPU translation units still link only the exact embedded guest runtime.
pub fn build_with_native_gpu(
    emitted: &Emitted,
    main_source: &Path,
    output: &Path,
    cfg: &BuildConfig,
    workspace: &Path,
) -> Result<BuildReport> {
    build_inner(emitted, main_source, output, cfg, Some(workspace))
}

fn build_inner(
    emitted: &Emitted,
    main_source: &Path,
    output: &Path,
    cfg: &BuildConfig,
    gpu_workspace: Option<&Path>,
) -> Result<BuildReport> {
    fs::create_dir_all(&cfg.cache_dir).map_err(|e| format!("{}: {e}", cfg.cache_dir.display()))?;
    let version = rustc()
        .arg("-vV")
        .output()
        .map_err(|e| format!("cannot launch rustc: {e}"))?;
    if !version.status.success() {
        return Err("rustc -vV failed".into());
    }
    let toolchain = String::from_utf8_lossy(&version.stdout).into_owned();
    let native = gpu_workspace
        .map(|workspace| native_gpu::prepare(workspace, cfg, &toolchain))
        .transpose()?;
    let flags = flags_identity(cfg);
    let mut runtime_parts: Vec<&[u8]> =
        vec![b"wl_guest_runtime", toolchain.as_bytes(), flags.as_bytes()];
    for (name, text) in &RUNTIME_SOURCES {
        runtime_parts.push(name.as_bytes());
        runtime_parts.push(text.as_bytes());
    }
    let runtime_key = hash(&runtime_parts);
    let (runtime_rlib, runtime_cached, runtime_seconds) = cached_rlib(
        cfg,
        "wl_guest_runtime",
        &runtime_key,
        &RUNTIME_SOURCES,
        None,
    )?;
    let runtime = CrateReport {
        name: "wl_guest_runtime".into(),
        key: runtime_key.clone(),
        cached: runtime_cached,
        seconds: runtime_seconds,
        source_bytes: RUNTIME_SOURCES.iter().map(|s| s.1.len()).sum(),
    };
    // Units: keyed, then compiled in parallel by `jobs` workers.
    let keys: Vec<String> = emitted
        .units
        .iter()
        .map(|u| {
            hash(&[
                u.name.as_bytes(),
                u.source.as_bytes(),
                runtime_key.as_bytes(),
                toolchain.as_bytes(),
                flags.as_bytes(),
            ])
        })
        .collect();
    let results: Mutex<Vec<Option<Built>>> =
        Mutex::new((0..emitted.units.len()).map(|_| None).collect());
    let next = AtomicUsize::new(0);
    let jobs = cfg.jobs.max(1);
    let started = Instant::now();
    std::thread::scope(|scope| {
        for _ in 0..jobs.min(emitted.units.len().max(1)) {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(unit) = emitted.units.get(i) else {
                        break;
                    };
                    let result = cached_rlib(
                        cfg,
                        &unit.name,
                        &keys[i],
                        &[("unit.rs", unit.source.as_str())],
                        Some(&runtime_rlib),
                    );
                    let failed = result.is_err();
                    results.lock().map(|mut r| r[i] = Some(result)).ok();
                    if failed {
                        // Stop handing out work after the first failure.
                        next.store(usize::MAX / 2, Ordering::Relaxed);
                    }
                }
            });
        }
    });
    let units_wall_seconds = started.elapsed().as_secs_f64();
    let mut units = Vec::new();
    let mut rlibs = vec![runtime_rlib];
    for (i, result) in results
        .into_inner()
        .map_err(|_| "unit build state poisoned")?
        .into_iter()
        .enumerate()
    {
        let (rlib, cached, seconds) = result.ok_or("unit compilation did not run")??;
        rlibs.push(rlib);
        units.push(CrateReport {
            name: emitted.units[i].name.clone(),
            key: keys[i].clone(),
            cached,
            seconds,
            source_bytes: emitted.units[i].source.len(),
        });
    }
    // One dependency directory per executable keeps the rustc command line short.
    let mut deps = output.as_os_str().to_owned();
    deps.push(".deps");
    let deps = PathBuf::from(deps);
    if deps.is_dir() {
        for entry in fs::read_dir(&deps).map_err(|e| e.to_string())?.flatten() {
            if entry.path().extension().is_some_and(|e| e == "rlib") {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    fs::create_dir_all(&deps).map_err(|e| format!("{}: {e}", deps.display()))?;
    for rlib in &rlibs {
        link_or_copy(rlib, &deps.join(rlib.file_name().ok_or("rlib name")?))?;
    }
    let started = Instant::now();
    let mut cmd = rustc();
    cmd.arg(main_source)
        .args(["--crate-name", "wanderlust_guest", "-L"])
        .arg(format!("crate={}", deps.display()));
    common(&mut cmd, cfg);
    if let Some(native) = &native {
        native.verify()?;
        native.link(&mut cmd);
    }
    // Extra linker configuration is deliberately a single executable path, not a shell string.
    if let Some(linker) = std::env::var_os("WL_LINKER") {
        if cfg.target.as_deref() == Some("x86_64-pc-windows-gnu") {
            cmd.args(["-C", "link-self-contained=yes"]);
        }
        cmd.arg("-C")
            .arg(format!("linker={}", linker.to_string_lossy()));
    }
    for var in ["WL_NATIVE_LIB_DIR", "WL_GCC_LIB_DIR"] {
        if let Some(lib) = std::env::var_os(var) {
            cmd.arg("-L")
                .arg(format!("native={}", lib.to_string_lossy()));
        }
    }
    let temporary = native
        .as_ref()
        .map(|_| native_gpu::temporary_output(output));
    cmd.arg("-o").arg(temporary.as_deref().unwrap_or(output));
    run(cmd, "executable crate")?;
    if let Some(native) = &native {
        native.verify()?;
        fs::rename(temporary.as_ref().unwrap(), output).map_err(|e| e.to_string())?;
    }
    let cache_hits = units.iter().filter(|u| u.cached).count() + runtime.cached as usize;
    Ok(BuildReport {
        toolchain: toolchain.lines().next().unwrap_or_default().to_string(),
        jobs,
        cache_misses: units.len() + 1 - cache_hits,
        cache_hits,
        unit_compile_seconds: units.iter().map(|u| u.seconds).sum(),
        units_wall_seconds,
        main_seconds: started.elapsed().as_secs_f64(),
        runtime,
        units,
        native_gpu: native.map(|v| v.report),
    })
}
