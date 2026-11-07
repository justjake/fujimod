//! RSN authenticator for one WPA2-PSK station.
//!
//! The chip transports EAPOL; PBKDF2, PTK derivation and key-data wrapping
//! happen in the access point. CCMP on data frames remains the chip model's
//! responsibility and is not implemented in this module.
use aes_kw::{KeyInit as WrapKeyInit, KwAes128};
use hmac::{Hmac, KeyInit, Mac};
use pbkdf2::pbkdf2_hmac;
use serde::{Deserialize, Serialize};
use sha1::Sha1;

const EAPOL_KEY: u8 = 3;
const DESCRIPTOR_VERSION: u16 = 2;
const KEY_TYPE_PAIRWISE: u16 = 1 << 3;
const KEY_INSTALL: u16 = 1 << 6;
const KEY_ACK: u16 = 1 << 7;
const KEY_MIC: u16 = 1 << 8;
const KEY_SECURE: u16 = 1 << 9;
const KEY_ENCRYPTED: u16 = 1 << 12;
const EAPOL_HEADER: usize = 4;
const KEY_BODY: usize = 95;
const KEY_REPLAY_OFFSET: usize = EAPOL_HEADER + 5;
const KEY_NONCE_OFFSET: usize = EAPOL_HEADER + 13;
const KEY_MIC_OFFSET: usize = EAPOL_HEADER + 77;
#[cfg(test)]
const KEY_DATA_OFFSET: usize = EAPOL_HEADER + KEY_BODY;

/// RSN element for CCMP pairwise and group ciphers and PSK authentication.
pub const RSN_ELEMENT: [u8; 22] = [
    48, 20, 1, 0, 0x00, 0x0f, 0xac, 4, 0x00, 0x0f, 0xac, 4, 1, 0, 0x00, 0x0f, 0xac, 2, 0x00, 0x00,
    0x00, 0x00,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Deserialize, Serialize)]
pub enum Phase {
    #[default]
    WaitingForMessage2,
    WaitingForMessage4,
    Complete,
    Failed,
}

/// WPA2-PSK authenticator state. Nonces and the group key are fixed per
/// exchange so checkpointing preserves the same handshake.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct Authenticator {
    pub phase: Phase,
    pmk: [u8; 32],
    anonce: [u8; 32],
    snonce: [u8; 32],
    kck: [u8; 16],
    kek: [u8; 16],
    gtk: [u8; 16],
    replay_counter: [u8; 8],
    sequence: u8,
}

impl Authenticator {
    pub fn new(passphrase: &str, ssid: &str) -> Result<Self, String> {
        if !(8..=63).contains(&passphrase.len()) {
            return Err("WPA2 passphrase must be 8..=63 bytes".into());
        }
        let mut pmk = [0u8; 32];
        pbkdf2_hmac::<Sha1>(passphrase.as_bytes(), ssid.as_bytes(), 4096, &mut pmk);
        let mut state = Self {
            phase: Phase::WaitingForMessage2,
            pmk,
            replay_counter: [0, 0, 0, 0, 0, 0, 0, 1],
            ..Self::default()
        };
        for (index, byte) in state.anonce.iter_mut().enumerate() {
            *byte = (index as u8).wrapping_mul(7).wrapping_add(0x41);
        }
        for (index, byte) in state.gtk.iter_mut().enumerate() {
            *byte = (index as u8).wrapping_mul(11).wrapping_add(0x81);
        }
        Ok(state)
    }

    /// Builds an EAPOL-Key message 1/4, or retransmits 1/4 with its MIC when
    /// waiting for a repeated message 2/4.
    pub fn message1(&mut self, station: [u8; 6], access_point: [u8; 6]) -> Vec<u8> {
        let mut key_info = DESCRIPTOR_VERSION | KEY_TYPE_PAIRWISE | KEY_ACK;
        if self.phase == Phase::WaitingForMessage4 {
            key_info |= KEY_MIC;
        }
        let mut frame = self.key_frame(key_info, 0, &self.anonce, &[0; 16], &[]);
        if key_info & KEY_MIC != 0 {
            let mic = eapol_mic(&self.kck, &frame[EAPOL_HEADER..]);
            frame[KEY_MIC_OFFSET..KEY_MIC_OFFSET + 16].copy_from_slice(&mic);
        }
        ethernet(access_point, station, &frame)
    }

    /// Consumes message 2/4 and returns message 3/4. A MIC mismatch or a
    /// malformed body fails the exchange without producing a frame.
    pub fn message2(
        &mut self,
        frame: &[u8],
        station: [u8; 6],
        access_point: [u8; 6],
    ) -> Option<Vec<u8>> {
        if self.phase != Phase::WaitingForMessage2 {
            return None;
        }
        let eapol = eapol_payload(frame)?;
        let key_info = u16::from_be_bytes([eapol[5], eapol[6]]);
        if eapol[1] != EAPOL_KEY {
            eprintln!("bad type");
            return None;
        }
        if key_info & 7 != DESCRIPTOR_VERSION {
            eprintln!("bad desc {key_info:#x}");
            return None;
        }
        if key_info & (KEY_ACK | KEY_MIC) != KEY_MIC {
            eprintln!("bad bits {key_info:#x}");
            return None;
        }
        self.snonce
            .copy_from_slice(&eapol[KEY_NONCE_OFFSET..KEY_NONCE_OFFSET + 32]);
        self.replay_counter
            .copy_from_slice(&eapol[KEY_REPLAY_OFFSET..KEY_REPLAY_OFFSET + 8]);
        let ptk = derive_ptk(&self.pmk, station, access_point, &self.anonce, &self.snonce);
        self.kck.copy_from_slice(&ptk[..16]);
        self.kek.copy_from_slice(&ptk[16..32]);
        let received_mic = eapol[KEY_MIC_OFFSET..KEY_MIC_OFFSET + 16].to_vec();
        let mut verified = eapol.to_vec();
        verified[KEY_MIC_OFFSET..KEY_MIC_OFFSET + 16].fill(0);
        if !mac_eq(&hmac(&self.kck, verified.as_slice())[..16], &received_mic) {
            self.phase = Phase::Failed;
            return None;
        }

        // The RSN element must match the beacon exactly. The group-key KDE
        // carries its own two reserved bytes before the GTK, then the key
        // data is padded to an eight-byte boundary before AES key wrap.
        let mut key_data = RSN_ELEMENT.to_vec();
        let mut kde = vec![221, 22, 0x00, 0x0f, 0xac, 4, 1, 0];
        kde.extend_from_slice(&[0; 6]);
        kde.extend_from_slice(&self.gtk);
        key_data.extend_from_slice(&kde);
        while !key_data.len().is_multiple_of(8) {
            key_data.push(0xdd);
        }
        let wrapped = wrap(&self.kek, &key_data).ok()?;
        let key_info = DESCRIPTOR_VERSION
            | KEY_TYPE_PAIRWISE
            | KEY_ACK
            | KEY_MIC
            | KEY_INSTALL
            | KEY_SECURE
            | KEY_ENCRYPTED;
        self.sequence = 1;
        increment(&mut self.replay_counter);
        let mut reply = self.key_frame(key_info, 16, &self.anonce, &[0; 8], &wrapped);
        let mic = eapol_mic(&self.kck, &reply[EAPOL_HEADER..]);
        reply[KEY_MIC_OFFSET..KEY_MIC_OFFSET + 16].copy_from_slice(&mic);
        self.phase = Phase::WaitingForMessage4;
        Some(ethernet(access_point, station, &reply))
    }

    /// Consumes message 4/4 and reports whether the pairwise handshake and its
    /// group-key installation completed.
    pub fn message4(&mut self, frame: &[u8]) -> bool {
        if self.phase != Phase::WaitingForMessage4 {
            return false;
        }
        let Some(eapol) = eapol_payload(frame) else {
            return false;
        };
        let key_info = u16::from_be_bytes([eapol[5], eapol[6]]);
        if eapol[1] != EAPOL_KEY
            || key_info & 7 != DESCRIPTOR_VERSION
            || key_info & (KEY_ACK | KEY_MIC | KEY_TYPE_PAIRWISE) != KEY_MIC | KEY_TYPE_PAIRWISE
        {
            return false;
        }
        let received_mic = eapol[KEY_MIC_OFFSET..KEY_MIC_OFFSET + 16].to_vec();
        let mut verified = eapol.to_vec();
        verified[KEY_MIC_OFFSET..KEY_MIC_OFFSET + 16].fill(0);
        let complete = mac_eq(&hmac(&self.kck, &verified)[..16], &received_mic)
            && verified[KEY_REPLAY_OFFSET..KEY_REPLAY_OFFSET + 8] == self.replay_counter;
        self.phase = if complete {
            Phase::Complete
        } else {
            Phase::Failed
        };
        complete
    }

    pub fn sequence(&self) -> u8 {
        self.sequence
    }

    fn key_frame(
        &self,
        key_info: u16,
        key_length: u16,
        nonce: &[u8; 32],
        iv: &[u8],
        key_data: &[u8],
    ) -> Vec<u8> {
        let mut body = vec![2];
        body.extend_from_slice(&key_info.to_be_bytes());
        body.extend_from_slice(&key_length.to_be_bytes());
        body.extend_from_slice(&self.replay_counter);
        body.extend_from_slice(nonce);
        body.extend_from_slice(iv);
        body.extend_from_slice(&[0; 8]);
        body.extend_from_slice(&[0; 8]);
        body.extend_from_slice(&[0; 16]);
        body.extend_from_slice(&(key_data.len() as u16).to_be_bytes());
        body.extend_from_slice(key_data);
        let mut eapol = vec![2, EAPOL_KEY];
        eapol.extend_from_slice(&(body.len() as u16).to_be_bytes());
        eapol.extend_from_slice(&body);
        eapol
    }
}

fn eapol_payload(frame: &[u8]) -> Option<&[u8]> {
    if frame.len() < 14 + EAPOL_HEADER + KEY_BODY || frame[12..14] != [0x88, 0x8e] {
        return None;
    }
    let eapol = &frame[14..];
    let length = u16::from_be_bytes([eapol[2], eapol[3]]) as usize;
    (length >= KEY_BODY && EAPOL_HEADER + length <= eapol.len()).then_some(eapol)
}

fn ethernet(destination: [u8; 6], source: [u8; 6], payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(14 + payload.len());
    frame.extend_from_slice(&destination);
    frame.extend_from_slice(&source);
    frame.extend_from_slice(&[0x88, 0x8e]);
    frame.extend_from_slice(payload);
    frame
}

fn hmac(key: &[u8], value: &[u8]) -> Vec<u8> {
    let mut mac =
        <Hmac<Sha1> as KeyInit>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(value);
    mac.finalize().into_bytes().to_vec()
}

fn eapol_mic(kck: &[u8], eapol: &[u8]) -> [u8; 16] {
    hmac(kck, eapol)[..16].try_into().unwrap()
}

fn mac_eq(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0u8, |difference, (a, b)| difference | (a ^ b))
            == 0
}

fn derive_ptk(
    pmk: &[u8],
    station: [u8; 6],
    access_point: [u8; 6],
    anonce: &[u8; 32],
    snonce: &[u8; 32],
) -> [u8; 48] {
    let mut data = Vec::with_capacity(76);
    if station < access_point {
        data.extend_from_slice(&station);
        data.extend_from_slice(&access_point);
    } else {
        data.extend_from_slice(&access_point);
        data.extend_from_slice(&station);
    }
    if anonce < snonce {
        data.extend_from_slice(anonce);
        data.extend_from_slice(snonce);
    } else {
        data.extend_from_slice(snonce);
        data.extend_from_slice(anonce);
    }
    let mut ptk = [0u8; 48];
    prf(pmk, b"Pairwise key expansion", &data, &mut ptk);
    ptk
}

/// IEEE 802.11i SHA-1 PRF, producing 160-bit HMAC blocks.
fn prf(key: &[u8], label: &[u8], data: &[u8], output: &mut [u8]) {
    let mut offset = 0;
    let mut counter = 0u8;
    while offset < output.len() {
        let mut input = Vec::with_capacity(label.len() + data.len() + 2);
        input.extend_from_slice(label);
        input.push(0);
        input.extend_from_slice(data);
        input.push(counter);
        let block = hmac(key, &input);
        let count = (output.len() - offset).min(block.len());
        output[offset..offset + count].copy_from_slice(&block[..count]);
        offset += count;
        counter = counter.wrapping_add(1);
    }
}

fn wrap(key: &[u8; 16], plaintext: &[u8]) -> Result<Vec<u8>, String> {
    let cipher = KwAes128::new(&(*key).into());
    let mut output = vec![0u8; plaintext.len() + 8];
    cipher
        .wrap_key(plaintext, &mut output)
        .map_err(|error| format!("AES key wrap failed: {error:?}"))?;
    Ok(output)
}

fn increment(counter: &mut [u8; 8]) {
    for byte in counter.iter_mut().rev() {
        *byte = byte.wrapping_add(1);
        if *byte != 0 {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATION: [u8; 6] = [2, 0, 0, 0, 0, 9];
    const ACCESS_POINT: [u8; 6] = [2, 0, 0, 0, 0, 1];

    fn derive_pbkdf2(password: &[u8], salt: &[u8], rounds: u32, length: usize) -> Vec<u8> {
        let mut output = vec![0u8; length];
        pbkdf2_hmac::<Sha1>(password, salt, rounds, &mut output);
        output
    }

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len() / 2)
            .map(|index| u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).unwrap())
            .collect()
    }

    /// PBKDF2-HMAC-SHA1 vectors from RFC 6070 section 2.
    #[test]
    fn pbkdf2_matches_rfc6070_vectors() {
        assert_eq!(
            derive_pbkdf2(b"password", b"salt", 1, 20),
            hex("0c60c80f961f0e71f3a9b524af6012062fe037a6")
        );
        assert_eq!(
            derive_pbkdf2(b"password", b"salt", 2, 20),
            hex("ea6c014dc72d6f8ccd1ed92ace1d41f0d8de8957")
        );
        assert_eq!(
            derive_pbkdf2(b"password", b"salt", 4096, 20),
            hex("4b007901b765489abead49d926f721d065a429c1")
        );
        assert_eq!(
            derive_pbkdf2(
                b"passwordPASSWORDpassword",
                b"saltSALTsaltSALTsaltSALTsaltSALTsalt",
                4096,
                25
            ),
            hex("3d2eec4fe41c849b80c8d83662c0e44a8b291a964cf2f07038")
        );
        assert_eq!(
            derive_pbkdf2(b"pass\0word", b"sa\0lt", 4096, 16),
            hex("56fa6aa75548099dcc37d7f03425e0c3")
        );
    }

    /// The IEEE 802.11i PSK test vector (passphrase "password", SSID "IEEE").
    #[test]
    fn pbkdf2_matches_ieee_80211i_psk_vector() {
        assert_eq!(
            derive_pbkdf2(b"password", b"IEEE", 4096, 32),
            hex("f42c6fc52df0ebef9ebb4b90b38a5f902e83fe1b135a70e23aed762e9710a12e")
        );
    }

    #[test]
    fn rfc3394_vector() {
        let key = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
            0x0e, 0x0f,
        ];
        let plaintext = [
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff,
        ];
        assert_eq!(
            wrap(&key, &plaintext).unwrap(),
            [
                0x1f, 0xa6, 0x8b, 0x0a, 0x81, 0x12, 0xb4, 0x47, 0xae, 0xf3, 0x4b, 0xd8, 0xfb, 0x5a,
                0x7b, 0x82, 0x9d, 0x3e, 0x86, 0x23, 0x71, 0xd2, 0xcf, 0xe5,
            ]
        );
    }

    fn station_message(key: &[u8]) -> (Vec<u8>, [u8; 16]) {
        let snonce = [0x55u8; 32];
        let mut pmk = [0u8; 32];
        pbkdf2_hmac::<Sha1>(key, b"Camera OTA", 4096, &mut pmk);
        let anonce = std::array::from_fn(|index| (index as u8).wrapping_mul(7).wrapping_add(0x41));
        let ptk = derive_ptk(&pmk, STATION, ACCESS_POINT, &anonce, &snonce);
        let state = Authenticator {
            replay_counter: [0, 0, 0, 0, 0, 0, 0, 1],
            ..Authenticator::default()
        };
        let mut eapol = state.key_frame(
            DESCRIPTOR_VERSION | KEY_TYPE_PAIRWISE | KEY_MIC,
            0,
            &snonce,
            &[0; 16],
            &RSN_ELEMENT,
        );
        let mic = eapol_mic(&ptk[..16], &eapol);
        eapol[KEY_MIC_OFFSET..KEY_MIC_OFFSET + 16].copy_from_slice(&mic);
        (
            ethernet(ACCESS_POINT, STATION, &eapol),
            ptk[..16].try_into().unwrap(),
        )
    }

    #[test]
    fn correct_psk_completes_four_way_handshake() {
        let mut authenticator = Authenticator::new("password123", "Camera OTA").unwrap();
        let message1 = authenticator.message1(STATION, ACCESS_POINT);
        assert_eq!(message1[..6], ACCESS_POINT);
        let (message2, kck) = station_message(b"password123");
        let message3 = authenticator
            .message2(&message2, STATION, ACCESS_POINT)
            .expect("message 2 should be accepted");
        assert_eq!(message3[12..14], [0x88, 0x8e]);
        let eapol = &message3[14..];
        assert_eq!(u16::from_be_bytes([eapol[5], eapol[6]]), 0x13ca);
        assert!(eapol[KEY_DATA_OFFSET..].len().is_multiple_of(8));

        let mut message4 = vec![2, EAPOL_KEY, 0, 95, 2];
        message4.extend_from_slice(
            &(DESCRIPTOR_VERSION | KEY_TYPE_PAIRWISE | KEY_MIC | KEY_SECURE).to_be_bytes(),
        );
        message4.extend_from_slice(&0u16.to_be_bytes());
        message4.extend_from_slice(&eapol[KEY_REPLAY_OFFSET..KEY_REPLAY_OFFSET + 8]);
        message4.extend_from_slice(&eapol[KEY_NONCE_OFFSET..KEY_NONCE_OFFSET + 32]);
        message4.extend_from_slice(&[0; 16]);
        message4.extend_from_slice(&[0; 8]);
        message4.extend_from_slice(&[0; 8]);
        message4.extend_from_slice(&[0; 16]);
        message4.extend_from_slice(&0u16.to_be_bytes());
        let mic = eapol_mic(&kck, &message4);
        message4[KEY_MIC_OFFSET..KEY_MIC_OFFSET + 16].copy_from_slice(&mic);
        let message4 = ethernet(ACCESS_POINT, STATION, &message4);
        assert!(authenticator.message4(&message4));
        assert_eq!(authenticator.phase, Phase::Complete);
    }

    #[test]
    fn wrong_psk_fails_on_message_two_mic() {
        let mut authenticator = Authenticator::new("wrongpass", "Camera OTA").unwrap();
        assert!(
            authenticator
                .message2(&station_message(b"password123").0, STATION, ACCESS_POINT)
                .is_none()
        );
        assert_eq!(authenticator.phase, Phase::Failed);
    }
}
