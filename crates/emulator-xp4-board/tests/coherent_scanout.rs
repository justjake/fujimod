//! Declared data-cache coherence ranges: the display controller scans out
//! the memory view, so framebuffer lines the processor wrote but did not
//! clean still show their old content.
use fujimod_emulator_xp4_board::{Definition, Session};
use serde_json::json;
use std::sync::Mutex;

const CBAR: u32 = 0x2000_0000;
const LCDC: u32 = 0xfffa_9000;
const CODE: u32 = 0x1000;
const PARK: u32 = 0x2000;
const FRAME: u32 = 0x4000;
const LINE: u32 = 32;
const ROWS: u32 = 4;
// One framebuffer line per row: eight 4-byte pixels.
const OLD: u32 = 0x4050_60ff;
const NEW: u32 = 0x1020_30ff;

// The environment is process-wide: sessions are opened one at a time.
static ENVIRONMENT: Mutex<()> = Mutex::new(());

fn movw(rd: u32, value: u32) -> u32 {
    0xe300_0000 | ((value >> 12) << 16) | (rd << 12) | (value & 0xfff)
}
fn movt(rd: u32, value: u32) -> u32 {
    0xe340_0000 | ((value >> 12) << 16) | (rd << 12) | (value & 0xfff)
}
fn load(rd: u32, value: u32) -> [u32; 2] {
    [movw(rd, value & 0xffff), movt(rd, value >> 16)]
}
fn str_at(rt: u32, rn: u32, offset: u32) -> u32 {
    0xe580_0000 | (rn << 16) | (rt << 12) | offset
}

/// A routine that programs the display controller for an 8x4 picture at
/// `FRAME`, fills the picture with `NEW`, cleans the data-cache lines at
/// `clean`, enables the display and spins.
fn routine(clean: &[u32]) -> Vec<u8> {
    let mut code = Vec::new();
    code.extend(load(0, LCDC));
    for (offset, value) in [
        (0x10, 12),
        (0x14, 0x10008),
        (0x18, 0x10002),
        (0x1c, 7),
        (0x20, 0x10004),
        (0x24, 0x10001),
        (0x44, FRAME),
        (0x4c, FRAME),
        (0x50, 0x10008),
        (0x54, 0x40008),
        (0x5c, 0x40008),
        (0x60, 0x1000_1000),
        (8, 0x11000),
    ] {
        code.extend(load(1, value));
        code.push(str_at(1, 0, offset));
    }
    code.extend(load(2, FRAME));
    code.extend(load(3, NEW));
    code.extend(load(4, LINE / 4 * ROWS));
    code.push(0xe482_3004); // 1: str r3, [r2], #4
    code.push(0xe254_4001); // subs r4, r4, #1
    code.push(0x1aff_fffc); // bne 1b
    for &address in clean {
        code.extend(load(2, address));
        code.push(0xee07_2f3a); // mcr p15, 0, r2, c7, c10, 1 (DCCMVAC)
    }
    code.extend(load(1, 1));
    code.push(str_at(1, 0, 4)); // enable
    code.push(0xeaff_fffe); // b .
    code.iter().flat_map(|w| w.to_le_bytes()).collect()
}

/// The first scanned-out frame's pixel word per row.
fn rows(coherent: bool, clean: &[u32]) -> Vec<u32> {
    rows_on(1, coherent, clean)
}

/// `cores` processors; the others spin at `PARK`. Declared ranges select
/// counted execution on their own.
fn rows_on(cores: usize, coherent: bool, clean: &[u32]) -> Vec<u32> {
    let mut initial = vec![json!([["Pc", CODE]])];
    initial.resize(cores, json!([["Pc", PARK]]));
    let mut definition = json!({
        "ram_size": 65536, "cores": cores, "discovery": false, "cbar": CBAR,
        "initial_registers": initial,
        "lcdc": [[LCDC, 2, 117, 118, "abgr8888-straight", null]]
    });
    if coherent {
        definition["coherent_ranges"] =
            json!({"line_bytes": LINE, "ranges": [[FRAME, FRAME + LINE * ROWS]]});
    }
    let definition: Definition = serde_json::from_value(definition).unwrap();
    let session = {
        let _environment = ENVIRONMENT.lock().unwrap();
        // SAFETY: sessions in this binary are opened under the lock above.
        if cores == 1 {
            unsafe { std::env::set_var("FUJIMOD_XP4_COUNTED", "1") };
        }
        let session = Session::new(definition, None, vec![]);
        unsafe { std::env::remove_var("FUJIMOD_XP4_COUNTED") };
        session.unwrap()
    };
    session
        .write(PARK as usize, &0xeaff_fffeu32.to_le_bytes())
        .unwrap();
    session.write(CODE as usize, &routine(clean)).unwrap();
    let old: Vec<u8> = (0..LINE * ROWS / 4)
        .flat_map(|_| OLD.to_le_bytes())
        .collect();
    session.write(FRAME as usize, &old).unwrap();
    session.run_board(20_000, 64).unwrap();
    let state = session.state(false);
    assert!(state.failure.is_none(), "{:?}", state.failure);
    let frames = session.scanout_frames();
    assert_eq!(frames.len(), 1, "no frame scanned out");
    let frame = &frames[0];
    assert_eq!((frame.width, frame.height), (8, ROWS));
    (0..ROWS as usize)
        .map(|row| {
            let pixel = &frame.rgba[row * 32..row * 32 + 4];
            // Unique per row: every pixel of a line is the same.
            assert!(
                frame.rgba[row * 32..(row + 1) * 32]
                    .chunks(4)
                    .all(|p| p == pixel)
            );
            u32::from_be_bytes([pixel[0], pixel[1], pixel[2], 0xff])
        })
        .collect()
}

const OLD_PIXEL: u32 = OLD | 0xff;
const NEW_PIXEL: u32 = NEW | 0xff;

#[test]
fn uncleaned_lines_scan_out_their_old_content() {
    assert_eq!(rows(true, &[]), [OLD_PIXEL; 4]);
}

#[test]
fn cleaning_the_top_half_shows_new_lines_above_old_ones() {
    let (old, new) = (OLD_PIXEL, NEW_PIXEL);
    assert_eq!(rows(true, &[FRAME, FRAME + LINE]), [new, new, old, old]);
}

#[test]
fn without_declared_ranges_the_whole_frame_is_new() {
    assert_eq!(rows(false, &[]), [NEW_PIXEL; 4]);
}

#[test]
fn several_cores_share_one_model_without_the_counted_switch() {
    assert_eq!(rows_on(2, true, &[]), [OLD_PIXEL; 4]);
    let (old, new) = (OLD_PIXEL, NEW_PIXEL);
    assert_eq!(
        rows_on(2, true, &[FRAME, FRAME + LINE]),
        [new, new, old, old]
    );
}
