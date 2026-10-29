use super::*;
use fujimod_emulator_devices::parts::{optics::Position, sd_card, sd_card::Card};
use fujimod_emulator_exr2_resources::Wire;
use fujimod_emulator_imaging::{analysis::ImageLayout, exposure::Exposure};

#[test]
fn noisy_rgb_input_is_atomic_and_checkpointed_without_changing_the_latched_frame() {
    let mut resources = resources();
    let input = images(&resources).unwrap().sensors["sensor"].clone();
    input.lock().unwrap().latch(5).unwrap();
    let latched = input.lock().unwrap().latched.clone();
    let noise = serde_json::json!({"seed":4,"realization":9,"full_well_electrons":20000.,"read_noise_electrons":3.});
    resources
        .images()
        .unwrap()
        .sensor_rgb_noise("sensor", 2, 1, &[128; 6], 1000., noise.clone())
        .unwrap();
    assert!(Arc::ptr_eq(&latched, &input.lock().unwrap().latched));
    let frame = input.lock().unwrap().current.clone();
    assert_eq!(frame.noise.unwrap().realization, 9);
    let before = resources.fingerprint().unwrap();
    let archive = resources
        .checkpoint()
        .unwrap()
        .export()
        .unwrap()
        .to_bytes()
        .unwrap();
    let mut invalid = noise.clone();
    invalid["full_well_electrons"] = serde_json::json!(-1.);
    for invalid in [serde_json::json!({}), invalid] {
        assert!(
            resources
                .images()
                .unwrap()
                .sensor_rgb_noise("sensor", 2, 1, &[128; 6], 1000., invalid)
                .is_err()
        );
        assert_eq!(resources.fingerprint().unwrap(), before);
    }
    resources
        .images()
        .unwrap()
        .sensor_rgb("sensor", 2, 1, &[1; 6], 1000.)
        .unwrap();
    assert!(input.lock().unwrap().current.noise.is_none());
    let archive = Checkpoint::from_bytes(&archive).unwrap();
    let restored = resources.import_checkpoint(&archive).unwrap();
    resources.restore_checkpoint(&restored).unwrap();
    assert_eq!(resources.fingerprint().unwrap(), before);
    assert_eq!(
        latched.fingerprint(),
        input.lock().unwrap().latched.fingerprint()
    );
}

#[test]
fn checkpoint_graph_resources_restore_existing_stream_scene_and_signal_handles() {
    let mut resources = Resources::default();
    let logic = fujimod_emulator_devices::signals::Level::new(false);
    let position = Position::default();
    position.publish(23);
    resources
        .wires
        .insert("focus".into(), Wire::OpticalPosition(position.clone()));
    resources
        .wires
        .insert("cable".into(), Wire::LogicLevel(logic.clone()));
    let stream: isp::stream::StreamHandle = Default::default();
    let phases: fujimod_emulator_imaging::scene::FramePhases = Default::default();
    let part = images_mut(&mut resources).unwrap();
    part.wires
        .insert("stream".into(), ImageWire::Stream(stream.clone()));
    part.wires
        .insert("phases".into(), ImageWire::PhaseEdges(phases.clone()));
    let image = ImageFrames::new(ImageLayout {
        width: 8,
        height: 2,
        stride: 16,
    })
    .unwrap();
    image.prepare_rgb(2, 1, &[255, 0, 0, 0, 255, 0]).unwrap();
    part.images.insert("preview".into(), image.clone());
    let sensor = Arc::new(Mutex::new(SensorFrames::new(
        Frame::new(2, 1, FramePixels::Raw16(vec![1, 2])).unwrap(),
    )));
    sensor.lock().unwrap().latch(5).unwrap();
    sensor
        .lock()
        .unwrap()
        .set_frame(Frame::new(2, 1, FramePixels::Raw16(vec![3, 4])).unwrap());
    part.sensors.insert("sensor".into(), sensor.clone());
    *phases.lock().unwrap() = [Some(FrameEdge { tick: 5, index: 1 }), None];
    stream.lock().unwrap().pixels = vec![9, 8, 7, 6].into();
    let before = resources.fingerprint().unwrap();
    let saved = resources.checkpoint().unwrap();
    let captured = images(&resources).unwrap().checkpoint().unwrap();
    let captured = checkpoint::saved_sensor(&captured.sensors["sensor"]).unwrap();
    assert!(Arc::ptr_eq(
        &captured.current,
        &sensor.lock().unwrap().current
    ));
    assert!(Arc::ptr_eq(
        &captured.latched,
        &sensor.lock().unwrap().latched
    ));
    logic.drive(true);
    position.publish(-42);
    image.use_analytic();
    stream.lock().unwrap().pixels.fill(0);
    sensor.lock().unwrap().latch(6).unwrap();
    *phases.lock().unwrap() = [None; 2];
    resources.restore_checkpoint(&saved).unwrap();
    assert_eq!(resources.fingerprint().unwrap(), before);
    assert!(!logic.high());
    assert_eq!(position.value(), 23);
    assert_eq!(stream.lock().unwrap().pixels.as_slice(), [9, 8, 7, 6]);
    assert_eq!(sensor.lock().unwrap().edge.unwrap().tick, 5);
}

#[test]
fn physical_wires_restore_shared_values() {
    let mut resources = Resources::default();
    let exposure = Arc::new(Mutex::new(Exposure {
        seconds: 0.008,
        analog_gain: 2.,
    }));
    let aperture = Arc::new(Mutex::new(16.));
    images_mut(&mut resources)
        .unwrap()
        .wires
        .insert("exposure".into(), ImageWire::Exposure(exposure.clone()));
    resources
        .wires
        .insert("iris".into(), Wire::Aperture(aperture.clone()));
    let initial = resources.fingerprint().unwrap();
    let saved = resources.checkpoint().unwrap();
    *exposure.lock().unwrap() = Exposure {
        seconds: 0.1,
        analog_gain: 4.,
    };
    *aperture.lock().unwrap() = 64.;
    resources.restore_checkpoint(&saved).unwrap();
    assert_eq!(
        *exposure.lock().unwrap(),
        Exposure {
            seconds: 0.008,
            analog_gain: 2.
        }
    );
    assert_eq!(*aperture.lock().unwrap(), 16.);
    assert_eq!(resources.fingerprint().unwrap(), initial);
}

fn resources() -> Resources {
    let mut resources = Resources::default();
    resources
        .wires
        .insert("cable".into(), Wire::logic_level(false));
    let sensor = SensorFrames::new(Frame::new(2, 1, FramePixels::Raw16(vec![0; 2])).unwrap());
    images_mut(&mut resources)
        .unwrap()
        .sensors
        .insert("sensor".into(), Arc::new(Mutex::new(sensor)));
    resources.inputs.cards.insert(
        "sd".into(),
        Card::new(sd_card::Options {
            sectors: 0x400000,
            max_dirty_sectors: 16,
        })
        .unwrap(),
    );
    resources
}

#[test]
fn resource_archive_restores_into_fresh_wires_media_and_sensor_handles() {
    let (encoded, expected) = {
        let mut source = resources();
        let Wire::LogicLevel(level) = &source.wires["cable"] else {
            unreachable!()
        };
        level.drive(true);
        source
            .inputs
            .cards
            .get_mut("sd")
            .unwrap()
            .write_bytes(3, &[0x42; 700])
            .unwrap();
        let mut sensor = images(&source).unwrap().sensors["sensor"].lock().unwrap();
        sensor.latch(42).unwrap();
        sensor.set_frame(
            Frame::new(2, 1, FramePixels::Rgb24(vec![1, 2, 3, 4, 5, 6].into())).unwrap(),
        );
        drop(sensor);
        let exported = source.checkpoint().unwrap().export().unwrap();
        (exported.to_bytes().unwrap(), source.fingerprint().unwrap())
    };
    let archive = Checkpoint::from_bytes(&encoded).unwrap();
    let mut target = resources();
    let saved = target.import_checkpoint(&archive).unwrap();
    let handle = images(&target).unwrap().sensors["sensor"].clone();
    let before = target.fingerprint().unwrap();
    target.validate_checkpoint(&saved).unwrap();
    assert_eq!(target.fingerprint().unwrap(), before);
    target.restore_checkpoint(&saved).unwrap();
    assert_eq!(target.fingerprint().unwrap(), expected);
    assert!(Arc::ptr_eq(
        &images(&target).unwrap().sensors["sensor"],
        &handle
    ));
    let sensor = handle.lock().unwrap();
    assert!(!Arc::ptr_eq(&sensor.current, &sensor.latched));
    assert_eq!(sensor.edge, Some(FrameEdge { tick: 42, index: 1 }));
    drop(sensor);
    let fresh = resources();
    let shared = fresh
        .import_checkpoint(&fresh.checkpoint().unwrap().export().unwrap())
        .unwrap();
    fresh.validate_checkpoint(&shared).unwrap();
    let exported = shared.export().unwrap();
    let metadata: Metadata = exported.decode("session-resources-v1").unwrap();
    assert!(metadata.sensors[0].shared);
}

/// The exported resource metadata, as the archive format records it.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Metadata {
    wires: Vec<String>,
    images: Vec<String>,
    statistics: Vec<String>,
    nands: Vec<String>,
    sensors: Vec<SensorRecord>,
}

#[test]
fn invalid_resource_archives_are_refused_before_mutating_the_graph() {
    let target = resources();
    let saved = target.checkpoint().unwrap();
    let archive = saved.export().unwrap();
    let before = target.fingerprint().unwrap();
    for bad in [
        archive.clone().with_children(vec![]),
        archive.clone().with_blobs(vec![vec![1].into()]),
    ] {
        assert!(target.import_checkpoint(&bad).is_err());
    }
    let mut metadata: Metadata = archive.decode("session-resources-v1").unwrap();
    metadata.wires.push("cable".into());
    let bad = Checkpoint::encode("session-resources-v1", &metadata)
        .unwrap()
        .with_children(archive.children().to_vec());
    assert!(target.import_checkpoint(&bad).is_err());
    let mut metadata: Metadata = archive.decode("session-resources-v1").unwrap();
    metadata.sensors[0].edge = Some(Edge { tick: 42, index: 0 });
    let bad = Checkpoint::encode("session-resources-v1", &metadata)
        .unwrap()
        .with_children(archive.children().to_vec());
    assert!(target.import_checkpoint(&bad).is_err());
    let card: Card = serde_json::from_value(serde_json::json!({
        "sectors":1,"max_dirty":1,"revision":0,"data":{}
    }))
    .unwrap();
    let media = Checkpoint::encode(
        "resource-media",
        &(
            BTreeMap::from([("sd".to_owned(), card)]),
            BTreeMap::<String, Vec<(String, Vec<u8>)>>::new(),
        ),
    )
    .unwrap();
    // The media child follows the one wire.
    let mut children = archive.children().to_vec();
    children[1] = media;
    let corrupt = archive.clone().with_children(children);
    let imported = target.import_checkpoint(&corrupt).unwrap();
    assert!(target.validate_checkpoint(&imported).is_err());
    assert_eq!(target.fingerprint().unwrap(), before);
}
