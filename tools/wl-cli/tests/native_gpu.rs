//! Executed guest AArch64 service calls, not host-constructed ioctl messages.
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "wanderlust-native-gpu-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}
#[test]
fn native_gpu_requires_a_horizon_process() {
    let result = Command::new(env!("CARGO_BIN_EXE_wanderlust"))
        .args([
            "recompile",
            "missing-input",
            "--native-gpu",
            "--out",
            "unused",
        ])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("--native-gpu requires --horizon"));
}
#[test]
#[ignore = "compiles and executes actual AArch64 NVIDIA requests on a host GPU"]
fn guest_aarch64_nvidia_draws_through_the_generated_executable() {
    let t = Temp::new();
    let output = t.0.join(format!("guest{}", std::env::consts::EXE_SUFFIX));
    let mut first: Option<serde_json::Value> = None;
    for warm in [false, true] {
        let built = Command::new(env!("CARGO_BIN_EXE_wanderlust"))
            .arg("recompile")
            .arg(root().join("tests/horizon/nvidia"))
            .args([
                "--horizon",
                "--allow-unsupported",
                "--native-gpu",
                "--jobs",
                "1",
                "--out",
            ])
            .arg(&output)
            .output()
            .unwrap();
        assert!(
            built.status.success(),
            "{}",
            String::from_utf8_lossy(&built.stderr)
        );
        let report: serde_json::Value = serde_json::from_slice(&built.stdout).unwrap();
        assert!(report["build"]["native_gpu"]["artifacts"].as_u64().unwrap() > 10);
        if warm {
            assert_eq!(report["build"]["cache_misses"], 0);
            assert_eq!(report["build"]["native_gpu"]["reused"], true);
            assert_eq!(
                report["build"]["native_gpu"]["key"],
                first.as_ref().unwrap()["build"]["native_gpu"]["key"]
            );
        }
        let ran = Command::new(&output)
            .env("WL_NO_SDL", "1")
            .args(["--steps", "3000000"])
            .output()
            .unwrap();
        assert!(
            ran.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&ran.stdout),
            String::from_utf8_lossy(&ran.stderr)
        );
        eprintln!("{}", String::from_utf8_lossy(&ran.stderr));
        let result: serde_json::Value = serde_json::from_slice(&ran.stdout).unwrap();
        assert_eq!(result["status"], "exit_process", "{result}");
        assert!(result["fault"].is_null(), "{result}");
        let debug = result["debug_output"].as_array().unwrap();
        assert_eq!(debug.len(), 3, "{debug:?}");
        assert!(
            debug[0]
                .as_str()
                .unwrap()
                .starts_with("nvidia frame=1 inside=1 outside=1 fence=")
        );
        assert!(debug[0].as_str().unwrap().ends_with("threshold=2"));
        assert!(
            debug[1]
                .as_str()
                .unwrap()
                .starts_with("nvidia frame=2 inside=1 outside=1 fence=")
        );
        assert!(debug[1].as_str().unwrap().ends_with("threshold=4"));
        assert_eq!(debug[2], "nvidia completed two guest-issued indexed draws");
        eprintln!("native AOT GPU build warm={warm} {}", report["build"]);
        first = Some(report);
    }
}
