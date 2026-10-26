use super::*;
use fujimod_emulator_exr2_resources::ImageSpec;
use fujimod_emulator_imaging::analysis::ImageLayout;

#[test]
fn initial_rgb_waits_for_a_real_frame_edge_and_validates_extent() {
    let assets = BTreeMap::from([
        ("packed".into(), vec![1, 2, 128, 128]),
        ("rgb".into(), vec![255, 0, 0]),
    ]);
    let spec = |image: serde_json::Value| ImageSpec {
        images: BTreeMap::from([("live".to_owned(), image)]),
        ..ImageSpec::default()
    };
    let image = serde_json::json!({"width":2,"height":1,"stride":4,
        "asset":"packed","rgb_source":{"width":1,"height":1,"asset":"rgb"}});
    let part = preparation::prepare(spec(image.clone()), &assets).unwrap();
    let frames = &part.as_any().downcast_ref::<Images>().unwrap().images["live"];
    let baseline = ImageFrames::from_pixels(
        ImageLayout {
            width: 2,
            height: 1,
            stride: 4,
        },
        vec![1, 2, 128, 128],
    )
    .unwrap();
    assert_eq!(frames.fingerprint()["edge"], serde_json::Value::Null);
    assert_eq!(
        frames.fingerprint()["bytes"],
        baseline.fingerprint()["bytes"]
    );
    assert_ne!(frames.fingerprint()["prepared"], serde_json::Value::Null);
    let mut invalid = image;
    invalid["rgb_source"]["width"] = serde_json::json!(2);
    assert!(preparation::prepare(spec(invalid), &assets).is_err());
}
