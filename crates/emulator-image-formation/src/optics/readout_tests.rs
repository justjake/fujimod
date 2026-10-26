use super::*;
use crate::exposure::Filtered;
use fujimod_emulator_imaging::exposure::{Calibration, Exposure};
use fujimod_emulator_imaging::scene::Frame;

fn exposure(frame: Arc<Frame>, seconds: f64) -> ExposedFrame {
    exposure_at_aperture(frame, seconds, 4.)
}
fn exposure_at_aperture(frame: Arc<Frame>, seconds: f64, aperture_squared: f64) -> ExposedFrame {
    ExposedFrame::from_frame(
        frame,
        Exposure {
            seconds,
            analog_gain: 1.3,
        },
        aperture_squared,
        Calibration {
            full_scale: 16380,
            reference_white: [193, 257, 311],
            saturation_lux_seconds: 1.,
            color_sample_weights: [8, 20, 8],
        },
    )
    .unwrap()
}
fn projection(illumination: Illumination) -> Projection {
    Projection::new(
        Spec {
            focus: "focus".into(),
            focal_length_mm: 18.5,
            subject_distance_mm: 2000.,
            sensor_width_mm: 0.4,
            relative_illumination: illumination,
        },
        Default::default(),
    )
    .unwrap()
}
// Independent scalar separable PSF: clamp every contributing coordinate and
// round each horizontal/vertical box before blending adjacent integer radii.
fn scalar(p: &Projection, frame: &ExposedFrame, shift: f64) -> Arc<[u8]> {
    let FramePixels::Rgb24(rgb) = &frame.frame.pixels else {
        unreachable!()
    };
    let [w, h] = [frame.frame.width, frame.frame.height];
    let lut: [u16; 256] = std::array::from_fn(|i| {
        let v = i as f64 / 255.;
        let v = if v <= 0.04045 {
            v / 12.92
        } else {
            libm::pow((v + 0.055) / 1.055, 2.4)
        };
        libm::rint(v * 65535.) as u16
    });
    let source: Vec<[u16; 3]> = rgb
        .chunks_exact(3)
        .map(|v| std::array::from_fn(|c| lut[v[c] as usize]))
        .collect();
    let diameter = (shift - p.subject_shift).abs() * p.spec.focal_length_mm
        / (libm::sqrt(frame.aperture_squared) * (p.spec.focal_length_mm + p.subject_shift));
    let radius = (diameter * w as f64 / (2. * p.spec.sensor_width_mm)).min(w.max(h) as f64);
    let whole = libm::floor(radius);
    let fraction = libm::rint((radius - whole) * 65536.).clamp(0., 65535.) as u64;
    let radius = whole as usize;
    let pass = |values: &[[u16; 3]], horizontal: bool| {
        let mut output = vec![[0; 3]; values.len()];
        for y in 0..h {
            for x in 0..w {
                let mean = |r: usize, c: usize| {
                    let mut sum = 0u64;
                    for d in -(r as isize)..=r as isize {
                        let (xx, yy) = if horizontal {
                            ((x as isize + d).clamp(0, w as isize - 1) as usize, y)
                        } else {
                            (x, (y as isize + d).clamp(0, h as isize - 1) as usize)
                        };
                        sum += u64::from(values[yy * w + xx][c]);
                    }
                    (sum + r as u64) / (2 * r + 1) as u64
                };
                output[y * w + x] = std::array::from_fn(|c| {
                    let inner = mean(radius, c);
                    if fraction == 0 {
                        inner as u16
                    } else {
                        ((inner * (65536 - fraction) + mean(radius + 1, c) * fraction + 32768)
                            >> 16) as u16
                    }
                });
            }
        }
        output
    };
    let horizontal = pass(&source, true);
    let result = pass(&horizontal, false);
    let illumination = match p.spec.relative_illumination {
        Illumination::Uniform => None,
        Illumination::CosineFourth => Some(Field::cosine_fourth(p.spec.focal_length_mm + shift)),
        Illumination::RadialPolynomial { .. } => unreachable!("reference covers the thin-lens laws"),
    };
    encode_response(
        &result,
        frame.response_scale,
        [w, h],
        p.spec.sensor_width_mm,
        illumination,
    )
}
#[test]
fn sparse_readouts_and_unsampled_queries_match_scalar_psf() {
    for [w, h] in [[1, 1], [5, 3], [17, 13]] {
        let frame = Arc::new(
            Frame::new(
                w,
                h,
                FramePixels::Rgb24(
                    (0..w * h * 3)
                        .map(|i| ((i * 73 + i / 3 * 11) % 256) as u8)
                        .collect(),
                ),
            )
            .unwrap(),
        );
        for illumination in [Illumination::Uniform, Illumination::CosineFourth] {
            let p = projection(illumination);
            for shift in [
                p.subject_shift + 0.001,
                p.subject_shift + 0.03,
                p.subject_shift + 0.35,
                p.subject_shift + 10.,
            ] {
                for (grid, window) in [
                    ([w, h], [0, 0, w, h]),
                    ([31, 19], [3, 5, 7, 4]),
                    ([3, 2], [1, 0, 1, 2]),
                    ([3, 2], [3, 1, 0, 1]),
                ] {
                    let mut f = exposure(frame.clone(), 0.0003);
                    let expected = scalar(&p, &f, shift);
                    p.prepare_readout(&mut f, shift, grid, window).unwrap();
                    // The chosen shifts require a nonzero PSF, even for one pixel.
                    let pixels = f.filtered.as_ref().unwrap();
                    assert_eq!(
                        pixels.materialize(),
                        expected,
                        "shape {w}x{h}, grid {grid:?}, window {window:?}, shift {shift}"
                    );
                    for y in 0..h {
                        for x in 0..w {
                            let v = f.sample_source(x, y);
                            for c in 0..3 {
                                let at = (y * w + x) * 6 + c * 2;
                                assert_eq!(
                                    v[c],
                                    u16::from_le_bytes([expected[at], expected[at + 1]])
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}
#[test]
fn scene_prefix_and_readout_memos_use_complete_physical_keys() {
    let rgb: Arc<[u8]> = (0..192 * 128 * 3).map(|i| (i * 37 % 256) as u8).collect();
    let frame = Arc::new(Frame::new(192, 128, FramePixels::Rgb24(rgb)).unwrap());
    let p = projection(Illumination::CosineFourth);
    let grid = [6240, 4160];
    let window = [500, 400, 240, 160];
    let mut a = exposure(frame.clone(), 0.0003);
    p.prepare_readout(&mut a, 0.2, grid, window).unwrap();
    let scene = p.scene.lock().unwrap().as_ref().unwrap().clone();
    let prepared = p.prepared.lock().unwrap();
    let old = prepared.as_ref().unwrap();
    let linear = old.linear.clone();
    let pixels = old.pixels.clone();
    assert!(linear.len() < 192 * 128 / 100);
    drop(prepared);
    let mut b = exposure(frame.clone(), 0.0003);
    p.prepare_readout(&mut b, 0.2, grid, window).unwrap();
    assert!(Arc::ptr_eq(
        &pixels,
        &p.prepared.lock().unwrap().as_ref().unwrap().pixels
    ));
    let mut c = exposure(frame.clone(), 0.0005);
    p.prepare_readout(&mut c, 0.2, grid, window).unwrap();
    assert!(Arc::ptr_eq(
        &linear,
        &p.prepared.lock().unwrap().as_ref().unwrap().linear
    ));
    assert!(!Arc::ptr_eq(
        &pixels,
        &p.prepared.lock().unwrap().as_ref().unwrap().pixels
    ));
    let mut d = exposure(frame.clone(), 0.0005);
    p.prepare_readout(&mut d, 0.6, grid, window).unwrap();
    assert!(Arc::ptr_eq(
        &scene,
        p.scene.lock().unwrap().as_ref().unwrap()
    ));
    assert!(!Arc::ptr_eq(
        &linear,
        &p.prepared.lock().unwrap().as_ref().unwrap().linear
    ));
    let mut e = exposure(frame, 0.0005);
    p.prepare_readout(&mut e, 0.6, grid, [2000, 1000, 240, 160])
        .unwrap();
    assert!(Arc::ptr_eq(
        &scene,
        p.scene.lock().unwrap().as_ref().unwrap()
    ));
    assert_ne!(e.filtered.as_ref().unwrap().materialize().len(), 0);
}
#[test]
fn large_scene_sparse_cache_matches_fresh_projection_and_invalidates() {
    let [width, height] = [2050, 2048];
    assert!(width * height > CACHE_PIXELS);
    let rgb: Arc<[u8]> = (0..width * height * 3)
        .map(|i| ((i * 37 + i / (width * 3) * 19) % 256) as u8)
        .collect();
    let source = Arc::new(Frame::new(width, height, FramePixels::Rgb24(rgb.clone())).unwrap());
    let mut brighter = Frame::new(width, height, FramePixels::Rgb24(rgb)).unwrap();
    brighter.white_luminance *= 1.2;
    let brighter = Arc::new(brighter);
    let replacement = Arc::new(
        Frame::new(
            width,
            height,
            FramePixels::Rgb24(vec![173; width * height * 3].into()),
        )
        .unwrap(),
    );
    let p = projection(Illumination::CosineFourth);
    let grid = [6240, 4160];
    let window = [500, 400, 24, 16];
    let mut old_pixels = None;
    for (index, (source, seconds, aperture, shift, grid, window)) in [
        (source.clone(), 0.0003, 4., 0.2, grid, window),
        (source.clone(), 0.0003, 4., 0.2, grid, window),
        (source.clone(), 0.0005, 4., 0.2, grid, window),
        (source.clone(), 0.0005, 4., 0.6, grid, window),
        (source.clone(), 0.0005, 9., 0.6, grid, window),
        (source.clone(), 0.0005, 9., 0.6, grid, [2000, 1000, 24, 16]),
        (source, 0.0005, 9., 0.6, [4000, 3000], [2000, 1000, 24, 16]),
        (brighter, 0.0005, 9., 0.6, grid, window),
        (replacement, 0.0005, 9., 0.6, grid, window),
    ]
    .into_iter()
    .enumerate()
    {
        let mut actual = exposure_at_aperture(source, seconds, aperture);
        let mut expected = actual.clone();
        p.prepare_readout(&mut actual, shift, grid, window).unwrap();
        // A separate projection has no prepared arithmetic to reuse.
        let fresh = projection(Illumination::CosineFourth);
        fresh
            .prepare_readout(&mut expected, shift, grid, window)
            .unwrap();
        let prepared = p.prepared.lock().unwrap();
        let prepared = prepared.as_ref().expect("sparse cache must be admitted");
        assert!(prepared.linear.len() <= CACHE_PIXELS);
        if let Some(old) = old_pixels {
            assert_eq!(Arc::ptr_eq(&old, &prepared.pixels), index == 1);
        }
        old_pixels = Some(prepared.pixels.clone());
        for &y in &prepared.coordinates.ys {
            for &x in &prepared.coordinates.xs {
                assert_eq!(actual.sample_source(x, y), expected.sample_source(x, y));
            }
        }
        // The cached sparse law also answers unsampled coordinates identically.
        for (x, y) in [(0, 0), (width / 2, height / 2), (width - 1, height - 1)] {
            assert_eq!(actual.sample_source(x, y), expected.sample_source(x, y));
        }
    }
}
#[test]
fn sparse_projection_checkpoint_is_canonical_and_observational() {
    let frame = Arc::new(
        Frame::new(
            17,
            13,
            FramePixels::Rgb24((0..17 * 13 * 3).map(|i| (i * 73 % 256) as u8).collect()),
        )
        .unwrap(),
    );
    let p = projection(Illumination::CosineFourth);
    let mut f = exposure(frame, 0.0003);
    p.prepare_readout(&mut f, 0.5, [31, 19], [3, 5, 7, 4])
        .unwrap();
    let Filtered::Readout(projected) = f.filtered.as_ref().unwrap() else {
        panic!("readout projection missing")
    };
    let projected = projected.clone();
    let before = f.fingerprint();
    let saved = f.checkpoint().unwrap();
    assert_eq!(f.fingerprint(), before);
    assert!(Arc::ptr_eq(
        &projected,
        &p.prepared.lock().unwrap().as_ref().unwrap().pixels
    ));
    let restored = ExposedFrame::from_checkpoint(&saved).unwrap();
    assert_eq!(restored.fingerprint(), before);
    for y in 0..13 {
        for x in 0..17 {
            assert_eq!(restored.sample_source(x, y), f.sample_source(x, y));
        }
    }
}
#[test]
fn invalid_readout_geometry_refuses_before_changing_the_exposure() {
    let frame = Arc::new(Frame::new(2, 1, FramePixels::Rgb24(vec![128; 6].into())).unwrap());
    let p = projection(Illumination::CosineFourth);
    let mut f = exposure(frame, 0.0003);
    p.prepare(&mut f, 0.2).unwrap();
    let before = f.fingerprint();
    for (grid, window) in [
        ([0, 2], [0, 0, 1, 1]),
        ([16385, 2], [0, 0, 1, 1]),
        ([2, 2], [1, 0, 2, 1]),
        ([2, 2], [0, 2, 1, 1]),
    ] {
        assert!(p.prepare_readout(&mut f, 0.2, grid, window).is_err());
        assert_eq!(f.fingerprint(), before);
    }
}

#[test]
fn vectorizable_response_rounding_matches_canonical_full_precision() {
    let dimensions = [256, 256];
    let frame = Frame::new(256, 256, FramePixels::Rgb24(vec![0; 256 * 256 * 3].into())).unwrap();
    let scene = Arc::new(scene::Scene::new(Arc::new(frame)).unwrap());
    let coordinates = Arc::new(readout::Coordinates::full(dimensions));
    // Every possible linear ADC input, including half-way quantization values,
    // underflow, saturation and finite scales whose intermediate overflows.
    let linear: Vec<[u16; 3]> = (0..=u16::MAX)
        .map(|v| [v, v.rotate_left(7), u16::MAX - v])
        .collect();
    for scale in [0., f64::MIN_POSITIVE, 0.5, 32767.5, 65535., 1e9, f64::MAX] {
        for illumination in [None, Some(Field::cosine_fourth(18.5))] {
            let scale = [scale; 3];
            let projected = Projected::new(
                scene.clone(),
                coordinates.clone(),
                0,
                0,
                &linear,
                scale,
                23.6,
                illumination,
            );
            assert_eq!(
                projected.materialize(),
                encode_response(&linear, scale, dimensions, 23.6, illumination)
            );
        }
    }
}

#[test]
fn dense_readouts_keep_peer_sampling_constant_time_and_small_windows_sparse() {
    let frame = Arc::new(
        Frame::new(
            8,
            8,
            FramePixels::Rgb24((0..8 * 8 * 3).map(|i| (i * 73 % 256) as u8).collect()),
        )
        .unwrap(),
    );
    let p = projection(Illumination::CosineFourth);
    let mut a = exposure(frame.clone(), 0.0003);
    p.prepare_readout(&mut a, 0.5, [8, 6], [0, 0, 8, 6])
        .unwrap();
    let Filtered::Full(first) = a.filtered.as_ref().unwrap() else {
        panic!("three-quarter readout must retain complete peer samples")
    };
    assert_eq!(first, &scalar(&p, &a, 0.5));
    let mut b = exposure(frame.clone(), 0.0003);
    p.prepare_readout(&mut b, 0.5, [8, 6], [0, 0, 8, 6])
        .unwrap();
    let Filtered::Full(second) = b.filtered.as_ref().unwrap() else {
        panic!()
    };
    assert!(Arc::ptr_eq(first, second));
    assert_eq!(
        p.prepared.lock().unwrap().as_ref().unwrap().coordinates.key,
        ([8, 6], [0, 0, 8, 6])
    );
    let mut c = exposure(frame, 0.0003);
    p.prepare_readout(&mut c, 0.5, [8, 5], [0, 0, 8, 5])
        .unwrap();
    assert!(matches!(c.filtered, Some(Filtered::Readout(_))));
    assert_eq!(c.fingerprint(), a.fingerprint());
}
