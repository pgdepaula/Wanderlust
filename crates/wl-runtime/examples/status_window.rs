//! Native SDL monitor for a bounded, atomically replaced diagnostic text file.
//! Repaints of this host UI are never guest frames or graphics submissions.
use std::{io::Read, path::PathBuf, time::Duration};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let path = PathBuf::from(args.next().ok_or("usage: status_window STATUS.txt")?);
    if args.next().is_some() {
        return Err("usage: status_window STATUS.txt".into());
    }
    let mut window =
        wl_runtime::sdl::SdlHost::open("TOTK - Wanderlust | SDL2 | diagnostics", 960, 540)
            .ok_or("SDL2 window unavailable")?;
    while !window.should_close {
        window.poll_input();
        let mut text = String::new();
        let result =
            std::fs::File::open(&path).and_then(|f| f.take(16 * 1024).read_to_string(&mut text));
        let title = if result.is_ok() {
            let lines: Vec<_> = text.lines().take(17).collect();
            let summary = lines
                .iter()
                .skip(1)
                .filter(|line| {
                    line.contains("State:")
                        || line.contains("CPU units:")
                        || line.contains("backend:")
                        || line.contains("throughput:")
                        || line.contains("draws:")
                        || line.contains("targets:")
                        || line.contains("frames:")
                        || line.contains("RECOMPILING")
                        || line.contains("STOPPED")
                })
                .take(6)
                .copied()
                .collect::<Vec<_>>()
                .join(" | ");
            format!("TOTK - Wanderlust | SDL2 | {summary}")
        } else {
            "TOTK - Wanderlust | SDL2 | waiting for status".into()
        };
        window.set_title(&title.chars().take(300).collect::<String>());
        if !window.present_frame(&wl_runtime::display::DisplayServer::default()) {
            return Err("SDL2 presentation failed".into());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Ok(())
}
