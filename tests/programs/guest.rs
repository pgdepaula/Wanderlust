//! AArch64 guest build of kernel.rs: `wl_entry(seed) -> hash`, linked as a MOD0 module.
#![no_std]
include!("kernel.rs");
#[unsafe(no_mangle)]
pub extern "C" fn wl_entry(seed: u64) -> u64 {
    kernel(seed)
}
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
