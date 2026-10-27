use std::{fs, process::Command};
use wl_analysis::{Image, discover};
use wl_codegen_rust::{EmitOptions, HorizonOptions, build, emit};
use wl_common::GuestAddr;

#[test]
fn native_application_thread_budget_counts_live_threads_and_keeps_a_hard_limit() {
    let dir = std::env::temp_dir().join(format!("wanderlust-thread-budget-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    // GNU as 2.46: MOV W19,#count; ADR X1,worker; MOV X2,XZR;
    // MOV X3,SP; MOV W4,#44; MOV W5,WZR; SVC #8; CBNZ W0,fail;
    // SUBS W19,W19,#1; B.NE loop; SVC #7; fail: SVC #0x26; worker: RET.
    // Threads stay initialized, so every successful creation holds a resource.
    let template = [
        0x52800513u32,
        0x10000161,
        0xaa1f03e2,
        0x910003e3,
        0x52800584,
        0x2a1f03e5,
        0xd4000101,
        0x35000080,
        0x71000673,
        0x54ffff01,
        0xd40000e1,
        0xd40004c1,
        0xd65f03c0,
    ];
    for (name, application, count, limit, success) in [
        ("application", true, 95, 96, true),
        ("application-full", true, 96, 96, false),
        ("standalone", false, 32, 32, false),
    ] {
        let mut words = template;
        words[0] = 0x52800013 | count << 5;
        let original: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let image = Image::raw(original.clone(), GuestAddr(0x8000000)).unwrap();
        let program = discover(&image, GuestAddr(0x8000000), &[GuestAddr(0x8000030)]).unwrap();
        let generated = emit(
            &image,
            &program,
            "guest.rs",
            &[],
            EmitOptions {
                unit_shift: 12,
                horizon: Some(HorizonOptions {
                    code_start: 0x8000000,
                    code_end: 0x8001000,
                    program_id: 1,
                    main_thread_stack_size: 0x100000,
                    main_thread_priority: 44,
                    ideal_core: 0,
                    core_mask: 7,
                    priority_mask: u64::MAX,
                    handle_table_size: 512,
                    is_application: application,
                    svc_permissions: [u64::MAX; 3],
                    service_access: vec![],
                    memory_limit: 512 * 1024 * 1024,
                    system_resource_size: 0,
                    source: "native-thread-fixture",
                    homebrew: false,
                }),
            },
        )
        .unwrap();
        assert!(generated.main.contains(&format!("thread_limit: {limit},")));
        let main = dir.join(format!("{name}.rs"));
        fs::write(&main, &generated.main).unwrap();
        for (filename, data) in &generated.blobs {
            fs::write(dir.join(filename), data).unwrap();
        }
        let exe = dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        build::build(
            &generated,
            &main,
            &exe,
            &build::BuildConfig {
                target: None,
                cache_dir: dir.join("cache"),
                jobs: 1,
                opt_level: 0,
            },
        )
        .unwrap();
        let run = Command::new(exe).output().unwrap();
        assert_eq!(run.status.success(), success, "{name}: {run:?}");
        let result: serde_json::Value = serde_json::from_slice(&run.stdout).unwrap();
        assert_eq!(result["threads"].as_array().unwrap().len(), limit);
        assert_eq!(result["svc_counts"]["0x08"], count);
        if success {
            assert_eq!(result["status"], "exit_process");
            assert!(result["fault"].is_null());
        } else {
            assert_eq!(result["status"], "fault");
            assert!(String::from_utf8_lossy(&run.stderr).contains("0x10801"));
        }
        assert_eq!(image.mappings[0].bytes, original);
    }
    fs::remove_dir_all(dir).unwrap();
}
