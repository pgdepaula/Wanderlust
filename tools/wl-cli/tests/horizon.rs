//! A real libnx application (tests/horizon/probe.c, built with devkitA64 and libnx into the
//! NSO and NPDM in tests/horizon/probe/) runs through the complete pipeline: parse, load,
//! analysis, AOT compilation, native execution against the Horizon process environment, and
//! exit through svcExitProcess.
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};
static ID: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "wanderlust-horizon-{}-{}",
            std::process::id(),
            ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
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
fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_wanderlust"))
}
/// Lines the probe reports through svcOutputDebugString, derived independently:
/// handle = index 1022 (top of the NPDM's 1023-entry table) | linear id 1 << 15; TLS value 0x5a5a + fib(15) = 0x5cbc;
/// a GetInfo with subtype 1 fails with InvalidCombination (0xe801) and zero output; the
/// checksum of 4096 values i * 2654435761 is 0x952e8000; ratio = 1 MiB stack / 512 MiB.
const EXPECTED: [&str; 3] = [
    "probe tid=0x72 pid=0x51 handle=0x83fe tls=0x5cbc getinfo_bad_subtype=0xe801 out=0",
    "text state=3 perm=5 size=0x10000; stack state=11 perm=3 size=0x100000 in_stack_region=1",
    "heap_ok=1 sum=952e8000 ratio=0.001953 used_below_total=1",
];
fn run(input: &Path, t: &Temp) -> serde_json::Value {
    let out = t.0.join(format!("native{}", std::env::consts::EXE_SUFFIX));
    let build = cli()
        .arg("recompile")
        .arg(input)
        .args(["--horizon", "--allow-unsupported", "--out"])
        .arg(&out)
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&build.stdout).unwrap();
    assert!(report["jump_tables"].as_u64().unwrap() > 0, "{report}");
    let run = Command::new(&out).output().unwrap();
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(
        run.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    serde_json::from_str(stdout.trim()).unwrap()
}
fn check(state: &serde_json::Value) {
    assert_eq!(state["mode"], "horizon");
    assert_eq!(state["status"], "exit_process", "{state}");
    assert_eq!(state["fault"], serde_json::Value::Null);
    assert_eq!(state["config_source"], "main.npdm");
    let lines: Vec<&str> = state["debug_output"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l.as_str().unwrap())
        .collect();
    assert_eq!(lines, EXPECTED);
    let svc = &state["svc_counts"];
    // Startup: RELRO reprotect + lock, GetInfo, heap, stack query; then the probe's own calls.
    for (id, count) in [
        ("0x01", 1),
        ("0x02", 1),
        ("0x03", 1),
        ("0x07", 1),
        ("0x24", 1),
        ("0x25", 1),
        ("0x27", 3),
    ] {
        assert_eq!(svc[id], count, "svc {id}: {svc}");
    }
    assert_eq!(state["thread"]["id"], 0x72);
    assert_eq!(state["heap"][1], 0x1fc0_0000);
}

#[test]
fn libnx_application_runs_natively_under_horizon() {
    let t = Temp::new();
    check(&run(&root().join("tests/horizon/probe"), &t));
}

/// Rebuilds the fixture from source when devkitA64 and libnx are available
/// (`DEVKITPRO` or target/toolchains/devkitpro); otherwise reports the skip.
#[test]
fn rebuilt_fixtures_match_the_committed_behaviour() {
    let local = root().join("target/toolchains/devkitpro/root/opt/devkitpro");
    let dkp = std::env::var_os("DEVKITPRO")
        .map(PathBuf::from)
        .unwrap_or(local);
    if !dkp.join("devkitA64/bin/aarch64-none-elf-gcc").is_file() {
        eprintln!("skipped: devkitA64 not found at {}", dkp.display());
        return;
    }
    let t = Temp::new();
    let dir = t.0.join("probe");
    let python = std::env::var("WL_PYTHON").unwrap_or_else(|_| "python3".into());
    let o = Command::new(python)
        .arg(root().join("tools/python/homebrew.py"))
        .arg(root().join("tests/horizon/probe.c"))
        .arg("--out-dir")
        .arg(&dir)
        .env("DEVKITPRO", &dkp)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    check(&run(&dir, &t));
    let dir = t.0.join("threads");
    let python = std::env::var("WL_PYTHON").unwrap_or_else(|_| "python3".into());
    let o = Command::new(python)
        .arg(root().join("tools/python/homebrew.py"))
        .arg(root().join("tests/horizon/threads.c"))
        .arg("--out-dir")
        .arg(&dir)
        .env("DEVKITPRO", &dkp)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    check_threads(&run(&dir, &t));
    let dir = t.0.join("ipc");
    let python = std::env::var("WL_PYTHON").unwrap_or_else(|_| "python3".into());
    let o = Command::new(python)
        .arg(root().join("tools/python/homebrew.py"))
        .arg(root().join("tests/horizon/ipc.c"))
        .arg("--out-dir")
        .arg(&dir)
        .env("DEVKITPRO", &dkp)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    check_ipc(&run(&dir, &t));
}

/// tests/horizon/threads.c: a worker created with libnx threadCreate on core 1, a contended
/// libnx Mutex, a CondVar, a timed svcWaitSynchronization and threadWaitForExit. Expected
/// values: thread IDs 0x72/0x73; TLS slots 1 and 2 of the first ThreadLocal page at
/// 0x8204000; total = sum over both threads and i < 3000 of the 24 byte windows of
/// mix(who << 32 | i) = 0x1180eea (computed independently of the runtime); each thread's
/// __thread counter 7 + 3000; the 100 us wait on the sleeping worker times out (0xea01,
/// index -1); joining and closing succeed.
const THREADS_EXPECTED: [&str; 3] = [
    "threads create=0 start=0 main_tid=0x72 worker_tid=0x73 tls_main=0x8204200 tls_worker=0x8204400",
    "sync total=0000000001180eea entries=3000,3000 tls_values=3007,3007 ready=1",
    "wait timed=0xea01 index=-1 joined=0 closed=0",
];
fn check_threads(state: &serde_json::Value) {
    assert_eq!(state["status"], "exit_process", "{state}");
    let lines: Vec<&str> = state["debug_output"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l.as_str().unwrap())
        .collect();
    assert_eq!(lines, THREADS_EXPECTED);
    let svc = &state["svc_counts"];
    for id in [
        "0x04", "0x05", "0x08", "0x09", "0x0a", "0x0b", "0x1c", "0x1d",
    ] {
        assert!(svc[id].as_u64().unwrap() >= 1, "svc {id}: {svc}");
    }
    // Preemption inside the critical section made the mutex contended.
    assert!(
        svc["0x1a"].as_u64().unwrap() >= 1 && svc["0x1b"].as_u64().unwrap() >= 1,
        "{svc}"
    );
    let sched = &state["scheduler"];
    assert_eq!(sched["max_runnable"], 2);
    assert!(sched["context_switches"].as_u64().unwrap() > 10);
    assert!(
        sched["idle_ns"].as_u64().unwrap() > 0,
        "main blocked while the worker slept"
    );
    let threads = state["threads"].as_array().unwrap();
    assert_eq!(threads.len(), 2);
    assert_eq!(threads[1]["state"], "Exited");
    assert_eq!(threads[1]["destroyed"], true, "closed and released");
}

#[test]
fn libnx_threads_synchronize_natively() {
    let t = Temp::new();
    let state = run(&root().join("tests/horizon/threads"), &t);
    check_threads(&state);
    // The scheduler is deterministic: the committed fixture always retires this count.
    assert_eq!(state["steps"], 1_041_282);
}

/// tests/horizon/ipc.c against the runtime's sm: server. Expected results (kernel results
/// are 1 | description << 9, sm results 21 | description << 9): InvalidServiceName 0xc15 (libnx
/// leaves the out-handle untouched on failure), AlreadyRegistered 0x815, OutOfSessions 0x615
/// (one session allowed), NotRegistered 0xe15, the deferred lookup completing once the service
/// is registered, NotFound 0xf201, OutOfRange 0xee01 (12 characters, no terminator),
/// SessionClosed 0xf601 after an unknown command, and InvalidHandle 0xe401 for the session
/// closed by smExit.
const IPC_EXPECTED: [&str; 3] = [
    "sm init=0 bad=0xc15 bad_handle=0x1234 reg=0 port=1 dup=0x815 get=0 client=1 full=0x615",
    "sm close_client=0 unreg=0 unreg2=0xe15 close_port=0 late_reg=0 late=0 late_handle=1",
    "port conn=0 notfound=0xf201 toolong=0xee01 unknown=0xf601 again=0xf601 stale=0xe401",
];
fn check_ipc(state: &serde_json::Value) {
    assert_eq!(state["status"], "exit_process", "{state}");
    let lines: Vec<&str> = state["debug_output"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l.as_str().unwrap())
        .collect();
    assert_eq!(lines, IPC_EXPECTED);
    let ipc = &state["ipc"];
    assert_eq!(ipc["deferred"], 1, "{ipc}");
    // The unknown command and the three CMIF Close requests (two smExit-style closes and
    // the worker's serviceClose) closed sessions on the server side.
    assert!(
        ipc["sessions_closed_by_server"].as_u64().unwrap() >= 3,
        "{ipc}"
    );
    assert_eq!(ipc["services"], serde_json::json!(["wl:late"]));
}

#[test]
fn libnx_sm_client_exchanges_cmif_messages() {
    let t = Temp::new();
    check_ipc(&run(&root().join("tests/horizon/ipc"), &t));
}

#[test]
fn horizon_mode_requires_an_exefs_directory() {
    let t = Temp::new();
    let o = cli()
        .arg("recompile")
        .arg(root().join("tests/horizon/probe/main"))
        .args(["--horizon", "--out"])
        .arg(t.0.join("x"))
        .output()
        .unwrap();
    assert!(!o.status.success());
}
