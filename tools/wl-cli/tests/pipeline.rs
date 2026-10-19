use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};
static ID: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "wanderlust-test-{}-{}",
            std::process::id(),
            ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_wanderlust"))
}
fn raw(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}
fn compile(t: &Temp, words: &[u32]) -> PathBuf {
    let input = t.0.join("guest.a64");
    fs::write(&input, raw(words)).unwrap();
    let out = t.0.join(format!("guest{}", std::env::consts::EXE_SUFFIX));
    let result = cli()
        .args(["recompile"])
        .arg(&input)
        .args(["--raw", "--entry", "0x10000", "--out"])
        .arg(&out)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    out
}
fn state(output: &Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout).unwrap()
}
#[test]
fn help_and_bad_cli() {
    let o = cli().arg("--help").output().unwrap();
    assert!(o.status.success());
    assert!(String::from_utf8_lossy(&o.stdout).contains("recompile"));
    for args in [
        vec!["wat"],
        vec!["analyze", "input"],
        vec!["survey", "input", "--entry", "0x10000"],
        vec!["inspect", "input", "--target", "anything"],
    ] {
        assert!(!cli().args(args).output().unwrap().status.success());
    }
}
#[test]
fn compiles_and_executes_original_words() {
    let t = Temp::new();
    let exe = compile(
        &t,
        &[
            0xd28000e0, 0x91000c00, 0xd2a20002, 0xf9000040, 0xf9400043, 0xd65f03c0,
        ],
    );
    let dump = t.0.join("scratch");
    let o = Command::new(exe).arg("--dump").arg(&dump).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let s = state(&o);
    assert_eq!(s["x"][0], 10);
    assert_eq!(s["x"][3], 10);
    assert_eq!(s["steps"], 6);
    assert_eq!(&fs::read(dump).unwrap()[..8], &10u64.to_le_bytes());
}
#[test]
fn memory_fault_preserves_pc() {
    let t = Temp::new();
    let exe = compile(&t, &[0xf9400020, 0xd65f03c0]);
    let o = Command::new(exe).output().unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(state(&o)["pc"], 0x10000);
    assert!(String::from_utf8_lossy(&o.stderr).contains("unmapped guest memory"));
}
#[test]
fn unsupported_svc_is_an_explicit_fault() {
    let t = Temp::new();
    let exe = compile(&t, &[0xd4000021, 0xd65f03c0]);
    let o = Command::new(exe).output().unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("unsupported Horizon SVC 0x1"));
    assert_eq!(state(&o)["pc"], 0x10000);
}
#[test]
fn exit_process_has_no_fallthrough() {
    let t = Temp::new();
    let exe = compile(&t, &[0xd40000e1, 0xffffffff]);
    let o = Command::new(exe).output().unwrap();
    assert!(o.status.success());
    assert_eq!(state(&o)["status"], "exit_process");
}
#[test]
fn execution_budget_stops_real_native_loop() {
    let t = Temp::new();
    let exe = compile(&t, &[0x14000000]);
    let o = Command::new(exe).args(["--steps", "5"]).output().unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(state(&o)["steps"], 5);
    assert!(String::from_utf8_lossy(&o.stderr).contains("budget exhausted"));
}
#[test]
fn unknown_indirect_target_is_not_interpreted() {
    let t = Temp::new();
    let exe = compile(&t, &[0xd61f0000, 0xd65f03c0]);
    let o = Command::new(exe)
        .args(["--reg", "0=0x10004"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("unresolved native branch"));
}
#[test]
fn negative_translation_and_input_protection() {
    let t = Temp::new();
    let input = t.0.join("bad.a64");
    fs::write(&input, raw(&[0xffffffff])).unwrap();
    let o = cli()
        .arg("recompile")
        .arg(&input)
        .args(["--raw", "--entry", "0x10000", "--out"])
        .arg(t.0.join("bad"))
        .output()
        .unwrap();
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("opcode 0xffffffff"));
    fs::write(&input, raw(&[0xd65f03c0])).unwrap();
    let o = cli()
        .arg("recompile")
        .arg(&input)
        .args(["--raw", "--entry", "0x10000", "--out"])
        .arg(&input)
        .output()
        .unwrap();
    assert!(!o.status.success());
    assert_eq!(fs::read(&input).unwrap(), raw(&[0xd65f03c0]));
}
#[test]
fn compressed_nso_reconstructs_and_runs() {
    let t = Temp::new();
    let text = raw(&[0xd28000e0, 0xd65f03c0]);
    let stored = lz4_flex::block::compress(&text);
    let mut bytes = vec![0u8; 0x100];
    bytes[..4].copy_from_slice(b"NSO0");
    let put = |b: &mut [u8], at: usize, v: u32| b[at..at + 4].copy_from_slice(&v.to_le_bytes());
    put(&mut bytes, 0xc, 1);
    put(&mut bytes, 0x10, 0x100);
    put(&mut bytes, 0x18, 8);
    put(&mut bytes, 0x24, 0x1000);
    put(&mut bytes, 0x34, 0x2000);
    put(&mut bytes, 0x3c, 16);
    put(&mut bytes, 0x60, stored.len() as u32);
    bytes.extend(stored);
    let input = t.0.join("fixture.nso");
    fs::write(&input, bytes).unwrap();
    let inspected = cli()
        .arg("inspect")
        .arg(&input)
        .arg("--json")
        .output()
        .unwrap();
    assert!(inspected.status.success());
    assert_eq!(state(&inspected)["segments"][0]["compressed"], true);
    let exe = t.0.join(format!("nso{}", std::env::consts::EXE_SUFFIX));
    let o = cli()
        .arg("recompile")
        .arg(&input)
        .args(["--entry", "0x10000", "--out"])
        .arg(&exe)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let o = Command::new(exe).output().unwrap();
    assert!(o.status.success());
    assert_eq!(state(&o)["x"][0], 7);
}

#[test]
fn optimization_modes_preserve_guest_results_and_separate_cache_identity() {
    let t = Temp::new();
    let input = t.0.join("optimization.a64");
    // MOVZ X0,7; ADD X0,X0,3; MOVZ X2,#0x1000,LSL#16;
    // STR X0,[X2]; LDR X3,[X2]; RET. Independent expected result: 10.
    fs::write(
        &input,
        raw(&[
            0xd28000e0, 0x91000c00, 0xd2a20002, 0xf9000040, 0xf9400043, 0xd65f03c0,
        ]),
    )
    .unwrap();
    let output =
        t.0.join(format!("optimization{}", std::env::consts::EXE_SUFFIX));
    let mut previous = None;
    for (level, misses) in [("0", 2), ("0", 0), ("1", 2), ("0", 0)] {
        let build = cli()
            .arg("recompile")
            .arg(&input)
            .args(["--raw", "--entry", "0x10000", "--opt-level", level, "--out"])
            .arg(&output)
            .output()
            .unwrap();
        assert!(
            build.status.success(),
            "{}",
            String::from_utf8_lossy(&build.stderr)
        );
        let report = state(&build);
        assert_eq!(report["opt_level"], level.parse::<u64>().unwrap());
        assert_eq!(report["build"]["cache_misses"], misses);
        let run = Command::new(&output).output().unwrap();
        assert!(
            run.status.success(),
            "{}",
            String::from_utf8_lossy(&run.stderr)
        );
        let result = state(&run);
        assert_eq!(result["x"][0], 10);
        assert_eq!(result["x"][3], 10);
        assert_eq!(result["steps"], 6);
        if let Some(prior) = previous {
            assert_eq!(result, prior);
        }
        previous = Some(result);
    }
}
