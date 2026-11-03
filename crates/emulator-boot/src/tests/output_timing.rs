use super::*;

#[test]
fn output_dma_latency_is_independent_of_the_scene_frame_period() {
    for (latency, frame) in [(1024, 400000), (321, 480000)] {
        let mut resolved =
            ResolvedConfiguration::with_overlays("x70-110-rec", &["x70-110-browser-camera".into()])
                .unwrap();
        resolved
            .configuration
            .preset
            .as_mut()
            .unwrap()
            .factory_options
            .image_work_data_ticks = Some(latency);
        run_scene(&mut resolved).frame_period_ticks = frame;
        let run = prepared(resolved).unwrap();
        for bank in ["image_output_0", "image_output_1"] {
            assert_eq!(model(&run, bank)["options"]["clock"]["ticks"], latency);
        }
        assert_eq!(model(&run, "live_frames")["options"]["period_ticks"], frame);
    }
    let browser = prepare("x70-110-rec", &["x70-110-browser-camera".into()]).unwrap();
    let base = prepare("x70-110-rec", &[]).unwrap();
    for bank in ["image_output_0", "image_output_1"] {
        assert_eq!(model(&browser, bank)["options"]["clock"]["ticks"], 1024);
        assert_eq!(model(&base, bank)["options"]["clock"]["ticks"], 400000);
    }
}
