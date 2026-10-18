//! Target-specific Cargo acquisition for the GPU-enabled final AOT executable.
//! Only Cargo-selected artifacts enter an owned immutable dependency snapshot.
use super::{BuildConfig, hash, rustc};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::Instant,
};
use wl_common::Result;

pub(crate) const ADAPTER: &str = include_str!("../../wl-nvdrv/src/runtime_adapter.rs");
static NEXT: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, serde::Serialize)]
pub struct NativeGpuReport {
    pub target: String,
    pub key: String,
    pub artifacts: usize,
    pub snapshot_bytes: u64,
    pub reused: bool,
    pub seconds: f64,
}
pub(super) struct Prepared {
    directory: PathBuf,
    gpu: String,
    driver: String,
    manifest: BTreeMap<String, String>,
    pub report: NativeGpuReport,
}
impl Prepared {
    pub fn verify(&self) -> Result<()> {
        for (name, digest) in &self.manifest {
            if copy_digest(&self.directory.join(name), None)?.0 != *digest {
                return Err("GPU snapshot changed before executable publication".into());
            }
        }
        Ok(())
    }
    pub fn link(&self, command: &mut Command) {
        command
            .arg("-L")
            .arg(format!("dependency={}", self.directory.display()));
        command
            .arg("-L")
            .arg(format!("native={}", self.directory.display()));
        for (name, file) in [("wl_gpu", &self.gpu), ("wl_nvdrv", &self.driver)] {
            command
                .arg("--extern")
                .arg(format!("{name}={}", self.directory.join(file).display()));
        }
    }
}

pub(super) fn temporary_output(output: &Path) -> PathBuf {
    let mut name = output.as_os_str().to_owned();
    name.push(format!(
        ".native-link-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    PathBuf::from(name)
}

fn copy_digest(source: &Path, output: Option<&Path>) -> Result<(String, u64)> {
    let mut input = fs::File::open(source).map_err(|e| format!("{}: {e}", source.display()))?;
    let mut output = output
        .map(fs::File::create)
        .transpose()
        .map_err(|e| e.to_string())?;
    let mut digest = Sha256::new();
    let mut size = 0u64;
    let mut buffer = [0u8; 65536];
    loop {
        let n = input.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        size = size
            .checked_add(n as u64)
            .ok_or("GPU artifact size overflow")?;
        if size > 512 * 1024 * 1024 {
            return Err("GPU artifact exceeds 512 MiB".into());
        }
        digest.update(&buffer[..n]);
        if let Some(output) = &mut output {
            output.write_all(&buffer[..n]).map_err(|e| e.to_string())?;
        }
    }
    Ok((format!("{:x}", digest.finalize()), size))
}

pub(super) fn prepare(workspace: &Path, cfg: &BuildConfig, toolchain: &str) -> Result<Prepared> {
    let started = Instant::now();
    let workspace = fs::canonicalize(workspace).map_err(|e| format!("GPU workspace: {e}"))?;
    if fs::read_to_string(workspace.join("crates/wl-nvdrv/src/runtime_adapter.rs"))
        .map_err(|e| e.to_string())?
        != ADAPTER
    {
        return Err(
            "GPU workspace adapter differs from this code generator; rebuild the CLI".into(),
        );
    }
    let lock = fs::read(workspace.join("Cargo.lock")).map_err(|e| e.to_string())?;
    let target = cfg
        .target
        .clone()
        .or_else(|| {
            toolchain
                .lines()
                .find_map(|s| s.strip_prefix("host: ").map(str::to_owned))
        })
        .ok_or("rustc did not report its host target")?;
    let sysroot = rustc()
        .args(["--print", "sysroot"])
        .output()
        .map_err(|e| e.to_string())?;
    if !sysroot.status.success() {
        return Err("rustc sysroot query failed".into());
    }
    let compiler = PathBuf::from(
        String::from_utf8(sysroot.stdout)
            .map_err(|e| e.to_string())?
            .trim(),
    )
    .join("bin")
    .join(format!("rustc{}", std::env::consts::EXE_SUFFIX));
    let mut cargo = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    // A nested build must not wait on the outer `cargo test` build-directory
    // lock. Keep GPU dependency compilation in its own reusable target tree.
    let gpu_target = std::env::var_os("WL_GPU_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace.join("target/native-gpu"));
    cargo.arg("build").arg("--target-dir").arg(gpu_target);
    cargo.current_dir(&workspace).env("RUSTC", compiler).args([
        "-p",
        "wl-nvdrv",
        "--lib",
        "--locked",
        "--target",
        &target,
        "--message-format=json",
    ]);
    let result = cargo
        .output()
        .map_err(|e| format!("GPU Cargo build: {e}"))?;
    if !result.status.success() {
        return Err(format!(
            "GPU Cargo build failed: {}\n{}",
            String::from_utf8_lossy(&result.stderr),
            String::from_utf8_lossy(&result.stdout)
        ));
    }
    let mut files = BTreeMap::<String, PathBuf>::new();
    let (mut gpu, mut driver) = (None, None);
    for line in result
        .stdout
        .split(|v| *v == b'\n')
        .filter(|v| v.first() == Some(&b'{'))
    {
        let value: serde_json::Value = serde_json::from_slice(line).map_err(|e| e.to_string())?;
        if value["reason"] == "build-script-executed" {
            for entry in value["linked_paths"].as_array().into_iter().flatten() {
                let entry = entry.as_str().ok_or("invalid native search path")?;
                let directory = if let Some((kind, path)) = entry.split_once('=') {
                    if kind != "native" {
                        return Err("GPU dependency requires unsupported native search kind".into());
                    }
                    PathBuf::from(path)
                } else {
                    PathBuf::from(entry)
                };
                // windows-targets supplies an import archive outside its rlib.
                // Snapshot explicit static/import archives, never retain a live
                // Cargo build-script directory as a final linker search path.
                for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
                    let path = entry.map_err(|e| e.to_string())?.path();
                    if path.is_file() && path.extension().is_some_and(|v| v == "a" || v == "lib") {
                        let name = path
                            .file_name()
                            .and_then(|v| v.to_str())
                            .ok_or("native archive name")?
                            .to_owned();
                        if let Some(old) = files.insert(name, path.clone()) {
                            if old != path {
                                return Err("conflicting native archive filenames".into());
                            }
                        }
                    }
                }
            }
            continue;
        }
        if value["reason"] != "compiler-artifact" {
            continue;
        }
        for file in value["filenames"]
            .as_array()
            .ok_or("Cargo artifact lacks filenames")?
        {
            let path = PathBuf::from(file.as_str().ok_or("invalid Cargo artifact path")?);
            // Rust metadata also references host proc-macro crates. Cross builds
            // need the Cargo-selected HOST dylib as well as TARGET rlibs.
            let proc_macro = value["target"]["kind"]
                .as_array()
                .is_some_and(|v| v.iter().any(|v| v == "proc-macro"));
            let dependency = path.extension().is_some_and(|ext| {
                ext == "rlib"
                    || (proc_macro && matches!(ext.to_str(), Some("so" | "dylib" | "dll")))
            });
            if !dependency {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|v| v.to_str())
                .ok_or("GPU artifact filename")?
                .to_owned();
            if let Some(old) = files.insert(name.clone(), path.clone()) {
                if old != path {
                    return Err("conflicting Cargo artifact filenames".into());
                }
            }
            if value["target"]["name"] == "wl_gpu" {
                if !value["features"]
                    .as_array()
                    .is_some_and(|v| v.iter().any(|v| v == "wgpu-backend"))
                {
                    return Err("selected GPU artifact lacks wgpu-backend".into());
                }
                gpu = Some(name.clone());
            }
            if value["target"]["name"] == "wl_nvdrv" {
                driver = Some(name);
            }
        }
    }
    let gpu = gpu.ok_or("Cargo did not produce the GPU library")?;
    let driver = driver.ok_or("Cargo did not produce the NVIDIA adapter library")?;
    if files.len() > 512 {
        return Err("GPU dependency artifact budget exceeded".into());
    }
    let mut manifest = BTreeMap::new();
    let mut bytes = 0u64;
    for (name, path) in &files {
        let (digest, size) = copy_digest(path, None)?;
        bytes = bytes
            .checked_add(size)
            .filter(|s| *s <= 2 * 1024 * 1024 * 1024)
            .ok_or("GPU snapshot exceeds 2 GiB")?;
        manifest.insert(name.clone(), digest);
    }
    let manifest_text = serde_json::to_vec(&manifest).map_err(|e| e.to_string())?;
    let key = hash(&[
        b"native-gpu-v2",
        toolchain.as_bytes(),
        target.as_bytes(),
        &lock,
        ADAPTER.as_bytes(),
        &manifest_text,
    ]);
    let cache = fs::canonicalize(&cfg.cache_dir).map_err(|e| e.to_string())?;
    let directory = cache.join(format!("native-gpu-{key}"));
    let reused = directory.is_dir();
    if !reused {
        let temporary = cache.join(format!(
            ".native-gpu-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&temporary).map_err(|e| e.to_string())?;
        for (name, path) in &files {
            let (copied, _) = copy_digest(path, Some(&temporary.join(name)))?;
            if copied != manifest[name] {
                return Err("GPU artifact changed during snapshot acquisition".into());
            }
        }
        fs::write(temporary.join("manifest.json"), &manifest_text).map_err(|e| e.to_string())?;
        if fs::rename(&temporary, &directory).is_err() {
            if !directory.is_dir() {
                return Err("cannot publish GPU artifact snapshot".into());
            }
            fs::remove_dir_all(&temporary).map_err(|e| e.to_string())?;
        }
    }
    for (name, digest) in &manifest {
        if copy_digest(&directory.join(name), None)?.0 != *digest {
            return Err("GPU artifact snapshot integrity failure".into());
        }
    }
    if fs::read(workspace.join("Cargo.lock")).map_err(|e| e.to_string())? != lock {
        return Err("GPU lockfile changed during build".into());
    }
    Ok(Prepared {
        directory,
        gpu,
        driver,
        manifest,
        report: NativeGpuReport {
            target,
            key,
            artifacts: files.len(),
            snapshot_bytes: bytes,
            reused,
            seconds: started.elapsed().as_secs_f64(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn copied_artifact_is_owned_and_changes_cannot_hide_behind_a_replaced_manifest() {
        let path = std::env::temp_dir().join(format!(
            "wl-gpu-artifact-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        let source = path.join("source");
        let copied = path.join("owned.rlib");
        fs::write(&source, b"abc").unwrap();
        let (digest, size) = copy_digest(&source, Some(&copied)).unwrap();
        assert_eq!(size, 3);
        assert_eq!(
            digest,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        fs::write(&source, b"a different Cargo rebuild").unwrap();
        assert_eq!(fs::read(&copied).unwrap(), b"abc");
        let prepared = Prepared {
            directory: path.clone(),
            gpu: "owned.rlib".into(),
            driver: "owned.rlib".into(),
            manifest: BTreeMap::from([("owned.rlib".into(), digest)]),
            report: NativeGpuReport {
                target: "test".into(),
                key: "test".into(),
                artifacts: 1,
                snapshot_bytes: 3,
                reused: false,
                seconds: 0.,
            },
        };
        prepared.verify().unwrap();
        fs::write(copied, b"corruption").unwrap();
        fs::write(path.join("manifest.json"), b"{}").unwrap();
        assert!(prepared.verify().unwrap_err().contains("snapshot changed"));
        fs::remove_dir_all(path).unwrap();
    }
}
