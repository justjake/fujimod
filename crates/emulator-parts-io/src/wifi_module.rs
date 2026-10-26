//! One-function SDIO module with enumeration, ROM download and MT5931 HIF.
//! Device behavior consumes only SDIO transactions.
pub mod mt5931;
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    Enumeration,
    RomDownload,
    Mt5931,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub protocol: Protocol,
    pub firmware_bytes: u32,
    #[serde(default)]
    pub mt5931: Option<mt5931::Options>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
enum Phase {
    Idle,
    ResetAsserted,
    ResetReady,
    Sized,
    BootloaderReady,
    LoadEnabled,
    Loading,
    FirmwareReady,
    PostDownload,
    Configured,
}
#[derive(Clone, Serialize, serde::Deserialize)]
pub enum Change {
    None,
    Byte(u32, u8),
    Phase(PhaseChange),
    Frame(u32),
    Direct,
    Hif(mt5931::Change),
    Configure(u32),
}
#[derive(Clone, Copy, Serialize, serde::Deserialize)]
pub struct PhaseChange(Phase);
#[derive(Serialize, Deserialize)]
pub struct Operation {
    pub response: u32,
    pub data: Vec<u8>,
    pub change: Change,
}
#[derive(Serialize, Deserialize)]
pub struct WifiModule {
    options: Options,
    #[serde(with = "fujimod_emulator_devices::checkpoint::array")]
    bytes: [u8; 0x112],
    phase: Phase,
    cursor: u32,
    direct: bool,
    endpoint: mt5931::Endpoint,
}
impl WifiModule {
    pub fn fingerprint(&self) -> serde_json::Value {
        serde_json::json!({"options":self.options,"bytes":self.bytes.as_slice(),"phase":self.phase,"cursor":self.cursor,"direct":self.direct,"endpoint":self.endpoint.fingerprint()})
    }

    pub fn new(options: Options) -> Result<Self, String> {
        if options.protocol != Protocol::Enumeration
            && (options.firmware_bytes == 0
                || options.firmware_bytes > 0x1000000
                || !options.firmware_bytes.is_multiple_of(64))
        {
            return Err("Wi-Fi firmware extent must be bounded 64-byte units".into());
        }
        if options.protocol == Protocol::Mt5931 && options.mt5931.is_none() {
            return Err("MT5931 requires explicit endpoint identity".into());
        }
        if let Some(endpoint) = &options.mt5931 {
            if endpoint.mac_address == [0; 6] || endpoint.mac_address[0] & 1 != 0 {
                return Err("MT5931 permanent MAC must be a nonzero unicast address".into());
            }
        }
        let mut bytes = [0; 0x112];
        for (a, v) in [(0, 0x22), (1, 3), (8, 0x82)] {
            bytes[a] = v;
        }
        Ok(Self {
            options,
            bytes,
            phase: Phase::Idle,
            cursor: 0,
            direct: false,
            endpoint: mt5931::Endpoint::default(),
        })
    }
    pub fn protocol(&self) -> Protocol {
        self.options.protocol
    }
    fn writable(address: u32) -> bool {
        matches!(address, 2 | 4 | 6 | 7 | 0x10 | 0x11 | 0x110 | 0x111)
    }
    fn known(address: u32) -> bool {
        Self::writable(address)
            || matches!(
                address,
                0 | 1 | 3 | 5 | 8 | 9 | 10 | 11 | 12 | 13 | 14 | 15 | 0x100 | 0x109 | 0x10a | 0x10b
            )
    }
    pub fn direct(&self, argument: u32) -> Result<Operation, String> {
        let write = argument & 0x80000000 != 0;
        let function = argument >> 28 & 7;
        let raw = argument & 0x08000000 != 0;
        let address = argument >> 9 & 0x1ffff;
        let data = argument as u8;
        if argument & 0x04000100 != 0 {
            return Err("SDIO CMD52 reserved bits".into());
        }
        if function != 0 {
            if self.options.protocol == Protocol::Mt5931
                && function == 1
                && write
                && !raw
                && address == 4
            {
                return Ok(Operation {
                    response: 0,
                    data: Vec::new(),
                    change: Change::Hif(self.endpoint.low_power(data as u32)?),
                });
            }
            if self.options.protocol != Protocol::RomDownload
                || function != 1
                || !write
                || raw
                || address != 4
                || data != 2
                || self.phase != Phase::PostDownload
                || self.direct
            {
                return Err("Unsupported Wi-Fi function direct I/O".into());
            }
            return Ok(Operation {
                data: Vec::new(),
                response: 0,
                change: Change::Direct,
            });
        }
        if !Self::known(address) {
            return Err("Wi-Fi vendor/CIS byte register unsupported".into());
        }
        let old = self.bytes[address as usize];
        if write
            && (!Self::writable(address)
                || address == 2 && data & !2 != 0
                || address == 4 && data & !3 != 0
                || address == 6 && ![0, 1, 8].contains(&data)
                || address == 7 && ![0, 2].contains(&data))
        {
            return Err("Unsupported Wi-Fi CCCR write".into());
        }
        Ok(Operation {
            data: Vec::new(),
            response: if write && raw {
                data as u32
            } else {
                old as u32
            },
            change: if write {
                Change::Byte(address, data)
            } else {
                Change::None
            },
        })
    }
    pub fn interrupt(&self) -> bool {
        self.bytes[2] & 2 != 0 && self.bytes[4] & 3 == 3 && self.endpoint.interrupt()
    }
    pub fn block_size(&self) -> u16 {
        u16::from_le_bytes([self.bytes[0x110], self.bytes[0x111]])
    }
    pub fn vendor(
        &self,
        address: u32,
        write: bool,
        payload: &[u8],
        quantity: usize,
    ) -> Result<Operation, String> {
        if self.options.protocol == Protocol::Enumeration {
            return Err("Wi-Fi vendor transfers require ROM download option".into());
        }
        if self.options.protocol == Protocol::Mt5931 && self.phase == Phase::Configured {
            let (data, change) = self.endpoint.transfer(
                self.options.mt5931.as_ref().unwrap(),
                address,
                write,
                payload,
                quantity,
            )?;
            return Ok(Operation {
                response: data
                    .get(..4)
                    .map_or(0, |v| u32::from_le_bytes(v.try_into().unwrap())),
                data,
                change: change.map_or(Change::None, Change::Hif),
            });
        }
        if address == 0x80 {
            let expected = (self.options.firmware_bytes - self.cursor).min(2048) as usize;
            let length = (expected + 7) & !3;
            let wire = (length + 4).div_ceil(512) * 512;
            if !write
                || self.phase != Phase::Loading
                || expected == 0
                || quantity != wire
                || payload.len() != quantity
                || u16::from_le_bytes(payload[..2].try_into().unwrap()) as usize != length
                || payload[2..4] != [0; 2]
                || payload[length..length + 4] != [0; 4]
            {
                return Err("Wi-Fi download frame/header/termination unsupported".into());
            }
            return Ok(Operation {
                data: Vec::new(),
                response: 0,
                change: Change::Frame(expected as u32),
            });
        }
        if quantity != 4 {
            return Err("Wi-Fi vendor register requires one word".into());
        }
        if !write {
            let response = match (address, self.phase) {
                (0x8c, Phase::ResetReady) => 0x800,
                (0x8c, Phase::BootloaderReady) => 0x2000,
                (0x8c, Phase::FirmwareReady) => 0x8000,
                (0x88, Phase::Loading | Phase::LoadEnabled) => 0x101,
                (0, Phase::PostDownload | Phase::Configured) => 0x205931,
                (0xc, Phase::PostDownload | Phase::Configured) | (0x10, Phase::Configured) => 0,
                (4, Phase::PostDownload) if self.direct => 0x100,
                _ => return Err("Wi-Fi vendor status read unsupported in current phase".into()),
            };
            return Ok(Operation {
                data: Vec::new(),
                response,
                change: Change::None,
            });
        }
        if payload.len() != 4 {
            return Err("Wi-Fi register write requires four bytes".into());
        }
        let value = u32::from_le_bytes(payload.try_into().unwrap());
        let next = match (address, self.phase, value) {
            (0x8c, Phase::Idle, 0x8000) => Phase::ResetAsserted,
            (0x8c, Phase::ResetAsserted, 0) => Phase::ResetReady,
            (0x90, Phase::ResetReady, n) if n == self.options.firmware_bytes >> 6 => Phase::Sized,
            (0x8c, Phase::Sized, 1) => Phase::BootloaderReady,
            (0x88, Phase::BootloaderReady, 1) => Phase::LoadEnabled,
            (0x84, Phase::LoadEnabled, 0xff900000) => Phase::Loading,
            (0x88, Phase::FirmwareReady, 0) => Phase::PostDownload,
            (0xc, Phase::PostDownload, 0) => Phase::PostDownload,
            (0xc, Phase::Configured, 0) => Phase::Configured,
            (0x14, Phase::PostDownload, 0xffffff0f) if self.direct => Phase::Configured,
            _ => return Err("Unsupported Wi-Fi vendor write/order/value".into()),
        };
        Ok(Operation {
            data: Vec::new(),
            response: 0,
            change: if next == Phase::Configured && self.options.protocol == Protocol::Mt5931 {
                Change::Configure(value)
            } else {
                Change::Phase(PhaseChange(next))
            },
        })
    }
    pub fn commit(&mut self, operation: Operation) {
        match operation.change {
            Change::Configure(mask) => {
                self.phase = Phase::Configured;
                self.endpoint.commit(mt5931::Change::InterruptEnable(mask));
            }
            Change::Hif(change) => {
                if matches!(change, mt5931::Change::LowPower(_)) {
                    self.direct = true;
                }
                self.endpoint.commit(change);
            }
            Change::None => (),
            Change::Direct => self.direct = true,
            Change::Phase(PhaseChange(next)) => self.phase = next,
            Change::Frame(count) => {
                self.cursor += count;
                if self.cursor == self.options.firmware_bytes {
                    self.phase = Phase::FirmwareReady;
                }
            }
            Change::Byte(address, value) => {
                self.bytes[address as usize] = value;
                if address == 2 {
                    self.bytes[3] = value;
                } else if address == 6 && value == 8 {
                    for a in [2, 3, 4, 6] {
                        self.bytes[a] = 0;
                    }
                }
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn arbitrary_download_bytes_and_phases() {
        let mut m = WifiModule::new(Options {
            protocol: Protocol::RomDownload,
            firmware_bytes: 64,
            mt5931: None,
        })
        .unwrap();
        assert!(
            m.vendor(0x84, true, &0xff900000u32.to_le_bytes(), 4)
                .is_err()
        );
        for (a, v) in [
            (0x8c, 0x8000u32),
            (0x8c, 0),
            (0x90, 1),
            (0x8c, 1),
            (0x88, 1),
            (0x84, 0xff900000),
        ] {
            let p = m.vendor(a, true, &v.to_le_bytes(), 4).unwrap();
            m.commit(p);
        }
        let mut payload = [0xa5; 512];
        payload[..4].copy_from_slice(&[68, 0, 0, 0]);
        payload[68..72].fill(0);
        let p = m.vendor(0x80, true, &payload, 512).unwrap();
        assert_eq!(m.cursor, 0);
        m.commit(p);
        assert_eq!(m.vendor(0x8c, false, &[], 4).unwrap().response, 0x8000);
        assert!(m.vendor(0x80, true, &payload, 512).is_err());
    }
    #[test]
    fn enumeration_does_not_grant_vendor_io() {
        let mut m = WifiModule::new(Options {
            protocol: Protocol::Enumeration,
            firmware_bytes: 0,
            mt5931: None,
        })
        .unwrap();
        assert_eq!(m.direct(0).unwrap().response, 0x22);
        assert!(m.direct(0x80000000).is_err());
        let p = m.direct(0x80000000 | (2 << 9) | 2).unwrap();
        m.commit(p);
        assert_eq!(m.direct(3 << 9).unwrap().response, 2);
        assert!(m.vendor(0, false, &[], 4).is_err());
    }
}
