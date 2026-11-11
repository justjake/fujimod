use super::*;

const ENABLED: u32 = 0b101;

#[test]
fn region_attributes_select_the_l1_policy() {
    let region = |access| [(0, (29 << 1) | 1, access)];
    for (access, expected) in [
        (0x30b, Policy::WriteBackAllocate), // TEX 001, C B: write-back write-allocate
        (0x303, Policy::WriteBack),         // TEX 000, C B: write-back
        (0x302, Policy::WriteThrough),      // TEX 000, C
        (0x308, Policy::NonCacheable),      // TEX 001: Normal non-cacheable
        (0x301, Policy::NonCacheable),      // Device
        (0x30f, Policy::NonCacheable),      // Shareable write-back
        (0x321, Policy::WriteBackAllocate), // TEX 1BB, inner write-back write-allocate
    ] {
        assert_eq!(
            policy(ENABLED, &region(access), 0x1234),
            expected,
            "{access:#x}"
        );
    }
    assert_eq!(policy(0b001, &region(0x30b), 0x1234), Policy::NonCacheable);
    assert_eq!(policy(0b100, &region(0x30b), 0x1234), Policy::NonCacheable);
    assert_eq!(
        policy(ENABLED, &region(0x30b), 0x4000_0000),
        Policy::NonCacheable
    );
}

#[test]
fn higher_regions_and_disabled_subregions_split_mapped_spans() {
    // Region 0: 1 GiB write-back. Region 1: 0x1000..0x1800 non-cacheable,
    // a 4 KiB region whose upper four subregions are disabled.
    let regions = [
        (0, (29 << 1) | 1, 0x30b),
        (0x1000, 0xf000 | (11 << 1) | 1, 0x308),
    ];
    let spans = spans(ENABLED, &regions, &[(0, 0x4000, 0x8000_0000)]);
    let summary: Vec<_> = spans
        .iter()
        .map(|s| (s.begin, s.end, s.physical, s.policy))
        .collect();
    assert_eq!(
        summary,
        [
            (0, 0x1000, 0x8000_0000, Policy::WriteBackAllocate),
            (0x1000, 0x1800, 0x8000_1000, Policy::NonCacheable),
            (0x1800, 0x4000, 0x8000_1800, Policy::WriteBackAllocate),
        ]
    );
}
