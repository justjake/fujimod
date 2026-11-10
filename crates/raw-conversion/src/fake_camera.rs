//! A PTP device in USB RAW CONV. mode at the bulk-transfer level, for tests:
//! it parses containers, keeps USB short- and zero-length-packet framing, and
//! converts by writing a JPEG whose MakerNote carries the profile's film word.
use crate::profile::{Field, Profile};
use crate::ptp::Pipe;
use crate::run::{RAW_CONVERSION_PROFILE, SEND_OBJECT, SEND_OBJECT_INFO, START_RAW_CONVERSION};
use std::collections::{BTreeMap, VecDeque};

pub struct FakeCamera {
    pub packet: usize,
    /// Property words the camera's profile has.
    pub count: u16,
    pub profile: Profile,
    pub raf: Option<Vec<u8>>,
    pub objects: BTreeMap<u32, Vec<u8>>,
    /// File names and capture dates of SD card objects, by handle.
    pub names: BTreeMap<u32, (String, String)>,
    pub next_handle: u32,
    pub uploads: usize,
    pub conversions: usize,
    /// Refuse SetDevicePropValue for the profile.
    pub refuse_profile: bool,
    /// Keep this field at its as-shot value whatever is sent.
    pub clamp: Option<Field>,
    /// Fail the USB transfer that starts conversion number N (1-based).
    pub fail_conversion: Option<usize>,
    /// A session left open by an earlier controller.
    pub session_open: bool,
    /// Converted images the JPEG reader cannot parse.
    pub unreadable_output: bool,
    pending: Option<(u16, u32, Vec<u32>)>,
    incoming: Vec<u8>,
    expect_zlp: bool,
    units: VecDeque<Vec<u8>>,
}

const AS_SHOT_WB: u32 = 0x0902_6000;

impl FakeCamera {
    pub fn new(packet: usize) -> Self {
        Self {
            packet,
            count: 29,
            profile: Profile(Vec::new()),
            raf: None,
            objects: BTreeMap::new(),
            names: BTreeMap::new(),
            next_handle: 0x10,
            uploads: 0,
            conversions: 0,
            refuse_profile: false,
            clamp: None,
            fail_conversion: None,
            session_open: false,
            unreadable_output: false,
            pending: None,
            incoming: Vec::new(),
            expect_zlp: false,
            units: VecDeque::new(),
        }
    }

    pub fn add_card_file(&mut self, name: &str, bytes: Vec<u8>) {
        self.names
            .insert(self.next_handle, (name.into(), "20261110T120000".into()));
        self.objects.insert(self.next_handle, bytes);
        self.next_handle += 1;
    }
    /// A RAF whose header points at a preview JPEG carrying `iso`.
    pub fn add_card_raf(&mut self, name: &str, iso: u16, captured: &str) -> Vec<u8> {
        let preview = crate::jpeg::synthetic(160, 120, iso, &[]);
        let mut raf = b"FUJIFILMCCD-RAW 0201FF383501".to_vec();
        raf.resize(84, 0);
        raf.extend_from_slice(&100u32.to_be_bytes());
        raf.extend_from_slice(&(preview.len() as u32).to_be_bytes());
        raf.resize(100, 0);
        raf.extend_from_slice(&preview);
        raf.extend_from_slice(&[0x5a; 3000]);
        self.add_card_file(name, raf.clone());
        self.names.get_mut(&(self.next_handle - 1)).unwrap().1 = captured.into();
        raf
    }

    fn as_shot(&self) -> Profile {
        let mut bytes = vec![0u8; 0x201 + 4 * self.count as usize];
        bytes[0..2].copy_from_slice(&self.count.to_le_bytes());
        bytes[2] = 9;
        for (i, c) in "00080001".encode_utf16().enumerate() {
            bytes[3 + 2 * i..5 + 2 * i].copy_from_slice(&c.to_le_bytes());
        }
        let raf = Profile(bytes);
        let (p, _) = raf.patch(&[
            (Field::FilmSimulation, 1),
            (Field::MonochromeWarmCool, AS_SHOT_WB),
            (Field::MonochromeMagentaGreen, AS_SHOT_WB),
            (Field::NoiseReduction, 0x2000),
        ]);
        p
    }

    fn respond(&mut self, code: u16, id: u32, data: Option<Vec<u8>>) {
        if let Some(data) = data {
            self.push(container(2, code, id, &data));
        }
        self.push(container(3, 0x2001, id, &[]));
    }
    fn fail(&mut self, code: u16, id: u32) {
        self.push(container(3, code, id, &[]));
    }
    fn push(&mut self, unit: Vec<u8>) {
        let zlp = unit.len().is_multiple_of(self.packet);
        self.units.push_back(unit);
        if zlp {
            self.units.push_back(Vec::new());
        }
    }

    fn command(&mut self, code: u16, id: u32, params: Vec<u32>) {
        match code {
            0x1001 => {
                let mut info = Vec::new();
                info.extend_from_slice(&100u16.to_le_bytes());
                info.extend_from_slice(&0xeu32.to_le_bytes());
                info.extend_from_slice(&100u16.to_le_bytes());
                info.push(0);
                info.extend_from_slice(&0u16.to_le_bytes());
                for list in [&[][..], &[], &[0xd183u16, 0xd185, 0xd16e]] {
                    info.extend_from_slice(&(list.len() as u32).to_le_bytes());
                    for p in list {
                        info.extend_from_slice(&p.to_le_bytes());
                    }
                }
                info.extend_from_slice(&0u32.to_le_bytes());
                info.extend_from_slice(&0u32.to_le_bytes());
                for s in ["FUJIFILM", "X-T5", "1.10"] {
                    crate::ptp::write_string(&mut info, s);
                }
                self.respond(code, id, Some(info));
            }
            0x1002 if self.session_open => self.fail(crate::ptp::SESSION_ALREADY_OPEN, id),
            0x1002 => {
                self.session_open = true;
                self.respond(code, id, None)
            }
            0x1003 => {
                self.session_open = false;
                self.respond(code, id, None)
            }
            0x1007 => {
                let mut list = (self.objects.len() as u32).to_le_bytes().to_vec();
                for h in self.objects.keys() {
                    list.extend_from_slice(&h.to_le_bytes());
                }
                self.respond(code, id, Some(list));
            }
            0x1009 => match self.objects.get(&params[0]).cloned() {
                Some(o) => self.respond(code, id, Some(o)),
                None => self.fail(0x2009, id),
            },
            0x1008 => match (self.objects.get(&params[0]), self.names.get(&params[0])) {
                (Some(bytes), Some((name, captured))) => {
                    let mut info = vec![0u8; 8];
                    info.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
                    info.extend_from_slice(&[0; 2 + 4 * 7 + 2 + 8]);
                    crate::ptp::write_string(&mut info, name);
                    crate::ptp::write_string(&mut info, captured);
                    crate::ptp::write_string(&mut info, captured);
                    crate::ptp::write_string(&mut info, "");
                    self.respond(code, id, Some(info));
                }
                _ => self.fail(0x2009, id),
            },
            0x101b => match self.objects.get(&params[0]) {
                Some(bytes) => {
                    let start = (params[1] as usize).min(bytes.len());
                    let end = (start + params[2] as usize).min(bytes.len());
                    let part = bytes[start..end].to_vec();
                    self.respond(code, id, Some(part));
                }
                None => self.fail(0x2009, id),
            },
            0x100b => match self.objects.remove(&params[0]) {
                Some(_) => self.respond(code, id, None),
                None => self.fail(0x2009, id),
            },
            0x1015 if params[0] == RAW_CONVERSION_PROFILE && self.raf.is_some() => {
                let p = self.profile.0.clone();
                self.respond(code, id, Some(p));
            }
            0x1015 => self.fail(0x2019, id),
            0x1016 | SEND_OBJECT_INFO | SEND_OBJECT => self.pending = Some((code, id, params)),
            _ => self.fail(0x2005, id),
        }
    }

    fn data(&mut self, code: u16, id: u32, params: Vec<u32>, data: Vec<u8>) {
        match code {
            SEND_OBJECT_INFO => self.respond(code, id, None),
            SEND_OBJECT => {
                self.raf = Some(data);
                self.uploads += 1;
                self.profile = self.as_shot();
                self.respond(code, id, None);
            }
            0x1016 if params[0] == RAW_CONVERSION_PROFILE => {
                if self.refuse_profile || data.len() != self.profile.0.len() {
                    return self.fail(0x201c, id);
                }
                let mut sent = Profile(data);
                if let Some(f) = self.clamp {
                    let original = self.as_shot().get(f).unwrap();
                    sent = sent.patch(&[(f, original)]).0;
                }
                self.profile = sent;
                self.respond(code, id, None);
            }
            0x1016 if params[0] == START_RAW_CONVERSION => {
                self.conversions += 1;
                let film = self.profile.get(Field::FilmSimulation).unwrap() as u16;
                let mut jpeg = crate::jpeg::synthetic(6240, 4160, 160, &[(0x1401, film)]);
                if self.unreadable_output {
                    jpeg.truncate(40);
                }
                self.objects.insert(self.next_handle, jpeg);
                self.next_handle += 1;
                self.respond(code, id, None);
            }
            _ => self.fail(0x2005, id),
        }
    }
}

fn container(kind: u16, code: u16, id: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = ((12 + payload.len()) as u32).to_le_bytes().to_vec();
    out.extend_from_slice(&kind.to_le_bytes());
    out.extend_from_slice(&code.to_le_bytes());
    out.extend_from_slice(&id.to_le_bytes());
    out.extend_from_slice(payload);
    out
}

impl Pipe for FakeCamera {
    async fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
        if self.expect_zlp {
            assert!(bytes.is_empty(), "expected a zero-length packet");
            self.expect_zlp = false;
            return Ok(());
        }
        assert!(bytes.len() <= 1 << 20);
        self.incoming.extend_from_slice(bytes);
        let length = u32::from_le_bytes(self.incoming[0..4].try_into().unwrap()) as usize;
        if self.incoming.len() < length {
            assert_eq!(
                bytes.len() % self.packet,
                0,
                "short packet inside a container"
            );
            return Ok(());
        }
        assert_eq!(self.incoming.len(), length, "bytes past the container");
        let c = std::mem::take(&mut self.incoming);
        self.expect_zlp = length.is_multiple_of(self.packet);
        let kind = u16::from_le_bytes([c[4], c[5]]);
        let code = u16::from_le_bytes([c[6], c[7]]);
        let id = u32::from_le_bytes(c[8..12].try_into().unwrap());
        match kind {
            1 => {
                let params = c[12..]
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|b| u32::from_le_bytes(*b))
                    .collect();
                assert!(self.pending.is_none());
                self.command(code, id, params);
            }
            2 => {
                let (pcode, pid, params) = self.pending.take().expect("data without a command");
                assert_eq!((pcode, pid), (code, id));
                if code == 0x1016
                    && params[0] == START_RAW_CONVERSION
                    && self.fail_conversion == Some(self.conversions + 1)
                {
                    return Err("disconnected".into());
                }
                self.data(code, id, params, c[12..].to_vec());
            }
            _ => panic!("container type {kind} from the host"),
        }
        Ok(())
    }

    async fn read(&mut self, maximum: usize) -> Result<Vec<u8>, String> {
        assert_eq!(maximum % self.packet, 0);
        let unit = self.units.front_mut().ok_or("read with nothing to send")?;
        let n = maximum.min(unit.len());
        let out: Vec<u8> = unit.drain(..n).collect();
        if unit.is_empty() {
            self.units.pop_front();
            // A transfer with room left also takes the terminating zero-length packet.
            if n < maximum && n > 0 && self.units.front().is_some_and(Vec::is_empty) {
                self.units.pop_front();
            }
        }
        Ok(out)
    }

    fn packet_size(&self) -> usize {
        self.packet
    }
}
