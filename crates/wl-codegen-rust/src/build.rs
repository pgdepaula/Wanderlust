//! Separate, cached and parallel compilation of generated crates.
//!
//! Crate graph: `wl_guest_runtime` (rlib) ← every translation unit `wlu_<start>` (rlib) ← the
//! executable crate. A unit only calls the runtime; control transfers between blocks return
//! to the runtime's dispatcher, so units never reference one another. Each rlib is stored in
//! the cache under the SHA-256 of everything that determines its bytes: its source, the
//! runtime key it links against, the `rustc -vV` identity, the target and the flags.
use sha2::{Digest, Sha256};
use std::{
    borrow::Cow,
    fs,
    io::{Read, Seek, SeekFrom},
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
pub const RUNTIME_SOURCES: [(&str, &str); 15] = [
    ("lib.rs", include_str!("../../wl-runtime/src/lib.rs")),
    ("audio.rs", include_str!("../../wl-runtime/src/audio.rs")),
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
        "storage.rs",
        include_str!("../../wl-runtime/src/storage.rs"),
    ),
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
// Check archive framing without rereading object payloads. This detects missing,
// empty and truncated artifacts; it is not a checksum of compiled machine code.
fn complete_archive(path: &Path) -> bool {
    let check = || -> std::io::Result<bool> {
        let mut file = fs::File::open(path)?;
        let length = file.metadata()?.len();
        let mut magic = [0; 8];
        file.read_exact(&mut magic)?;
        if &magic != b"!<arch>\n" {
            return Ok(false);
        }
        let mut cursor = 8u64;
        let mut metadata = false;
        while cursor < length {
            let mut header = [0; 60];
            file.seek(SeekFrom::Start(cursor))?;
            file.read_exact(&mut header)?;
            if &header[58..] != b"`\n" {
                return Ok(false);
            }
            let Some(size) = std::str::from_utf8(&header[48..58])
                .ok()
                .and_then(|s| s.trim().parse::<u64>().ok())
            else {
                return Ok(false);
            };
            let Some(end) = cursor.checked_add(60).and_then(|v| v.checked_add(size)) else {
                return Ok(false);
            };
            let Some(next) = end.checked_add(size & 1).filter(|v| *v <= length) else {
                return Ok(false);
            };
            metadata |= size != 0
                && std::str::from_utf8(&header[..16])
                    .is_ok_and(|s| s.trim().trim_end_matches('/') == "lib.rmeta");
            cursor = next;
        }
        Ok(metadata && cursor == length)
    };
    check().unwrap_or(false)
}

/// Compiles `source` (written to `<dir>/src/<file>`) into `<dir>/lib<name>.rlib` unless
/// a structurally complete archive is cached. Population uses a per-key kernel lock
/// and publishes a temporary directory by rename; incomplete entries are quarantined.
fn cached_rlib(
    cfg: &BuildConfig,
    name: &str,
    key: &str,
    files: &[(&str, &str)],
    runtime: Option<&Path>,
) -> Built {
    let dir = cfg.cache_dir.join(format!("{name}-{}", &key[..20]));
    let rlib = dir.join(format!("lib{name}.rlib"));
    if complete_archive(&rlib) {
        return Ok((rlib, true, 0.0));
    }
    // Only cache population/repair takes the kernel lock. Complete immutable
    // archives keep their read-only hit path. Process exit releases this lock.
    let population_lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(cfg.cache_dir.join(format!(".lock-{name}-{}", &key[..20])))
        .map_err(|e| format!("cannot open archive population lock: {e}"))?;
    fs2::FileExt::lock_exclusive(&population_lock)
        .map_err(|e| format!("cannot lock archive population: {e}"))?;
    if complete_archive(&rlib) {
        return Ok((rlib, true, 0.0));
    }
    if dir.exists() {
        // Preserve interrupted output for diagnosis, including any hard-linked
        // readers. A fresh directory can then be published under the same key.
        static REPAIRS: AtomicUsize = AtomicUsize::new(0);
        let invalid = cfg.cache_dir.join(format!(
            ".invalid-{name}-{}-{}-{}",
            &key[..20],
            std::process::id(),
            REPAIRS.fetch_add(1, Ordering::Relaxed)
        ));
        if let Err(error) = fs::rename(&dir, &invalid) {
            if complete_archive(&rlib) {
                return Ok((rlib, true, 0.0));
            }
            if dir.exists() {
                return Err(format!("cannot quarantine {}: {error}", dir.display()));
            }
        }
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
    let compiled = tmp.join(format!("lib{name}.rlib"));
    if !complete_archive(&compiled) {
        return Err(format!(
            "compiler produced an incomplete archive for {name}"
        ));
    }
    fs::OpenOptions::new()
        .write(true)
        .open(&compiled)
        .and_then(|file| file.sync_all())
        .map_err(|e| format!("cannot persist {}: {e}", compiled.display()))?;
    let seconds = started.elapsed().as_secs_f64();
    if let Err(error) = fs::rename(&tmp, &dir) {
        if !complete_archive(&rlib) {
            return Err(format!(
                "cannot publish compiled archive for {name}: {error}"
            ));
        }
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
    build_inner(
        BuildInput::Borrowed(emitted),
        main_source,
        output,
        cfg,
        None,
    )
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
    build_inner(
        BuildInput::Borrowed(emitted),
        main_source,
        output,
        cfg,
        Some(workspace),
    )
}

/// Consumes emitted sources so their storage can be released after unit
/// compilation, before the final linker competes for host memory.
pub fn build_owned(
    emitted: Emitted,
    main_source: &Path,
    output: &Path,
    cfg: &BuildConfig,
    gpu_workspace: Option<&Path>,
) -> Result<BuildReport> {
    build_inner(
        BuildInput::Owned(emitted),
        main_source,
        output,
        cfg,
        gpu_workspace,
    )
}

/// A trusted generated Rust unit stored on disk. Its SHA-256 is checked before
/// cache creation and again when a worker acquires it; cache identity is identical
/// to the in-memory builder. Callers keep source files immutable during the build.
#[derive(Clone, Debug)]
pub struct UnitFile {
    pub name: String,
    pub path: PathBuf,
    pub sha256: String,
}
/// Bound each generated source acquisition independently of total program size.
pub const MAX_SOURCE_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// Digest of the exact generated UTF-8 source bytes, independent of its path.
pub fn source_sha256(source: &str) -> String {
    format!("{:x}", Sha256::digest(source.as_bytes()))
}
fn checked_file(path: &Path, expected: &str) -> Result<String> {
    if expected.len() != 64 || !expected.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("invalid generated source SHA-256".into());
    }
    let file = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if file.metadata().map_err(|e| e.to_string())?.len() > MAX_SOURCE_FILE_BYTES {
        return Err(format!(
            "generated source exceeds file budget: {}",
            path.display()
        ));
    }
    let mut source = String::new();
    file.take(MAX_SOURCE_FILE_BYTES + 1)
        .read_to_string(&mut source)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if source.len() as u64 > MAX_SOURCE_FILE_BYTES {
        return Err(format!(
            "generated source exceeds file budget: {}",
            path.display()
        ));
    }
    let actual = source_sha256(&source);
    if actual != expected.to_ascii_lowercase() {
        return Err(format!(
            "generated source digest mismatch: {}",
            path.display()
        ));
    }
    Ok(source)
}
/// Compiles previously published sources without retaining every unit's text.
/// At most `jobs` unit source strings are held during compilation. Source text,
/// runtime, target and compiler flags retain the existing cache-key semantics.
pub fn build_files(
    files: Vec<UnitFile>,
    main_source: &Path,
    main_sha256: &str,
    output: &Path,
    cfg: &BuildConfig,
    gpu_workspace: Option<&Path>,
) -> Result<BuildReport> {
    let mut names = std::collections::BTreeSet::new();
    for file in &files {
        let address = file
            .name
            .strip_prefix("wlu_")
            .and_then(|v| u64::from_str_radix(v, 16).ok());
        if address.is_none_or(|v| format!("wlu_{v:x}") != file.name) || !names.insert(&file.name) {
            return Err("invalid or duplicate generated unit name".into());
        }
    }
    let main = checked_file(main_source, main_sha256)?;
    build_inner(
        BuildInput::Files {
            main,
            files,
            main_sha256: main_sha256.into(),
        },
        main_source,
        output,
        cfg,
        gpu_workspace,
    )
}
#[derive(Clone, Copy)]
enum UnitInput<'a> {
    Inline(&'a crate::Unit),
    File(&'a UnitFile),
}
impl UnitInput<'_> {
    fn name(&self) -> &str {
        match self {
            Self::Inline(u) => &u.name,
            Self::File(u) => &u.name,
        }
    }
    fn source(&self) -> Result<Cow<'_, str>> {
        match self {
            Self::Inline(u) => Ok(Cow::Borrowed(&u.source)),
            Self::File(u) => checked_file(&u.path, &u.sha256).map(Cow::Owned),
        }
    }
}
enum BuildInput<'a> {
    Borrowed(&'a Emitted),
    Owned(Emitted),
    Files {
        main: String,
        files: Vec<UnitFile>,
        main_sha256: String,
    },
}
impl BuildInput<'_> {
    fn sources(&self) -> (&str, Vec<UnitInput<'_>>) {
        match self {
            Self::Borrowed(e) => (&e.main, e.units.iter().map(UnitInput::Inline).collect()),
            Self::Owned(e) => (&e.main, e.units.iter().map(UnitInput::Inline).collect()),
            Self::Files { main, files, .. } => (main, files.iter().map(UnitInput::File).collect()),
        }
    }
}

fn build_inner(
    input: BuildInput<'_>,
    main_source: &Path,
    output: &Path,
    cfg: &BuildConfig,
    gpu_workspace: Option<&Path>,
) -> Result<BuildReport> {
    let (main, sources) = input.sources();
    // A truncated diagnostic source is not an empty guest program. Reject it
    // before rebuilding dependencies or populating an apparently valid rlib.
    if main.trim().is_empty() {
        return Err("empty emitted executable source".into());
    }
    // Validate all file descriptors before cache population, releasing each
    // temporary acquisition before reading the next source.
    for unit in &sources {
        if unit.source()?.trim().is_empty() {
            return Err(format!("empty emitted source for unit {}", unit.name()));
        }
    }
    if gpu_workspace.is_some() {
        let module = "pub mod native_gpu {\n";
        if main.contains(module)
            && !main.contains(&format!("{module}{}\n}}\n", native_gpu::ADAPTER))
        {
            return Err("emitted NVIDIA host adapter is stale; refresh the host entry before compiling guest units".into());
        }
    }
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
    // File sources are acquired and released by each worker, rather than
    // loading the entire generated program into the compiler parent's heap.
    type UnitBuilt = Result<(PathBuf, bool, f64, String, usize)>;
    let results: Mutex<Vec<Option<UnitBuilt>>> =
        Mutex::new((0..sources.len()).map(|_| None).collect());
    let next = AtomicUsize::new(0);
    let jobs = cfg.jobs.max(1);
    let started = Instant::now();
    std::thread::scope(|scope| {
        for _ in 0..jobs.min(sources.len().max(1)) {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(unit) = sources.get(i) else {
                        break;
                    };
                    let result = (|| {
                        let source = unit.source()?;
                        let key = hash(&[
                            unit.name().as_bytes(),
                            source.as_bytes(),
                            runtime_key.as_bytes(),
                            toolchain.as_bytes(),
                            flags.as_bytes(),
                        ]);
                        let (archive, cached, seconds) = cached_rlib(
                            cfg,
                            unit.name(),
                            &key,
                            &[("unit.rs", source.as_ref())],
                            Some(&runtime_rlib),
                        )?;
                        Ok((archive, cached, seconds, key, source.len()))
                    })();
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
        let (rlib, cached, seconds, key, source_bytes) =
            result.ok_or("unit compilation did not run")??;
        rlibs.push(rlib);
        units.push(CrateReport {
            name: sources[i].name().into(),
            key,
            cached,
            seconds,
            source_bytes,
        });
    }
    let unit_count = units.len();
    if let BuildInput::Files { main_sha256, .. } = &input {
        checked_file(main_source, main_sha256)?;
    }
    drop(sources);
    drop(input);
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
        cache_misses: unit_count + 1 - cache_hits,
        cache_hits,
        unit_compile_seconds: units.iter().map(|u| u.seconds).sum(),
        units_wall_seconds,
        main_seconds: started.elapsed().as_secs_f64(),
        runtime,
        units,
        native_gpu: native.map(|v| v.report),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file_test_directory(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "wanderlust-file-build-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn file_units_execute_and_reuse_identical_inline_cache_keys() {
        let directory = file_test_directory("cache");
        fs::create_dir_all(&directory).unwrap();
        let main = "extern crate wlu_10000; fn main(){assert_eq!(wlu_10000::value(),47);println!(\"47\");}\n";
        let unit = "pub fn value()->u32{47}\n";
        let source = directory.join("main.rs");
        let unit_source = directory.join("unit.rs");
        fs::write(&source, main).unwrap();
        fs::write(&unit_source, unit).unwrap();
        let cfg = BuildConfig {
            target: None,
            cache_dir: directory.join("cache"),
            jobs: 2,
            opt_level: 0,
        };
        let inline_output = directory.join("inline");
        let inline = build_owned(
            Emitted {
                main: main.into(),
                units: vec![crate::Unit {
                    name: "wlu_10000".into(),
                    start: 0x10000,
                    source: unit.into(),
                    blocks: 0,
                    instructions: 0,
                }],
                blobs: vec![],
            },
            &source,
            &inline_output,
            &cfg,
            None,
        )
        .unwrap();
        let file_output = directory.join("files");
        let files = build_files(
            vec![UnitFile {
                name: "wlu_10000".into(),
                path: unit_source,
                sha256: source_sha256(unit),
            }],
            &source,
            &source_sha256(main),
            &file_output,
            &cfg,
            None,
        )
        .unwrap();
        assert_eq!(inline.runtime.key, files.runtime.key);
        assert_eq!(inline.units[0].key, files.units[0].key);
        assert_eq!(files.cache_hits, 2);
        assert_eq!(files.cache_misses, 0);
        for path in [inline_output, file_output] {
            let run = Command::new(path).output().unwrap();
            assert!(run.status.success());
            assert_eq!(run.stdout, b"47\n");
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn file_sources_reject_mutation_truncation_and_oversize_before_cache_creation() {
        let directory = file_test_directory("invalid");
        fs::create_dir_all(&directory).unwrap();
        let main = "fn main() {}\n";
        let unit = "pub fn value()->u32{47}\n";
        let source = directory.join("main.rs");
        let unit_source = directory.join("unit.rs");
        fs::write(&source, main).unwrap();
        fs::write(&unit_source, unit).unwrap();
        let descriptor = UnitFile {
            name: "wlu_10000".into(),
            path: unit_source.clone(),
            sha256: source_sha256(unit),
        };
        assert_eq!(UnitInput::File(&descriptor).source().unwrap(), unit);
        let cfg = BuildConfig {
            target: None,
            cache_dir: directory.join("must-not-exist"),
            jobs: 2,
            opt_level: 0,
        };
        for bytes in [b"pub fn value()->u32{99}\n".as_slice(), b""] {
            fs::write(&unit_source, bytes).unwrap();
            // A second acquisition must reject stale content, even after a valid preflight.
            assert!(
                UnitInput::File(&descriptor)
                    .source()
                    .unwrap_err()
                    .contains("digest mismatch")
            );
            assert!(
                build_files(
                    vec![descriptor.clone()],
                    &source,
                    &source_sha256(main),
                    &directory.join("no-output"),
                    &cfg,
                    None
                )
                .is_err()
            );
            assert!(!cfg.cache_dir.exists());
        }
        fs::write(&unit_source, []).unwrap();
        let empty = UnitFile {
            sha256: source_sha256(""),
            ..descriptor.clone()
        };
        assert!(
            build_files(
                vec![empty],
                &source,
                &source_sha256(main),
                &directory.join("no-output"),
                &cfg,
                None
            )
            .unwrap_err()
            .contains("empty emitted source")
        );
        fs::OpenOptions::new()
            .write(true)
            .open(&unit_source)
            .unwrap()
            .set_len(MAX_SOURCE_FILE_BYTES + 1)
            .unwrap();
        assert!(
            UnitInput::File(&descriptor)
                .source()
                .unwrap_err()
                .contains("file budget")
        );
        for files in [
            vec![descriptor.clone(), descriptor.clone()],
            vec![UnitFile {
                name: "../escape".into(),
                ..descriptor
            }],
        ] {
            assert!(
                build_files(
                    files,
                    &source,
                    &source_sha256(main),
                    &directory.join("no-output"),
                    &cfg,
                    None
                )
                .unwrap_err()
                .contains("unit name")
            );
        }
        assert!(!cfg.cache_dir.exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn empty_emitted_crates_fail_before_creating_cache_or_launching_compilers() {
        let cache = std::env::temp_dir().join(format!(
            "wanderlust-empty-emitted-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let cfg = BuildConfig {
            target: None,
            cache_dir: cache.clone(),
            jobs: 2,
            opt_level: 0,
        };
        let source = cache.join("missing-main.rs");
        let output = cache.join("must-not-be-built");
        for text in ["", " \n\t"] {
            let emitted = Emitted {
                main: text.into(),
                units: Vec::new(),
                blobs: Vec::new(),
            };
            let error = build_owned(emitted, &source, &output, &cfg, None).unwrap_err();
            assert_eq!(error, "empty emitted executable source");
            assert!(!cache.exists());
            let emitted = Emitted {
                main: "fn main() {}".into(),
                units: vec![crate::Unit {
                    name: "wlu_10000".into(),
                    start: 0x10000,
                    source: text.into(),
                    blocks: 0,
                    instructions: 0,
                }],
                blobs: Vec::new(),
            };
            let error = build_owned(emitted, &source, &output, &cfg, None).unwrap_err();
            assert_eq!(error, "empty emitted source for unit wlu_10000");
            assert!(!cache.exists());
        }
    }

    #[test]
    fn stale_embedded_host_adapter_fails_before_guest_compilation_or_cache_creation() {
        let cache =
            std::env::temp_dir().join(format!("wanderlust-stale-host-{}", std::process::id()));
        assert!(!cache.exists());
        let cfg = BuildConfig {
            target: None,
            cache_dir: cache.clone(),
            jobs: 1,
            opt_level: 0,
        };
        let emitted = Emitted {
            // A formerly generated wrapper can compile against Host defaults,
            // yet omit required callbacks. Type checking alone cannot catch it.
            main: "pub mod native_gpu {\n// obsolete host callbacks\n}\nfn main() {}\n".into(),
            units: Vec::new(),
            blobs: Vec::new(),
        };
        let error = build_owned(
            emitted,
            &cache.join("main.rs"),
            &cache.join("output"),
            &cfg,
            Some(&cache.join("missing-workspace")),
        )
        .unwrap_err();
        assert!(error.contains("host adapter is stale"), "{error}");
        assert!(!cache.exists());
    }

    #[test]
    fn interrupted_empty_cached_archive_is_recompiled() {
        let directory = std::env::temp_dir().join(format!(
            "wanderlust-cache-repair-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&directory).unwrap();
        let cfg = BuildConfig {
            target: None,
            cache_dir: directory.clone(),
            jobs: 1,
            opt_level: 0,
        };
        let key = "0123456789abcdef0123456789abcdef";
        let source = [("lib.rs", "pub fn value() -> u32 { 47 }\n")];
        let (archive, hit, _) = cached_rlib(&cfg, "cache_probe", key, &source, None).unwrap();
        assert!(!hit);
        let complete = fs::read(&archive).unwrap();
        let damaged = [
            Vec::new(),
            b"!<arch>\n".to_vec(),
            complete[..complete.len() - 1].to_vec(),
            vec![0; complete.len()],
        ];
        for (i, bytes) in damaged.iter().enumerate() {
            fs::write(&archive, bytes).unwrap();
            let preserved = directory.join(format!("reader-{i}.rlib"));
            fs::hard_link(&archive, &preserved).unwrap();
            let (repaired, hit, _) = cached_rlib(&cfg, "cache_probe", key, &source, None).unwrap();
            assert!(!hit, "an incomplete artifact must not be a cache hit: {i}");
            assert!(complete_archive(&repaired));
            assert_eq!(
                fs::read(preserved).unwrap(),
                *bytes,
                "repair must preserve existing readers"
            );
            let (_, hit, _) = cached_rlib(&cfg, "cache_probe", key, &source, None).unwrap();
            assert!(hit, "a repaired archive should be reusable");
        }
        fs::write(&archive, []).unwrap();
        let barrier = std::sync::Barrier::new(4);
        let hits = std::thread::scope(|scope| {
            let workers: Vec<_> = (0..4)
                .map(|_| {
                    let barrier = &barrier;
                    let cfg = &cfg;
                    let source = &source;
                    scope.spawn(move || {
                        barrier.wait();
                        cached_rlib(cfg, "cache_probe", key, source, None)
                            .unwrap()
                            .1
                    })
                })
                .collect();
            workers
                .into_iter()
                .map(|worker| worker.join().unwrap())
                .filter(|hit| *hit)
                .count()
        });
        assert_eq!(hits, 3, "one population, three reusable results");
        fs::remove_dir_all(directory).unwrap();
    }
}
