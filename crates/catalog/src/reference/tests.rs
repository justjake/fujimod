use super::validate_id;

#[test]
fn stable_identities_are_bounded_and_not_paths() {
    for identity in ["x70", "x70-110", "exr2.core0", "display_01"] {
        assert!(validate_id(identity).is_ok(), "{identity}");
    }
    for identity in ["", ".", "..", "../x70", "/x70", "x70/1.10", "bad id"] {
        assert!(validate_id(identity).is_err(), "{identity}");
    }
}
