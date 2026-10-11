//! Host oracle for kernel.rs: prints kernel(seed) for each seed argument.
include!("kernel.rs");
fn main() {
    for seed in std::env::args().skip(1) {
        let seed = u64::from_str_radix(seed.trim_start_matches("0x"), 16).expect("hex seed");
        println!("{:#x} {}", seed, kernel(std::hint::black_box(seed)));
    }
}
