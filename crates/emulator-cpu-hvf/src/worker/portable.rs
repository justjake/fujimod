//! Fixed architectural fields; no vCPU IDs, pointers, hooks or host clock state.
use super::*;
const MAGIC: &[u8; 8] = b"FMHVF001";
const LENGTH: usize = 8 + 35 * 8 + 32 * 16 + SYSTEM.len() * 10 + 2 + 40;
impl Registers {
    pub fn export(&self) -> Result<Vec<u8>, String> {
        if self
            .system
            .iter()
            .map(|(id, _)| *id)
            .ne(SYSTEM.iter().copied())
        {
            return Err("HVF snapshot system-register layout differs".into());
        }
        let mut out = Vec::with_capacity(LENGTH);
        out.extend_from_slice(MAGIC);
        for value in self.core {
            out.extend_from_slice(&value.to_le_bytes());
        }
        for vector in self.vectors {
            out.extend_from_slice(&vector);
        }
        for (id, value) in &self.system {
            out.extend_from_slice(&id.to_le_bytes());
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.extend(self.interrupts.map(u8::from));
        out.extend_from_slice(&self.exclusive.to_bytes());
        Ok(out)
    }
    pub fn import(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() != LENGTH || &bytes[..8] != MAGIC {
            return Err("invalid HVF CPU snapshot schema/extent".into());
        }
        let mut cursor = 8;
        let mut core = [0; 35];
        for value in &mut core {
            *value = u64::from_le_bytes(bytes[cursor..cursor + 8].try_into().unwrap());
            cursor += 8;
        }
        if core[ffi::PSTATE as usize] & (1 << 21) != 0 {
            return Err("HVF snapshot contains backend single-step state".into());
        }
        let mut vectors = [[0; 16]; 32];
        for vector in &mut vectors {
            vector.copy_from_slice(&bytes[cursor..cursor + 16]);
            cursor += 16;
        }
        let mut system = Vec::with_capacity(SYSTEM.len());
        for &expected in SYSTEM {
            let id = u16::from_le_bytes(bytes[cursor..cursor + 2].try_into().unwrap());
            let value = u64::from_le_bytes(bytes[cursor + 2..cursor + 10].try_into().unwrap());
            if id != expected {
                return Err("HVF snapshot system-register layout differs".into());
            }
            system.push((id, value));
            cursor += 10;
        }
        if bytes[cursor] > 1 || bytes[cursor + 1] > 1 {
            return Err("HVF snapshot interrupt encoding differs".into());
        }
        let interrupts = [bytes[cursor] != 0, bytes[cursor + 1] != 0];
        let exclusive = exclusive::Saved::from_bytes(&bytes[cursor + 2..])?;
        Ok(Self {
            core,
            vectors,
            system,
            interrupts,
            exclusive,
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn architecture_roundtrip_preserves_bits_and_rejects_truncation_unknown_registers_and_debug_state()
     {
        let mut monitors = exclusive::Monitors::default();
        let owner = monitors.register(64).unwrap();
        monitors.load(owner, 0x12340, 8);
        let mut core = std::array::from_fn(|i| (i as u64) << 45 | i as u64);
        core[ffi::PSTATE as usize] = 0x3c5;
        let state = Registers {
            core,
            vectors: std::array::from_fn(|i| [i as u8; 16]),
            system: SYSTEM
                .iter()
                .map(|id| (*id, u64::from(*id) << 40))
                .collect(),
            interrupts: [true, false],
            exclusive: monitors.save(owner),
        };
        let bytes = state.export().unwrap();
        assert_eq!(Registers::import(&bytes).unwrap().export().unwrap(), bytes);
        for n in 0..bytes.len() {
            assert!(Registers::import(&bytes[..n]).is_err());
        }
        let mut bad = bytes.clone();
        bad[8 + 35 * 8 + 32 * 16] ^= 1;
        assert!(Registers::import(&bad).is_err());
        let mut bad = bytes;
        bad[8 + ffi::PSTATE as usize * 8 + 2] |= 1 << 5;
        assert!(Registers::import(&bad).is_err());
    }
}
