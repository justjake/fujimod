//! Integer and section encodings shared by generated Wasm modules.
#[derive(Default, Debug)]
pub struct Binary(Vec<u8>);
impl Binary {
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }
    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
    pub fn extend(&mut self, bytes: &[u8]) {
        self.0.extend_from_slice(bytes);
    }
    pub fn byte(&mut self, byte: u8) {
        self.0.push(byte);
    }
    pub fn unsigned(&mut self, mut value: u32) {
        loop {
            let byte = (value & 127) as u8;
            value >>= 7;
            self.byte(byte | if value == 0 { 0 } else { 128 });
            if value == 0 {
                break;
            }
        }
    }
    pub fn signed(&mut self, mut value: i64) {
        loop {
            let byte = (value & 127) as u8;
            value >>= 7;
            let done = (value == 0 && byte & 64 == 0) || (value == -1 && byte & 64 != 0);
            self.byte(byte | if done { 0 } else { 128 });
            if done {
                break;
            }
        }
    }
    pub fn text(&mut self, value: &str) -> Result<(), String> {
        self.unsigned(
            value
                .len()
                .try_into()
                .map_err(|_| "Wasm string exceeds u32")?,
        );
        self.extend(value.as_bytes());
        Ok(())
    }
    pub fn section(&mut self, id: u8, body: Binary) -> Result<(), String> {
        let length = body
            .0
            .len()
            .try_into()
            .map_err(|_| "Wasm section exceeds u32")?;
        self.byte(id);
        self.unsigned(length);
        self.extend(&body.0);
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn leb_boundaries_and_signed_extrema() {
        for (value, expected) in [
            (0, vec![0]),
            (127, vec![127]),
            (128, vec![128, 1]),
            (624485, vec![0xe5, 0x8e, 0x26]),
            (u32::MAX, vec![255, 255, 255, 255, 15]),
        ] {
            let mut b = Binary::default();
            b.unsigned(value);
            assert_eq!(b.bytes(), expected);
        }
        for (value, expected) in [
            (0, vec![0]),
            (63, vec![63]),
            (64, vec![192, 0]),
            (-1, vec![127]),
            (-64, vec![64]),
            (-65, vec![191, 127]),
            (-624485, vec![0x9b, 0xf1, 0x59]),
            (
                i64::MIN,
                vec![128, 128, 128, 128, 128, 128, 128, 128, 128, 127],
            ),
            (
                i64::MAX,
                vec![255, 255, 255, 255, 255, 255, 255, 255, 255, 0],
            ),
        ] {
            let mut b = Binary::default();
            b.signed(value);
            assert_eq!(b.bytes(), expected);
        }
    }
    #[test]
    fn section_size_counts_utf8_bytes() {
        let mut body = Binary::default();
        body.text("λ").unwrap();
        let mut module = Binary::default();
        module.section(0, body).unwrap();
        assert_eq!(module.bytes(), [0, 3, 2, 0xce, 0xbb]);
    }
}
