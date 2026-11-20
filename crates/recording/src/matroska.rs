//! Streaming Matroska subset for one uncompressed RGB track (RFC 9559).
use crate::Frame;
use std::io::{self, Write};

// A timestamp tick is 100 microseconds, well under half a frame at all
// supported rates. FFmpeg rounds each timestamp back to its CFR frame index.
const TIMESTAMP_SCALE: u64 = 100_000;
const TICKS_PER_SECOND: u64 = 1_000_000_000 / TIMESTAMP_SCALE;

pub struct Matroska {
    fps: u64,
}

impl Matroska {
    pub fn open(output: &mut impl Write, frame: &Frame, fps: u32) -> io::Result<Self> {
        let fps = u64::from(fps);
        let mut header = Vec::new();
        integer(&mut header, 0x4286, 1); // EBMLVersion
        integer(&mut header, 0x42f7, 1); // EBMLReadVersion
        integer(&mut header, 0x42f2, 4); // EBMLMaxIDLength
        integer(&mut header, 0x42f3, 8); // EBMLMaxSizeLength
        element(&mut header, 0x4282, b"matroska"); // DocType
        integer(&mut header, 0x4287, 4); // DocTypeVersion
        integer(&mut header, 0x4285, 2); // DocTypeReadVersion
        let mut stream = Vec::new();
        element(&mut stream, 0x1a45dfa3, &header); // EBML
        stream.extend_from_slice(&[0x18, 0x53, 0x80, 0x67, 0xff]); // Segment, unknown size

        let mut info = Vec::new();
        integer(&mut info, 0x2ad7b1, TIMESTAMP_SCALE);
        element(&mut info, 0x4d80, b"FujiMod"); // MuxingApp
        element(&mut info, 0x5741, b"FujiMod"); // WritingApp
        element(&mut stream, 0x1549a966, &info);

        let mut video = Vec::new();
        integer(&mut video, 0xb0, u64::from(frame.width())); // PixelWidth
        integer(&mut video, 0xba, u64::from(frame.height())); // PixelHeight
        element(&mut video, 0x2eb524, b"RGB\x18"); // UncompressedFourCC (RGB24)
        let mut track = Vec::new();
        integer(&mut track, 0xd7, 1); // TrackNumber
        integer(&mut track, 0x73c5, 1); // TrackUID
        integer(&mut track, 0x83, 1); // TrackType (video)
        element(&mut track, 0x86, b"V_UNCOMPRESSED"); // CodecID
        element(&mut track, 0x22b59c, b"und"); // Language
        integer(&mut track, 0x23e383, (1_000_000_000 + fps / 2) / fps); // DefaultDuration
        element(&mut track, 0xe0, &video);
        let mut tracks = Vec::new();
        element(&mut tracks, 0xae, &track);
        element(&mut stream, 0x1654ae6b, &tracks);
        output.write_all(&stream)?;
        Ok(Self { fps })
    }

    pub fn write(&self, output: &mut impl Write, index: u64, frame: &Frame) -> io::Result<()> {
        let ticks = (index * TICKS_PER_SECOND + self.fps / 2) / self.fps;
        let mut timestamp = Vec::with_capacity(10);
        integer(&mut timestamp, 0xe7, ticks); // Cluster Timestamp
        let mut block = Vec::with_capacity(10);
        element_header(&mut block, 0xa3, frame.pixels().len() + 4); // SimpleBlock
        block.extend_from_slice(&[0x81, 0x00, 0x00, 0x80]); // Track 1, relative time 0, keyframe
        let mut cluster = Vec::with_capacity(8);
        element_header(
            &mut cluster,
            0x1f43b675,
            timestamp.len() + block.len() + frame.pixels().len(),
        );
        cluster.extend_from_slice(&timestamp);
        cluster.extend_from_slice(&block);
        output.write_all(&cluster)?;
        output.write_all(frame.pixels())
    }
}

fn integer(output: &mut Vec<u8>, id: u32, value: u64) {
    let bytes = value.to_be_bytes();
    let size = (u64::BITS - value.leading_zeros()).div_ceil(8).max(1) as usize;
    element(output, id, &bytes[8 - size..]);
}

fn element(output: &mut Vec<u8>, id: u32, contents: &[u8]) {
    element_header(output, id, contents.len());
    output.extend_from_slice(contents);
}

fn element_header(output: &mut Vec<u8>, id: u32, size: usize) {
    let id_bytes = id.to_be_bytes();
    output.extend_from_slice(&id_bytes[id.leading_zeros() as usize / 8..]);
    let size = size as u64;
    // All-one data bits are reserved for unknown size.
    let width = (u64::BITS - (size + 1).leading_zeros()).div_ceil(7) as usize;
    let size_bytes = (size | (1 << (width * 7))).to_be_bytes();
    output.extend_from_slice(&size_bytes[8 - width..]);
}
