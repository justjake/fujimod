use crate::ScanoutHost;
pub struct Observation<'a> {
    pub read: &'a mut dyn FnMut(u64, usize) -> Result<Vec<u8>, String>,
    pub diagnostic_hashes: bool,
}
impl ScanoutHost for Observation<'_> {
    fn diagnostic_hashes(&self) -> bool {
        self.diagnostic_hashes
    }
    fn read_bytes(
        &mut self,
        address: u64,
        count: usize,
        _cpu: i64,
    ) -> fujimod_framebuffer::Result<Vec<u8>> {
        (self.read)(address, count).map_err(Into::into)
    }
    fn render_rgb(
        &mut self,
        raw: &[u8],
        width: usize,
        height: usize,
        stride: usize,
        packing: &str,
        matrix: &str,
    ) -> fujimod_framebuffer::Result<Vec<u8>> {
        fujimod_framebuffer::packed_color::decode(
            raw,
            width as u64,
            height as u64,
            stride as u64,
            packing,
            matrix,
        )
        .map_err(Into::into)
    }
    fn encode_png(
        &mut self,
        _rgb: &[u8],
        _width: usize,
        _height: usize,
    ) -> fujimod_framebuffer::Result<Option<Vec<u8>>> {
        Ok(None)
    }
}
