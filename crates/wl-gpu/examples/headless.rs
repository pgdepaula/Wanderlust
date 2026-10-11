//! Execute and measure controlled indexed rendering; never consumes game assets.
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};
use std::alloc::System;
use std::time::Instant;
#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;
use wl_gpu::{Counters, backend::Renderer, command::*};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut renderer = Renderer::new()?;
    println!("capabilities={:?}", renderer.capabilities);
    let (vertices, indices) = test_geometry();
    let draw = Command::DrawIndexed {
        vertex: BufferId(1),
        index: BufferId(2),
        index_count: 12,
    };
    let create = [
        Command::CreateBuffer {
            id: BufferId(1),
            size: vertices.len() as u64,
            kind: BufferKind::Vertex,
        },
        Command::CreateBuffer {
            id: BufferId(2),
            size: indices.len() as u64,
            kind: BufferKind::Index,
        },
    ];
    let uploads = [
        Command::Upload {
            id: BufferId(1),
            offset: 0,
            data: &vertices,
        },
        Command::Upload {
            id: BufferId(2),
            offset: 0,
            data: &indices,
        },
    ];
    let initial = [
        create[0].clone(),
        create[1].clone(),
        uploads[0].clone(),
        uploads[1].clone(),
        draw.clone(),
    ];
    renderer
        .execute(66, 19, &initial)?
        .validate_test_pattern()?;
    println!("warmup={:?}", renderer.counters);
    let frames = 32;
    for mode in [
        "recreate_full",
        "reuse_full",
        "reuse_dirty",
        "reuse_unchanged",
    ] {
        renderer.counters = Counters::default();
        let allocations = Region::new(GLOBAL);
        let start = Instant::now();
        let mut encode_ns = 0_u128;
        let mut gpu_ns = 0.0;
        let mut timestamp_frames = 0;
        for _ in 0..frames {
            let cmds = match mode {
                "recreate_full" => vec![
                    Command::DestroyBuffer { id: BufferId(1) },
                    Command::DestroyBuffer { id: BufferId(2) },
                    create[0].clone(),
                    create[1].clone(),
                    uploads[0].clone(),
                    uploads[1].clone(),
                    draw.clone(),
                ],
                "reuse_full" => vec![uploads[0].clone(), uploads[1].clone(), draw.clone()],
                "reuse_dirty" => vec![
                    Command::Upload {
                        id: BufferId(1),
                        offset: 8,
                        data: &vertices[8..12],
                    },
                    draw.clone(),
                ],
                _ => vec![draw.clone()],
            };
            let frame = renderer.execute(66, 19, &cmds)?;
            frame.validate_test_pattern()?;
            encode_ns += frame.encode_time.as_nanos();
            if let Some(ns) = frame.gpu_nanoseconds {
                gpu_ns += ns;
                timestamp_frames += 1;
            }
        }
        let heap = allocations.change();
        println!("rust_heap={heap:?}");
        println!(
            "mode={mode} frames={frames} wall_us={} cpu_encode_us={} gpu_ns={:?} counters={:?}",
            start.elapsed().as_micros(),
            encode_ns / 1000,
            (timestamp_frames > 0).then_some(gpu_ns / f64::from(timestamp_frames)),
            renderer.counters
        );
    }
    Ok(())
}
