//! GPU-owned identities, fixed page mappings and bounded Switch submission ABI.
//! These IDs are not Horizon handles, nvmap handles or nvhost syncpoints.
use crate::{Result, invalid, memory::*};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
pub(crate) fn identity() -> Result<u64> {
    let mut current = NEXT_ID.load(Ordering::Relaxed);
    loop {
        let next = current
            .checked_add(1)
            .ok_or_else(|| invalid("GPU identity exhausted"))?;
        match NEXT_ID.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(id) => return Ok(id),
            Err(observed) => current = observed,
        }
    }
}
macro_rules! id {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $name(pub(crate) u64);
    };
}
id!(AddressSpaceId);
id!(BackingId);
id!(ReservationId);
id!(MappingId);
id!(ChannelId);
/// Supported fixed-mapping granularity; not a complete Switch VA-region policy.
pub const PAGE_SIZE: u64 = 4096;
/// Defensive resident registry budget, not a Maxwell hardware limit.
pub const MAX_OBJECTS: usize = 4096;
/// Bound individual transactional work independently of resident capacity.
pub const MAX_MAP_CHANGES: usize = 128;

#[derive(Debug, Clone)]
struct Backing {
    id: BackingId,
    base: GuestAddress,
    size: u64,
}
#[derive(Debug, Clone)]
struct Reservation {
    id: ReservationId,
    address: GpuAddress,
    size: u64,
    mappings: Vec<BufferMapping>,
}
#[derive(Debug, Clone)]
struct BufferMapping {
    id: MappingId,
    backing: BackingId,
    address: GpuAddress,
    size: u64,
}
#[derive(Debug, Clone)]
struct Space {
    id: AddressSpaceId,
    epoch: u64,
    map: AddressMap,
    reservations: Vec<Reservation>,
}
/// Registry of runtime-owned canonical backing. It owns no guest bytes. Register
/// one backing once; aliases map its ID, never register overlapping CPU ranges.
/// Mutation requires exclusive execution and stable runtime backing lifetimes.
#[derive(Debug, Default)]
pub struct MemoryManager {
    backings: Vec<Backing>,
    spaces: Vec<Space>,
}
/// A bounded replacement of resident pages within an existing reservation.
/// `None` removes backing; it does not create readable zero-filled guest RAM.
#[derive(Debug, Clone, Copy)]
pub struct MapChange {
    pub reservation: ReservationId,
    pub address: GpuAddress,
    pub size: u64,
    pub backing: Option<(BackingId, u64)>,
}
/// Snapshot of a currently resident mapping, not an owning guest-memory borrow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MappingInfo {
    pub id: MappingId,
    pub address: GpuAddress,
    pub size: u64,
}
fn aligned(value: u64) -> bool {
    value % PAGE_SIZE == 0
}
fn extent(start: u64, size: u64, gpu: bool) -> Result<u64> {
    let end = start
        .checked_add(size)
        .ok_or_else(|| invalid("mapping range overflow"))?;
    if size == 0 || !aligned(start) || !aligned(size) || (gpu && (start == 0 || end > 1 << 40)) {
        return Err(invalid(
            "unsupported empty, unaligned or out-of-range page mapping",
        ));
    }
    Ok(end)
}
impl MemoryManager {
    pub fn mappings(
        &self,
        space: AddressSpaceId,
        reservation: ReservationId,
    ) -> Result<Vec<MappingInfo>> {
        let r = self
            .space(space)?
            .reservations
            .iter()
            .find(|r| r.id == reservation)
            .ok_or_else(|| invalid("wrong reservation owner"))?;
        Ok(r.mappings
            .iter()
            .map(|m| MappingInfo {
                id: m.id,
                address: m.address,
                size: m.size,
            })
            .collect())
    }
    /// Transactional metadata replacement. No guest bytes are copied. Failed
    /// validation retains every old mapping ID and mapping epoch. Newly issued
    /// opaque IDs may be skipped by an unsuccessful trial.
    pub fn apply_map_changes(
        &mut self,
        space: AddressSpaceId,
        changes: &[MapChange],
    ) -> Result<()> {
        self.space(space)?;
        if changes.len() > MAX_MAP_CHANGES {
            return Err(invalid("mapping transaction capacity"));
        }
        if changes.is_empty() {
            return Ok(());
        }
        let mut trial = Self {
            backings: self.backings.clone(),
            spaces: self.spaces.clone(),
        };
        for change in changes {
            trial.apply_map_change(space, *change)?;
        }
        *self = trial;
        Ok(())
    }
    fn apply_map_change(&mut self, space: AddressSpaceId, change: MapChange) -> Result<()> {
        let end = extent(change.address.0, change.size, true)?;
        let r = self
            .space(space)?
            .reservations
            .iter()
            .find(|r| r.id == change.reservation)
            .ok_or_else(|| invalid("wrong reservation owner"))?;
        if change.address.0 < r.address.0 || end > r.address.0 + r.size {
            return Err(invalid("replacement outside reservation"));
        }
        let reservation_start = r.address.0;
        if let Some((backing, offset)) = change.backing {
            if !aligned(offset) {
                return Err(invalid("unaligned replacement backing"));
            }
            self.backing_range(
                backing,
                offset,
                usize::try_from(change.size).map_err(|_| invalid("replacement exceeds host"))?,
            )?;
        }
        let overlaps: Vec<_> = r
            .mappings
            .iter()
            .filter(|m| m.address.0 < end && change.address.0 < m.address.0 + m.size)
            .cloned()
            .collect();
        for old in overlaps {
            let old_guest = self.address_map(space)?.resolve(old.address, 1)?;
            let backing_base = self.backing_range(old.backing, 0, 1)?;
            let backing_offset = old_guest
                .0
                .checked_sub(backing_base.0)
                .ok_or_else(|| invalid("invalid canonical mapping offset"))?;
            self.unmap(space, old.id)?;
            if old.address.0 < change.address.0 {
                self.map_subrange(
                    space,
                    change.reservation,
                    old.backing,
                    backing_offset,
                    old.address.0 - reservation_start,
                    change.address.0 - old.address.0,
                )?;
            }
            let old_end = old.address.0 + old.size;
            if end < old_end {
                self.map_subrange(
                    space,
                    change.reservation,
                    old.backing,
                    backing_offset
                        .checked_add(end - old.address.0)
                        .ok_or_else(|| invalid("replacement backing overflow"))?,
                    end - reservation_start,
                    old_end - end,
                )?;
            }
        }
        if let Some((backing, offset)) = change.backing {
            self.map_subrange(
                space,
                change.reservation,
                backing,
                offset,
                change.address.0 - reservation_start,
                change.size,
            )?;
        }
        Ok(())
    }
    pub fn register_backing(&mut self, base: GuestAddress, size: u64) -> Result<BackingId> {
        let end = extent(base.0, size, false)?;
        if self.backings.len() >= MAX_OBJECTS
            || self
                .backings
                .iter()
                .any(|b| base.0 < b.base.0 + b.size && b.base.0 < end)
        {
            return Err(invalid("backing capacity or canonical overlap"));
        }
        let id = BackingId(identity()?);
        self.backings.push(Backing { id, base, size });
        Ok(id)
    }
    pub fn backing_range(&self, id: BackingId, offset: u64, size: usize) -> Result<GuestAddress> {
        let b = self
            .backings
            .iter()
            .find(|b| b.id == id)
            .ok_or_else(|| invalid("unknown or stale backing"))?;
        if size == 0
            || offset
                .checked_add(size as u64)
                .is_none_or(|end| end > b.size)
        {
            return Err(invalid("backing subregion out of bounds"));
        }
        Ok(GuestAddress(b.base.0 + offset))
    }
    /// Check release without changing identity, for an exclusive owner that
    /// must release its runtime pin before removing this registration.
    pub fn validate_release_backing(&self, id: BackingId) -> Result<()> {
        if !self.backings.iter().any(|b| b.id == id) {
            return Err(invalid("unknown or stale backing"));
        }
        if self
            .spaces
            .iter()
            .flat_map(|s| &s.reservations)
            .any(|r| r.mappings.iter().any(|m| m.backing == id))
        {
            return Err(invalid("backing still mapped"));
        }
        Ok(())
    }
    pub fn release_backing(&mut self, id: BackingId) -> Result<()> {
        self.validate_release_backing(id)?;
        let n = self
            .backings
            .iter()
            .position(|b| b.id == id)
            .ok_or_else(|| invalid("unknown or stale backing"))?;
        self.backings.remove(n);
        Ok(())
    }
    pub fn create_space(&mut self) -> Result<AddressSpaceId> {
        if self.spaces.len() >= 16 {
            return Err(invalid("address-space capacity"));
        }
        let id = AddressSpaceId(identity()?);
        self.spaces.push(Space {
            id,
            epoch: 0,
            map: AddressMap::default(),
            reservations: Vec::new(),
        });
        Ok(id)
    }
    fn space(&self, id: AddressSpaceId) -> Result<&Space> {
        self.spaces
            .iter()
            .find(|s| s.id == id)
            .ok_or_else(|| invalid("unknown or stale address space"))
    }
    fn space_mut(&mut self, id: AddressSpaceId) -> Result<&mut Space> {
        self.spaces
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or_else(|| invalid("unknown or stale address space"))
    }
    pub fn address_map(&self, id: AddressSpaceId) -> Result<&AddressMap> {
        Ok(&self.space(id)?.map)
    }
    pub fn mapping_epoch(&self, id: AddressSpaceId) -> Result<u64> {
        Ok(self.space(id)?.epoch)
    }
    pub fn reserve_fixed(
        &mut self,
        space: AddressSpaceId,
        address: GpuAddress,
        size: u64,
    ) -> Result<ReservationId> {
        let end = extent(address.0, size, true)?;
        let s = self.space_mut(space)?;
        if s.reservations.len() >= MAX_OBJECTS
            || s.reservations
                .iter()
                .any(|r| address.0 < r.address.0 + r.size && r.address.0 < end)
        {
            return Err(invalid("reservation capacity or overlap"));
        }
        let id = ReservationId(identity()?);
        s.reservations.push(Reservation {
            id,
            address,
            size,
            mappings: Vec::new(),
        });
        Ok(id)
    }
    /// Full-reservation mapping, retaining the original checked API.
    pub fn map(
        &mut self,
        space: AddressSpaceId,
        reservation: ReservationId,
        backing: BackingId,
        offset: u64,
    ) -> Result<MappingId> {
        let size = self
            .space(space)?
            .reservations
            .iter()
            .find(|r| r.id == reservation)
            .ok_or_else(|| invalid("wrong reservation owner"))?
            .size;
        self.map_subrange(space, reservation, backing, offset, 0, size)
    }
    /// Map an unmapped subinterval of an existing VA reservation. The mapping
    /// owns its exact extent; unmapping it preserves neighboring mappings and
    /// leaves reservation ownership intact. Replacement and gathers are absent.
    pub fn map_subrange(
        &mut self,
        space: AddressSpaceId,
        reservation: ReservationId,
        backing: BackingId,
        offset: u64,
        reservation_offset: u64,
        size: u64,
    ) -> Result<MappingId> {
        if !aligned(offset) {
            return Err(invalid("unaligned backing offset"));
        }
        let s = self.space(space)?;
        let r = s
            .reservations
            .iter()
            .find(|r| r.id == reservation)
            .ok_or_else(|| invalid("wrong reservation owner"))?;
        if !aligned(reservation_offset)
            || size == 0
            || !aligned(size)
            || reservation_offset
                .checked_add(size)
                .is_none_or(|end| end > r.size)
        {
            return Err(invalid("mapping subinterval outside reservation"));
        }
        if s.reservations
            .iter()
            .map(|r| r.mappings.len())
            .sum::<usize>()
            >= MAX_OBJECTS
        {
            return Err(invalid("mapping capacity"));
        }
        let address = GpuAddress(
            r.address
                .0
                .checked_add(reservation_offset)
                .ok_or_else(|| invalid("mapping address overflow"))?,
        );
        let guest = self.backing_range(
            backing,
            offset,
            usize::try_from(size).map_err(|_| invalid("mapping exceeds host"))?,
        )?;
        let s = self.space_mut(space)?;
        let epoch = s
            .epoch
            .checked_add(1)
            .ok_or_else(|| invalid("mapping epoch overflow"))?;
        let r = s
            .reservations
            .iter_mut()
            .find(|r| r.id == reservation)
            .unwrap();
        let id = MappingId(identity()?);
        s.map.map(address, guest, size)?;
        r.mappings.push(BufferMapping {
            id,
            backing,
            address,
            size,
        });
        s.epoch = epoch;
        Ok(id)
    }
    pub fn unmap(&mut self, space: AddressSpaceId, mapping: MappingId) -> Result<()> {
        let s = self.space_mut(space)?;
        let epoch = s
            .epoch
            .checked_add(1)
            .ok_or_else(|| invalid("mapping epoch overflow"))?;
        let r = s
            .reservations
            .iter_mut()
            .find(|r| r.mappings.iter().any(|m| m.id == mapping))
            .ok_or_else(|| invalid("unknown or stale mapping"))?;
        let index = r.mappings.iter().position(|m| m.id == mapping).unwrap();
        let m = &r.mappings[index];
        s.map.unmap_exact(m.address, m.size)?;
        r.mappings.remove(index);
        s.epoch = epoch;
        Ok(())
    }
    pub fn free_reservation(
        &mut self,
        space: AddressSpaceId,
        reservation: ReservationId,
    ) -> Result<()> {
        let s = self.space_mut(space)?;
        let n = s
            .reservations
            .iter()
            .position(|r| r.id == reservation)
            .ok_or_else(|| invalid("wrong reservation owner"))?;
        if !s.reservations[n].mappings.is_empty() {
            return Err(invalid("reservation still mapped"));
        }
        s.reservations.remove(n);
        Ok(())
    }
    pub fn destroy_space(&mut self, space: AddressSpaceId) -> Result<()> {
        let s = self.space(space)?;
        if !s.reservations.is_empty() {
            return Err(invalid("address space still reserved"));
        }
        let n = self.spaces.iter().position(|s| s.id == space).unwrap();
        self.spaces.remove(n);
        Ok(())
    }
}

/// Ioctl2 KickoffPb: IOWR('H', 0x1b, 24). Legacy SubmitGpfifo carries
/// entries inline and includes them in the ioctl size (libnx nvchannel.c).
pub const KICKOFF_PB: u32 = 0xc018481b;
#[derive(Debug)]
pub struct SubmitRequest<'a> {
    pub(crate) entries: &'a [u8],
}
impl<'a> SubmitRequest<'a> {
    pub fn decode(code: u32, payload: &'a [u8], extra: Option<&'a [u8]>) -> Result<Self> {
        if payload.len() < 24 {
            return Err(invalid("truncated channel submit header"));
        }
        let word = |n: usize| u32::from_le_bytes(payload[n..n + 4].try_into().unwrap());
        let count = word(8) as usize;
        if count == 0 || count > crate::gpfifo::MAX_ENTRIES {
            return Err(invalid("unsupported channel entry count"));
        }
        // HW_FORMAT only. No fence wait/get/increment, WFI suppression or host
        // syncpoint emulation. The ignored pointer is never dereferenced.
        if word(12) != 4 || word(16) != 0 || word(20) != 0 {
            return Err(invalid("unsupported channel flags or input fence"));
        }
        let size = count
            .checked_mul(8)
            .ok_or_else(|| invalid("entry size overflow"))?;
        let entries = if code == KICKOFF_PB {
            if payload.len() != 24 {
                return Err(invalid("KickoffPb header size"));
            }
            extra.ok_or_else(|| invalid("missing ioctl2 entry buffer"))?
        } else {
            let total = 24 + size;
            if count > 512
                || code != (0xc0004808 | ((total as u32) << 16))
                || payload.len() != total
                || extra.is_some()
            {
                return Err(invalid(
                    "unsupported channel ioctl direction, command or size",
                ));
            }
            &payload[24..]
        };
        if entries.len() != size {
            return Err(invalid("channel entry buffer length"));
        }
        crate::gpfifo::Submission::new(entries)?;
        Ok(Self { entries })
    }
    pub fn entries(&self) -> &'a [u8] {
        self.entries
    }
}
