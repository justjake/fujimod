//! Host-only proof that a private-engine processor's translations still match
//! guest memory after a peer ran.
//!
//! An M-profile processor translates code from RAM it shares with peers whose
//! stores its engine cannot observe. Discarding every translation whenever a
//! peer ran is always correct and costs a retranslation of the hot path on
//! every interleaved slice. This shadow keeps the translations when the bytes
//! they were made from are provably unchanged.
//!
//! The owner touches the pages of each instruction before that instruction is
//! entered, and the pages of the stop PC when a slice ends, so every page that
//! can hold a live translation has been touched. A page touched in a slice is
//! copied when that slice ends, after the processor's own stores, which its
//! engine already tracks. The first touch after a peer ran, or after the host
//! or a device wrote RAM, compares the copy with guest memory; a difference
//! means code changed behind the engine and the owner must discard the
//! translations before the instruction is entered.
//!
//! Nothing here is modeled state. It never changes which instructions run,
//! only whether the engine retranslates them.
use fujimod_emulator_cpu::Cpu;
use std::collections::HashMap;
use std::ptr::NonNull;

const PAGE: u64 = 4096;
type Bytes = [u8; PAGE as usize];

struct Page {
    /// Bytes at the end of the last slice that touched the page.
    bytes: Box<Bytes>,
    /// The page in host memory, when the processor maps it from external
    /// backing. Valid until the processor's maps change, which discards.
    host: Option<NonNull<u8>>,
    /// Peer epoch in which the bytes were last known equal to guest memory.
    verified: u64,
    /// Slice in which the page was last queued for a copy.
    touched: u64,
}
impl Page {
    /// Copy the page's present guest bytes; false when they cannot be read.
    fn read(cpu: &dyn Cpu, address: u64, host: Option<NonNull<u8>>, out: &mut Bytes) -> bool {
        match host {
            Some(host) => {
                // SAFETY: `host` addresses a whole mapped page of external
                // backing that outlives the processor, and execution is
                // serialized with this read.
                unsafe {
                    std::ptr::copy_nonoverlapping(host.as_ptr(), out.as_mut_ptr(), out.len())
                };
                true
            }
            None => cpu.read_memory(address, &mut out[..]).is_ok(),
        }
    }
    fn differs(&self, cpu: &dyn Cpu, address: u64, scratch: &mut Bytes) -> bool {
        match self.host {
            // SAFETY: as in `read`.
            Some(host) => unsafe {
                std::slice::from_raw_parts(host.as_ptr(), self.bytes.len()) != &self.bytes[..]
            },
            None => !Self::read(cpu, address, None, scratch) || scratch[..] != self.bytes[..],
        }
    }
}

#[derive(Default)]
pub(super) struct CodeShadow {
    pages: HashMap<u64, Page>,
    /// Pages to copy when the current slice ends.
    touched: Vec<u64>,
    /// The page of the previous instruction, with its host address; its touch
    /// already covered the next page too.
    last: Option<(u64, Option<NonNull<u8>>)>,
    epoch: u64,
    slice: u64,
    /// Executed code lies outside checked RAM: translations made since the
    /// last discard cannot be proven, so the next entry after a peer ran must
    /// discard them, as an owner without this shadow always does.
    unproven: bool,
    /// Code changed behind the engine: translations must be discarded before
    /// this processor enters another instruction.
    changed: bool,
    /// The running slice must stop, discard translations and resume.
    resync: bool,
    /// Entries after a peer ran that kept the translations, discards of the
    /// translations, and discards because code changed behind the engine.
    retained: u64,
    discards: u64,
    changes: u64,
    /// Pages found changed behind the engine, with how often.
    changed_pages: HashMap<u64, u64>,
    scratch: Option<Box<Bytes>>,
}

/// What a touch found.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Touch {
    /// The instruction's pages match the bytes its translations were made from.
    Proven,
    /// A touched page changed behind the engine: translations must go before
    /// execution.
    Changed,
    /// The page is not readable checked RAM: fall back to always discarding.
    Unchecked,
}

impl CodeShadow {
    /// Whether an entry after a peer ran must discard translations.
    pub(super) fn unproven(&self) -> bool {
        self.unproven
    }
    /// Whether the next entry must discard translations even when no peer ran.
    pub(super) fn changed(&self) -> bool {
        self.changed
    }
    #[cfg(test)]
    pub(super) fn retained(&self) -> u64 {
        self.retained
    }
    /// Host-only counters for a run report.
    pub(super) fn counters(&self) -> String {
        let mut changed: Vec<_> = self.changed_pages.iter().collect();
        changed.sort_by_key(|&(page, count)| (std::cmp::Reverse(*count), *page));
        format!(
            "kept={} discarded={} changed-behind-engine={} pages={} changed-pages={}",
            self.retained,
            self.discards,
            self.changes,
            self.pages.len(),
            changed
                .iter()
                .take(8)
                .map(|(page, count)| format!("{page:#x}:{count}"))
                .collect::<Vec<_>>()
                .join(",")
        )
    }
    /// The engine's translations were discarded.
    pub(super) fn discarded(&mut self) {
        self.discards += 1;
        self.changes += u64::from(self.changed);
        self.pages.clear();
        self.touched.clear();
        self.last = None;
        self.unproven = false;
        self.changed = false;
        self.resync = false;
        self.slice += 1;
    }
    /// A peer ran, or the host or a device wrote RAM, since this processor's
    /// last slice: every copy needs a new comparison before its code is
    /// entered.
    pub(super) fn peer_ran(&mut self) {
        self.epoch += 1;
        self.retained += 1;
        self.last = None;
    }
    /// A slice begins.
    pub(super) fn begin_slice(&mut self) {
        self.slice += 1;
        self.last = None;
    }
    pub(super) fn request_resync(&mut self) {
        self.changed = true;
        self.resync = true;
    }
    /// Something other than a peer's or the host's write to RAM may have
    /// outdated the translations: a store this processor made through an
    /// alias, a write that landed during its own slice, a changed memory
    /// decoding or a reset. Only a discard resynchronizes those.
    pub(super) fn invalidate(&mut self) {
        self.changed = true;
    }
    /// Code outside checked RAM was entered.
    pub(super) fn entered_unchecked(&mut self) {
        self.unproven = true;
    }
    pub(super) fn take_resync(&mut self) -> bool {
        std::mem::take(&mut self.resync)
    }
    /// The 16-bit word at `pc`, read from host memory, when the page of the
    /// instruction just touched is mapped from external backing. It is the
    /// word the processor's own memory read returns.
    #[inline]
    pub(super) fn fetch(&self, pc: u64) -> Option<u16> {
        let (page, host) = self.last?;
        let offset = pc.wrapping_sub(page);
        if offset > PAGE - 2 {
            return None;
        }
        // SAFETY: `host` addresses a whole mapped page of external backing
        // and `offset + 2` stays inside it; execution is serialized with
        // this read.
        Some(u16::from_le(unsafe {
            host?.as_ptr().add(offset as usize).cast::<u16>().read_unaligned()
        }))
    }
    /// Touch the pages an instruction at `pc` can occupy, and the page after
    /// its own: a translation that starts in one page ends within the next.
    /// `checked` reports whether a whole page is ordinary RAM of this
    /// processor, so host reads never reach a device.
    #[inline]
    pub(super) fn touch(
        &mut self,
        cpu: &dyn Cpu,
        pc: u64,
        checked: impl Fn(u64) -> bool,
    ) -> Touch {
        let page = pc & !(PAGE - 1);
        if self.last.is_some_and(|(last, _)| last == page) {
            return Touch::Proven;
        }
        self.touch_slow(cpu, page, &checked)
    }
    #[cold]
    fn touch_slow(&mut self, cpu: &dyn Cpu, page: u64, checked: &dyn Fn(u64) -> bool) -> Touch {
        self.last = None;
        if self.unproven {
            return Touch::Unchecked;
        }
        let mut host = None;
        for (index, address) in [Some(page), page.checked_add(PAGE)].into_iter().enumerate() {
            let Some(address) = address else { continue };
            match self.touch_page(cpu, address, checked) {
                (Touch::Proven, pointer) => {
                    if index == 0 {
                        host = pointer;
                    }
                }
                // Nothing readable follows this page, so no instruction
                // extends past it.
                (Touch::Unchecked, _)
                    if index == 1 && cpu.read_memory(address, &mut [0; 2]).is_err() => {}
                (other, _) => return other,
            }
        }
        self.last = Some((page, host));
        Touch::Proven
    }
    fn touch_page(
        &mut self,
        cpu: &dyn Cpu,
        address: u64,
        checked: &dyn Fn(u64) -> bool,
    ) -> (Touch, Option<NonNull<u8>>) {
        let (epoch, slice) = (self.epoch, self.slice);
        if let Some(page) = self.pages.get_mut(&address) {
            if page.verified != epoch {
                let scratch = self
                    .scratch
                    .get_or_insert_with(|| Box::new([0; PAGE as usize]));
                if page.differs(cpu, address, scratch) {
                    *self.changed_pages.entry(address).or_default() += 1;
                    return (Touch::Changed, None);
                }
                page.verified = epoch;
            }
            if page.touched != slice {
                page.touched = slice;
                self.touched.push(address);
            }
            return (Touch::Proven, page.host);
        }
        if !checked(address) {
            return (Touch::Unchecked, None);
        }
        let host = cpu.backing_pointer(address, PAGE as usize);
        let mut bytes = Box::new([0; PAGE as usize]);
        if !Page::read(cpu, address, host, &mut bytes) {
            return (Touch::Unchecked, None);
        }
        self.pages.insert(
            address,
            Page {
                bytes,
                host,
                verified: epoch,
                touched: slice,
            },
        );
        self.touched.push(address);
        (Touch::Proven, host)
    }
    /// A slice ended with the processor stopped before `pc`. The engine may
    /// already hold a translation for it. Copy every touched page: the copies
    /// then equal the bytes every live translation was made from.
    pub(super) fn end_slice(
        &mut self,
        cpu: &dyn Cpu,
        pc: Option<u64>,
        checked: impl Fn(u64) -> bool,
    ) {
        match pc.map(|pc| self.touch(cpu, pc, checked)) {
            Some(Touch::Proven) => (),
            Some(Touch::Unchecked) => self.unproven = true,
            // An unknown stop PC proves nothing about its translation.
            Some(Touch::Changed) | None => self.changed = true,
        }
        for address in self.touched.drain(..) {
            let Some(page) = self.pages.get_mut(&address) else {
                continue;
            };
            if !Page::read(cpu, address, page.host, &mut page.bytes) {
                self.changed = true;
            }
        }
        self.last = None;
    }
}
