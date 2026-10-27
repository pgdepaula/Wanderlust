//! Audio host callbacks. Concrete service code is linked with the launcher, not
//! the AOT runtime archive, so service-only changes retain CPU unit cache identity.
use crate::{Memory, RuntimeResult, horizon::Kernel, ipc::Request, services::HostObject};
use std::sync::OnceLock;
#[derive(Debug, Default)]
pub struct Reply {
    pub code: u32,
    pub data: Vec<u8>,
    pub objects: Vec<HostObject>,
    pub copies: Vec<u32>,
}
impl Reply {
    pub fn ok() -> Self {
        Self::default()
    }
    pub fn data(data: &[u8]) -> Self {
        Self {
            data: data.to_vec(),
            ..Self::ok()
        }
    }
    pub fn object(object: HostObject) -> Self {
        Self {
            objects: vec![object],
            ..Self::ok()
        }
    }
    pub fn failure(code: u32) -> Self {
        Self { code, ..Self::ok() }
    }
}
pub trait AudioHost: std::fmt::Debug + Send {
    fn request(
        &mut self,
        memory: &mut Memory,
        kernel: &mut Kernel,
        object: HostObject,
        command: u32,
        request: &Request,
        args: &[u8],
    ) -> RuntimeResult<Reply>;
    fn poll(&mut self, memory: &mut Memory, kernel: &mut Kernel) -> RuntimeResult<()>;
    fn next_deadline_ns(&self) -> Option<u64>;
    fn close(&mut self, kernel: &mut Kernel, object: HostObject);
}
pub struct Factory {
    pub create: fn() -> Box<dyn AudioHost>,
}
static FACTORY: OnceLock<&'static Factory> = OnceLock::new();
/// Install before process creation. Reinstalling the same static factory is harmless.
pub fn install(factory: &'static Factory) -> RuntimeResult<()> {
    let installed = FACTORY.get_or_init(|| factory);
    if std::ptr::eq(*installed, factory) {
        Ok(())
    } else {
        Err("a different audio host factory is already installed".into())
    }
}
pub fn available() -> bool {
    FACTORY.get().is_some()
}
pub(crate) fn create() -> Option<Box<dyn AudioHost>> {
    FACTORY.get().map(|f| (f.create)())
}
pub(crate) fn request(
    memory: &mut Memory,
    kernel: &mut Kernel,
    object: HostObject,
    command: u32,
    request: &Request,
    args: &[u8],
) -> RuntimeResult<Reply> {
    let mut host = kernel
        .audio
        .take()
        .ok_or("audio host absent or reentered")?;
    let result = host.request(memory, kernel, object, command, request, args);
    kernel.audio = Some(host);
    result
}
pub fn poll(memory: &mut Memory, kernel: &mut Kernel) -> RuntimeResult<()> {
    if let Some(mut host) = kernel.audio.take() {
        let result = host.poll(memory, kernel);
        kernel.audio = Some(host);
        result
    } else {
        Ok(())
    }
}
pub fn next_deadline_tick(kernel: &Kernel) -> Option<u64> {
    kernel
        .audio
        .as_ref()
        .and_then(|h| h.next_deadline_ns())
        .map(|ns| {
            (u128::from(ns) * 192)
                .div_ceil(10_000)
                .min(u128::from(u64::MAX)) as u64
        })
}

pub(crate) fn release_if_unreferenced(kernel: &mut Kernel, object: HostObject) {
    if !matches!(object, HostObject::AudioRenderer { .. }) {
        return;
    }
    let referenced = kernel.sessions.iter().any(|session| {
        session.references != 0
            && !session.closed
            && ((session.domain.is_none()
                && session.endpoint == crate::ipc::Endpoint::Host(object))
                || session.domain.is_some_and(|d| {
                    kernel.host.domains[d]
                        .objects
                        .iter()
                        .any(|(_, o)| *o == object)
                }))
    });
    if !referenced && let Some(mut host) = kernel.audio.take() {
        host.close(kernel, object);
        kernel.audio = Some(host);
    }
}
pub(crate) fn release_session(kernel: &mut Kernel, session: usize) {
    let mut objects = Vec::new();
    if let crate::ipc::Endpoint::Host(object) = kernel.sessions[session].endpoint {
        objects.push(object);
    }
    if let Some(d) = kernel.sessions[session].domain {
        objects.extend(kernel.host.domains[d].objects.iter().map(|(_, o)| *o));
    }
    for object in objects {
        release_if_unreferenced(kernel, object);
    }
}
