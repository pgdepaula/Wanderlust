use std::{fs, process::Command};

#[test]
#[ignore = "requires devkitA64 and libnx; builds and executes the AArch64 cache-reader client"]
fn libnx_cache_save_reader_uses_real_domain_objects_and_preserves_output() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir =
        std::env::temp_dir().join(format!("wanderlust-native-fscache-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let fixture = dir.join("exefs");
    let build = Command::new("python3")
        .arg(root.join("tools/python/homebrew.py"))
        .arg(root.join("tests/horizon/fs_cache.c"))
        .arg("--out-dir")
        .arg(&fixture)
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let exe = dir.join(format!("fscache{}", std::env::consts::EXE_SUFFIX));
    let compiled = Command::new(env!("CARGO_BIN_EXE_wanderlust"))
        .arg("recompile")
        .arg(&fixture)
        .args([
            "--horizon",
            "--allow-unsupported",
            "--memory-limit",
            "0x20000000",
            "--out",
        ])
        .arg(&exe)
        .output()
        .unwrap();
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let run = Command::new(exe)
        .env("WL_NO_SDL", "1")
        .env("WL_THREAD_TRACE", "1")
        .args(["--steps", "2000000"])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&run.stdout).unwrap();
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(stderr.contains("[thread-state] id=114"));
    assert_eq!(result["status"], "exit_process");
    assert!(
        result["debug_output"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s.as_str() == Some("FSCACHE_NATIVE_OK"))
    );
    fs::remove_dir_all(dir).unwrap();
}
