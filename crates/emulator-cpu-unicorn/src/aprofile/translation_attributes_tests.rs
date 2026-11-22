use super::translation_attributes::{Walk, cacheable_mapping};

#[test]
fn attributes_follow_the_tex_c_b_encoding() {
    // Strongly ordered, device, write-through, write-back.
    assert!(!cacheable_mapping(0, false, false));
    assert!(!cacheable_mapping(0, false, true));
    assert!(cacheable_mapping(0, true, false));
    assert!(cacheable_mapping(0, true, true));
    // Normal non-cacheable and shared device.
    assert!(!cacheable_mapping(1, false, false));
    assert!(!cacheable_mapping(2, false, false));
    // Outer/inner policies: cacheable at either level.
    assert!(cacheable_mapping(5, true, true));
    assert!(cacheable_mapping(4, true, false));
    assert!(cacheable_mapping(5, false, false));
    assert!(!cacheable_mapping(4, false, false));
}

fn walk() -> Walk {
    Walk {
        ttbr0: 0x8000,
        ttbr1: 0,
        ttbcr: 0,
        tex_remap: false,
    }
}

#[test]
fn sections_and_both_page_sizes_are_read() {
    let tables = |address: u64| -> Result<u32, String> {
        Ok(match address {
            // Section 0: write-back. Section 1: normal non-cacheable.
            0x8000 => 0x0000_0c0e,
            0x8004 => 0x0010_1c02,
            // Section 2: page table at 0x9000.
            0x8008 => 0x0000_9001,
            // Page 0 small non-cacheable (TEX 001), page 1 small
            // write-back, page 2 large device, page 3 fault.
            0x9000 => 0x0020_0072,
            0x9004 => 0x0020_103e,
            0x9008 => 0x0020_0005,
            _ => 0,
        })
    };
    let at = |va| walk().cacheable(va, tables).unwrap();
    assert!(at(0x0000_0000));
    assert!(!at(0x0010_0000));
    assert!(!at(0x0020_0000));
    assert!(at(0x0020_1000));
    assert!(!at(0x0020_2000));
    assert!(at(0x0020_3000));
}

#[test]
fn ttbr1_serves_the_upper_addresses() {
    let walk = Walk {
        ttbr0: 0x8000,
        ttbr1: 0xc000,
        ttbcr: 1,
        tex_remap: false,
    };
    let tables = |address: u64| -> Result<u32, String> {
        Ok(match address {
            0x8000 => 0x0000_0c0e,
            // Index 0x800 of TTBR1's table: section, non-cacheable.
            0xe000 => 0x8000_1c02,
            _ => 0,
        })
    };
    assert!(walk.cacheable(0, tables).unwrap());
    assert!(!walk.cacheable(0x8000_0000, tables).unwrap());
}

#[test]
fn unsupported_formats_are_refused() {
    let none = |_| Ok(0);
    let mut long = walk();
    long.ttbcr = 1 << 31;
    assert!(long.cacheable(0, none).is_err());
    let mut remap = walk();
    remap.tex_remap = true;
    assert!(remap.cacheable(0, none).is_err());
}
