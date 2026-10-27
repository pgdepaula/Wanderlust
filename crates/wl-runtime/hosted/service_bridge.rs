//! Typed SVC extension boundary, compiled against the exact selected runtime.
//! Concrete handlers stay in the executable; unchanged CPU units retain their ABI.
use std::sync::OnceLock;
use wl_guest_runtime::{Cpu, Process, RuntimeResult, Stop};

pub type Handler = fn(&mut Cpu, &mut Process, u16) -> RuntimeResult<Option<Stop>>;
static HANDLER: OnceLock<Handler> = OnceLock::new();

pub fn install(handler: Handler) -> RuntimeResult<()> {
    let current = HANDLER.get_or_init(|| handler);
    if std::ptr::fn_addr_eq(*current, handler) {
        Ok(())
    } else {
        Err("a different native SVC handler is already installed".into())
    }
}

pub fn service(cpu: &mut Cpu, process: &mut Process, id: u16) -> RuntimeResult<Option<Stop>> {
    match HANDLER.get() {
        Some(handler) => handler(cpu, process, id),
        None => wl_guest_runtime::service(cpu, process, id),
    }
}
