use super::*;

#[test]
fn profile_field_sources_round_trip_through_catalog_json() {
    for source in [
        ProfileFieldSource::Fill { value: 255 },
        ProfileFieldSource::BankCopy { offset: 0x9742 },
        ProfileFieldSource::BankBits {
            offset: 0x56df,
            mask: 31,
        },
        ProfileFieldSource::BankMap {
            offset: 0x218,
            mask: 0x60,
            values: vec![BankByteValue {
                input: 1,
                output: 0,
            }],
            otherwise: 0x20,
            otherwise_high: Some(0),
        },
    ] {
        let json = serde_json::to_string(&source).unwrap();
        assert_eq!(
            serde_json::from_str::<ProfileFieldSource>(&json).unwrap(),
            source
        );
    }
}
