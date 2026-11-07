//! Host oracle for one saved X-T20 MOV. Set FUJIMOD_XT20_MOV_FIXTURE to the
//! stock-written file. This test never supplies pixels to the emulator.
use std::env;
use std::fs;

use fujimod_emulator_image_codecs::h264::decode::Decoder;

#[derive(Clone, Copy)]
struct Atom {
    kind: [u8; 4],
    body: usize,
    end: usize,
}

struct AvcC {
    profile: u8,
    level: u8,
    length_size: usize,
    sps: Vec<u8>,
    pps: Vec<u8>,
}

fn children(data: &[u8], start: usize, end: usize) -> Vec<Atom> {
    let mut rows = Vec::new();
    let mut at = start;
    while at < end {
        assert!(end - at >= 8, "truncated MOV atom");
        let size = u32::from_be_bytes(data[at..at + 4].try_into().unwrap()) as usize;
        assert!(size >= 8 && at + size <= end, "invalid MOV atom size");
        rows.push(Atom {
            kind: data[at + 4..at + 8].try_into().unwrap(),
            body: at + 8,
            end: at + size,
        });
        at += size;
    }
    rows
}

fn one(rows: &[Atom], kind: [u8; 4]) -> Atom {
    let matches: Vec<_> = rows
        .iter()
        .copied()
        .filter(|row| row.kind == kind)
        .collect();
    assert_eq!(
        matches.len(),
        1,
        "expected one {:?}",
        std::str::from_utf8(&kind).unwrap()
    );
    matches[0]
}

fn be32(data: &[u8], at: usize) -> usize {
    u32::from_be_bytes(data[at..at + 4].try_into().unwrap()) as usize
}

fn table(data: &[u8], atom: Atom, width: usize) -> Vec<&[u8]> {
    let count = be32(data, atom.body + 4);
    assert!(
        atom.body + 8 + count * width <= atom.end,
        "MOV table exceeds its atom"
    );
    (0..count)
        .map(|index| &data[atom.body + 8 + index * width..][..width])
        .collect()
}

fn avcc(data: &[u8], atom: Atom) -> AvcC {
    let mut at = atom.body + 6;
    let mut sps = Vec::new();
    for _ in 0..data[atom.body + 5] & 31 {
        let size = u16::from_be_bytes(data[at..at + 2].try_into().unwrap()) as usize;
        at += 2;
        sps.extend_from_slice(&data[at..at + size]);
        at += size;
    }
    let pps_count = data[at];
    at += 1;
    let mut pps = Vec::new();
    for _ in 0..pps_count {
        let mut size = u16::from_be_bytes(data[at..at + 2].try_into().unwrap()) as usize;
        // The stock Fuji writer leaves one zero byte before the PPS length.
        if size == 0
            && at + 3 <= atom.end
            && u16::from_be_bytes(data[at + 1..at + 3].try_into().unwrap()) > 0
        {
            at += 1;
            size = u16::from_be_bytes(data[at..at + 2].try_into().unwrap()) as usize;
        }
        at += 2;
        assert!(0 < size && at + size <= atom.end, "invalid PPS length");
        pps.extend_from_slice(&data[at..at + size]);
        at += size;
    }
    assert_eq!(at, atom.end, "avcC has trailing bytes");
    assert_eq!(
        (sps.len(), pps.len()),
        (1, 1),
        "avcC must carry one SPS and one PPS"
    );
    AvcC {
        profile: data[atom.body + 1],
        level: data[atom.body + 3],
        length_size: (data[atom.body + 4] & 3) as usize + 1,
        sps,
        pps,
    }
}

fn annexb(sample: &[u8], length_size: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < sample.len() {
        assert!(
            at + length_size <= sample.len(),
            "sample ends inside a NAL length"
        );
        let mut size = 0usize;
        for byte in &sample[at..at + length_size] {
            size = size << 8 | *byte as usize;
        }
        at += length_size;
        assert!(
            0 < size && at + size <= sample.len(),
            "sample has an invalid NAL length"
        );
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(&sample[at..at + size]);
        at += size;
    }
    out
}

fn demux(data: &[u8]) -> (Vec<Vec<u8>>, AvcC, usize, usize) {
    let top = children(data, 0, data.len());
    let moov = one(&top, *b"moov");
    let mdat = one(&top, *b"mdat");
    let tracks: Vec<_> = children(data, moov.body, moov.end)
        .into_iter()
        .filter(|row| row.kind == *b"trak")
        .collect();
    let trak = tracks
        .into_iter()
        .find(|trak| {
            let mdia = one(&children(data, trak.body, trak.end), *b"mdia");
            let hdlr = one(&children(data, mdia.body, mdia.end), *b"hdlr");
            &data[hdlr.body + 8..hdlr.body + 12] == b"vide"
        })
        .expect("one video trak");
    let mdia = one(&children(data, trak.body, trak.end), *b"mdia");
    let minf = one(&children(data, mdia.body, mdia.end), *b"minf");
    let stbl = one(&children(data, minf.body, minf.end), *b"stbl");
    let rows = children(data, stbl.body, stbl.end);
    let stsd = one(&rows, *b"stsd");
    let entry = stsd.body + 8;
    assert_eq!(
        &data[entry + 4..entry + 8],
        b"avc1",
        "video entry must be avc1"
    );
    let entry_end = entry + be32(data, entry);
    let avc1_body = entry + 8;
    let width =
        u16::from_be_bytes(data[avc1_body + 0x18..avc1_body + 0x1a].try_into().unwrap()) as usize;
    let height =
        u16::from_be_bytes(data[avc1_body + 0x1a..avc1_body + 0x1c].try_into().unwrap()) as usize;
    let avc = avcc(
        data,
        one(&children(data, avc1_body + 78, entry_end), *b"avcC"),
    );

    let stsz = one(&rows, *b"stsz");
    let fixed = be32(data, stsz.body + 4);
    let count = be32(data, stsz.body + 8);
    let sizes: Vec<usize> = if fixed != 0 {
        vec![fixed; count]
    } else {
        (0..count)
            .map(|index| be32(data, stsz.body + 12 + index * 4))
            .collect()
    };
    let stco = one(&rows, *b"stco");
    let chunk_offsets: Vec<usize> = table(data, stco, 4)
        .iter()
        .map(|row| u32::from_be_bytes((*row).try_into().unwrap()) as usize)
        .collect();
    let stsc = one(&rows, *b"stsc");
    let mapping: Vec<(usize, usize)> = table(data, stsc, 12)
        .iter()
        .map(|row| {
            (
                u32::from_be_bytes(row[0..4].try_into().unwrap()) as usize,
                u32::from_be_bytes(row[4..8].try_into().unwrap()) as usize,
            )
        })
        .collect();
    let stts = one(&rows, *b"stts");
    assert_eq!(
        table(data, stts, 8)
            .iter()
            .map(|row| u32::from_be_bytes(row[0..4].try_into().unwrap()) as usize)
            .sum::<usize>(),
        count,
        "stts must time every stsz sample"
    );

    let mut offsets = Vec::new();
    let mut index = 0;
    for (chunk, mut cursor) in chunk_offsets.into_iter().enumerate() {
        let per = mapping
            .iter()
            .rev()
            .find(|(first, _)| *first <= chunk + 1)
            .expect("stsc covers every chunk")
            .1;
        for _ in 0..per {
            assert!(index < sizes.len(), "stsc describes more samples than stsz");
            offsets.push(cursor);
            cursor += sizes[index];
            index += 1;
        }
    }
    assert_eq!(index, sizes.len(), "stsc describes fewer samples than stsz");

    let mut units: Vec<Vec<u8>> = offsets
        .into_iter()
        .zip(&sizes)
        .map(|(offset, size)| {
            assert!(
                offset >= mdat.body && offset + size <= mdat.end,
                "sample outside mdat"
            );
            annexb(&data[offset..offset + size], avc.length_size)
        })
        .collect();
    let mut first = vec![0, 0, 0, 1];
    first.extend_from_slice(&avc.sps);
    first.extend_from_slice(&[0, 0, 0, 1]);
    first.extend_from_slice(&avc.pps);
    first.extend_from_slice(&units[0]);
    units[0] = first;
    (units, avc, width, height)
}

#[test]
fn stock_movie_demuxes_to_the_declared_samples_and_decodes() {
    let Some(path) = env::var_os("FUJIMOD_XT20_MOV_FIXTURE") else {
        eprintln!("skip: set FUJIMOD_XT20_MOV_FIXTURE to a stock-written X-T20 MOV");
        return;
    };
    let data = fs::read(&path).expect("read the selected MOV");
    let (units, avc, width, height) = demux(&data);
    assert_eq!(avc.profile, 100, "stock X-T20 avcC profile");
    assert_eq!(avc.level, 40, "stock X-T20 avcC level");
    let mut decoder = Decoder::new_movie(&[]).expect("host movie decoder");
    let mut decoded = 0;
    for unit in &units {
        let picture = decoder
            .decode_movie_unit(unit)
            .expect("decode one stock access unit");
        assert_eq!(
            (picture.sequence.width, picture.sequence.height),
            (width, height)
        );
        decoded += 1;
    }
    assert_eq!(decoded, units.len());
}
