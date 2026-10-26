//! A strict, CPU-free bus that routes accesses to registered devices.

#[derive(Default)]
pub struct TestBus {
    windows: Vec<(u64, u64, Box<dyn crate::Device>)>,
}
impl TestBus {
    pub fn register(
        &mut self,
        base: u64,
        size: u64,
        device: Box<dyn crate::Device>,
    ) -> Result<usize, String> {
        let end = base
            .checked_add(size)
            .filter(|_| size != 0)
            .ok_or("invalid test window")?;
        if self
            .windows
            .iter()
            .any(|(b, n, _)| base < b + n && *b < end)
        {
            return Err("overlapping test windows".into());
        }
        let index = self.windows.len();
        self.windows.push((base, size, device));
        Ok(index)
    }
    fn index(&self, address: u64, size: u8) -> Result<usize, String> {
        self.windows
            .iter()
            .position(|(base, len, _)| {
                address >= *base
                    && address
                        .checked_add(size as u64)
                        .is_some_and(|end| end <= base + len)
            })
            .ok_or("unmapped test access".into())
    }
    pub fn read(
        &mut self,
        cx: &mut crate::Cx<'_>,
        address: u64,
        size: u8,
    ) -> Result<u64, String> {
        let index = self.index(address, size)?;
        cx.device = index;
        let result = self.windows[index].2.read(cx, address, size);
        cx.take_refusal().map_or(Ok(result), Err)
    }
    pub fn write(
        &mut self,
        cx: &mut crate::Cx<'_>,
        address: u64,
        size: u8,
        value: u64,
    ) -> Result<(), String> {
        let index = self.index(address, size)?;
        cx.device = index;
        self.windows[index].2.write(cx, address, size, value);
        cx.take_refusal().map_or(Ok(()), Err)
    }
    pub fn event(
        &mut self,
        cx: &mut crate::Cx<'_>,
        device: usize,
        token: u64,
    ) -> Result<(), String> {
        cx.device = device;
        self.windows[device].2.event(cx, token);
        cx.take_refusal().map_or(Ok(()), Err)
    }
    pub fn save(&self) -> serde_json::Value {
        serde_json::Value::Array(self.windows.iter().map(|(_, _, d)| d.save()).collect())
    }
    pub fn restore(&mut self, state: &serde_json::Value) -> Result<(), String> {
        let states = state
            .as_array()
            .filter(|s| s.len() == self.windows.len())
            .ok_or("invalid test checkpoint")?;
        for ((_, _, device), state) in self.windows.iter_mut().zip(states) {
            device.restore(state)?;
        }
        Ok(())
    }
}
