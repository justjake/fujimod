//! End-to-end checks on synthetic X-Trans bursts with known motion.
//! Run with `--release`; the scenes are rendered with 16 samples per site.

use fujimod_multishot::align::{
    AlignParams, Alignment, FrameAlignment, MatchParams, MatchStats, align,
};
use fujimod_multishot::demosaic;
use fujimod_multishot::dng::{DngInfo, DngWriter, XYZ_TO_SRGB};
use fujimod_multishot::fit::{Affine, FitParams};
use fujimod_multishot::local::{LocalParams, residual_fields};
use fujimod_multishot::measure::{crop, luma, patch_noise, rmse, slanted_edge};
use fujimod_multishot::merge::{
    ColorMerge, ImageSink, OutputGrid, Region, ResolutionParams, merge_resolution,
};
use fujimod_multishot::noise::NoiseModel;
use fujimod_multishot::source::{FrameSource, MemoryFrame};
use fujimod_multishot::synthetic::{
    MovingDisc, NoiseSpec, Scene, WB, handheld_transforms, render, render_exposed,
};
use fujimod_multishot::tiff::{Tiff, find};

const W: usize = 600;
const H: usize = 420;
const NOISE: NoiseSpec = NoiseSpec {
    shot: 2e-5,
    read: 0.001,
};

fn test_align_params() -> AlignParams {
    AlignParams {
        matching: MatchParams {
            levels: 3,
            patch_radius: 6,
            coarse_search: 3,
            grid_step: 8,
            ..MatchParams::default()
        },
        fit: FitParams {
            min_grid_cells: 4,
            ..FitParams::default()
        },
        ..AlignParams::default()
    }
}

fn burst(scene: &Scene, transforms: &[Affine], noise: Option<NoiseSpec>) -> Vec<MemoryFrame> {
    std::thread::scope(|s| {
        let handles: Vec<_> = transforms
            .iter()
            .enumerate()
            .map(|(k, t)| s.spawn(move || render(scene, W, H, t, k, noise, 7)))
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    })
}

fn refs(frames: &[MemoryFrame]) -> Vec<&dyn FrameSource> {
    frames.iter().map(|f| f as &dyn FrameSource).collect()
}

fn region((x, y, width, height): (usize, usize, usize, usize)) -> Region {
    Region {
        x,
        y,
        width,
        height,
    }
}

fn merge(
    frames: &[&dyn FrameSource],
    alignment: &Alignment,
    params: &ResolutionParams,
    r: Region,
) -> (OutputGrid, Vec<f32>) {
    let p = ResolutionParams {
        region: Some(r),
        ..params.clone()
    };
    let grid = OutputGrid::new(r, p.scale);
    let mut sink = ImageSink::new(grid.width, grid.height);
    merge_resolution(frames, alignment, &p, &mut sink).unwrap();
    (grid, sink.rgb)
}

fn reference_only(a: &Alignment) -> Alignment {
    let mut s = a.clone();
    for (k, f) in s.frames.iter_mut().enumerate() {
        if k != a.reference {
            f.to_frame = None;
        }
    }
    s
}

fn mtf50(rgb: &[f32], grid: &OutputGrid) -> f64 {
    slanted_edge(&luma(rgb, WB), grid.width, grid.height)
        .unwrap()
        .mtf50
        * grid.scale
}

#[test]
fn alignment_recovers_known_transforms() {
    let scene = Scene::chart(W, H);
    let truth = handheld_transforms(6, W, H, 11);
    let frames = burst(&scene, &truth, Some(NOISE));
    let a = align(&refs(&frames), 0, &test_align_params()).unwrap();
    let mut worst = 0f64;
    for (t, f) in truth.iter().zip(&a.frames) {
        let got = f
            .to_frame
            .unwrap_or_else(|| panic!("{} rejected: {:?}", f.label, f.rejected));
        for (x, y) in [
            (0.0, 0.0),
            (W as f64, 0.0),
            (0.0, H as f64),
            (W as f64, H as f64),
            (W as f64 / 2.0, H as f64 / 2.0),
        ] {
            let (a, b) = got.apply(x, y);
            let (c, d) = t.apply(x, y);
            worst = worst.max(((a - c).powi(2) + (b - d).powi(2)).sqrt());
        }
    }
    println!("worst corner error {worst:.4} sites");
    assert!(worst < 0.1, "worst corner error {worst} sites");
    let noise = a.noise.expect("noise model");
    // Green block mean of five sites on the flat 0.3 patch:
    // (shot * 0.3 + read^2) / 5.
    let expected = (NOISE.shot * 0.3 + NOISE.read * NOISE.read) / 5.0;
    let got = noise.variance(1, 0.3);
    println!("green block variance at 0.3: {got:e} (expected {expected:e})");
    assert!(got > 0.5 * expected && got < 2.0 * expected, "{noise:?}");
}

#[test]
fn merge_beats_single_frame() {
    let scene = Scene::chart(W, H);
    let truth = handheld_transforms(12, W, H, 5);
    let frames = burst(&scene, &truth, Some(NOISE));
    let fr = refs(&frames);
    let a = align(&fr, 0, &test_align_params()).unwrap();
    assert_eq!(a.accepted(), 12);
    let params = ResolutionParams::default();

    let edge = region(scene.edge_region());
    let (grid, merged) = merge(&fr, &a, &params, edge);
    let (_, single) = merge(&fr, &reference_only(&a), &params, edge);
    let baseline = demosaic::baseline(fr[0], &grid).unwrap();
    let (m, s, b) = (
        mtf50(&merged, &grid),
        mtf50(&single, &grid),
        mtf50(&baseline, &grid),
    );
    println!("MTF50 cycles/site: merge {m:.4}  single-frame merge {s:.4}  demosaic+upscale {b:.4}");

    let detail = region(scene.detail_region());
    let (dgrid, dmerged) = merge(&fr, &a, &params, detail);
    let dbase = demosaic::baseline(fr[0], &dgrid).unwrap();
    let ideal = scene.truth(&dgrid, 0);
    let (em, eb) = (rmse(&dmerged, &ideal), rmse(&dbase, &ideal));
    println!("detail RMSE vs full-color truth: merge {em:.5}  demosaic+upscale {eb:.5}");

    let flat = region(scene.flat_region());
    let (fgrid, fmerged) = merge(&fr, &a, &params, flat);
    let fbase = demosaic::baseline(fr[0], &fgrid).unwrap();
    let inset = |v: &[f32]| crop(v, fgrid.width, 3, 4, 4, fgrid.width - 8, fgrid.height - 8);
    let (nm, nb) = (
        patch_noise(&inset(&fmerged), fgrid.width - 8, WB),
        patch_noise(&inset(&fbase), fgrid.width - 8, WB),
    );
    println!(
        "flat patch luma relative std: merge {:.5}  demosaic+upscale {:.5}",
        nm.luma_relative_std, nb.luma_relative_std
    );
    assert!(nm.luma_relative_std < 0.5 * nb.luma_relative_std);

    assert!(m > 1.05 * b, "merge MTF50 {m} vs baseline {b}");
    assert!(m > 1.15 * s, "merge MTF50 {m} vs single-frame merge {s}");
    assert!(em < 0.75 * eb, "merge RMSE {em} vs baseline {eb}");

    // Red and blue merged on their own are sparser than green and blur luma
    // on gray detail. Uncorrelated color texture is the worst case for
    // merging color differences, so there it must only stay close.
    let independent = ResolutionParams {
        color: ColorMerge::Independent,
        ..params.clone()
    };
    let (_, imerged) = merge(&fr, &a, &independent, edge);
    let mi = mtf50(&imerged, &grid);
    let (dx, dy, dw, dh) = scene.detail_region();
    let half = dw / 2;
    let (star, texture) = (
        region((dx, dy, half - 4, dh)),
        region((dx + half + 4, dy, dw - half - 4, dh)),
    );
    let part_rmse = |r: Region| {
        let (g, guided) = merge(&fr, &a, &params, r);
        let (_, indep) = merge(&fr, &a, &independent, r);
        let ideal = scene.truth(&g, 0);
        (rmse(&guided, &ideal), rmse(&indep, &ideal))
    };
    let ((sg, si), (tg, ti)) = (part_rmse(star), part_rmse(texture));
    println!(
        "green-guided vs independent: MTF50 {m:.4} vs {mi:.4}, star RMSE {sg:.5} vs {si:.5}, color texture RMSE {tg:.5} vs {ti:.5}"
    );
    assert!(m > 1.02 * mi, "green-guided MTF50 {m} vs independent {mi}");
    assert!(sg < 0.95 * si, "star RMSE {sg} vs independent {si}");
    assert!(
        tg < 1.25 * ti,
        "color texture RMSE {tg} vs independent {ti}"
    );
}

#[test]
fn identical_frames_give_no_gain() {
    let scene = Scene::chart(W, H);
    let one = render(&scene, W, H, &Affine::IDENTITY, 0, Some(NOISE), 7);
    let copies: Vec<MemoryFrame> = (0..8)
        .map(|k| MemoryFrame {
            label: format!("copy-{k}"),
            width: one.width,
            height: one.height,
            cfa: one.cfa,
            calibration: one.calibration.clone(),
            data: one.data.clone(),
        })
        .collect();
    let fr = refs(&copies);
    // Registration of identical frames finds the identity.
    let a = align(&fr, 0, &test_align_params()).unwrap();
    for f in &a.frames {
        let t = f.to_frame.unwrap();
        for (x, y) in [(0.0, 0.0), (W as f64, H as f64)] {
            let (u, v) = t.apply(x, y);
            assert!((u - x).abs() < 1e-3 && (v - y).abs() < 1e-3, "{:?}", t);
        }
    }
    // Merge exact identities so the comparison isolates the merge.
    let mut exact = a.clone();
    for f in exact.frames.iter_mut() {
        f.to_frame = Some(Affine::IDENTITY);
    }
    let params = ResolutionParams::default();
    let edge = region(scene.edge_region());
    let (grid, merged) = merge(&fr, &exact, &params, edge);
    let (_, single) = merge(&fr, &reference_only(&exact), &params, edge);
    let diff = merged
        .iter()
        .zip(&single)
        .map(|(a, b)| (a - b).abs())
        .fold(0f32, f32::max);
    let (m, s) = (mtf50(&merged, &grid), mtf50(&single, &grid));
    println!("identical frames: MTF50 merge {m:.4} single {s:.4}, max pixel difference {diff:e}");
    assert!(diff < 1e-5, "max difference {diff}");
    assert!((m - s).abs() < 1e-3 * s);
}

#[test]
fn robustness_rejects_moving_subject() {
    let mut scene = Scene::chart(W, H);
    scene.moving = Some(MovingDisc {
        center: (W as f64 * 0.62, H as f64 * 0.5),
        radius: 14.0,
        velocity: (6.0, 2.0),
        color: [0.7, 0.1, 0.1],
    });
    let truth = handheld_transforms(8, W, H, 3);
    let frames = burst(&scene, &truth, Some(NOISE));
    let fr = refs(&frames);
    let a = align(&fr, 0, &test_align_params()).unwrap();
    assert_eq!(a.accepted(), 8);
    // The disc's path, minus where it sits in the reference.
    let m = scene.moving.clone().unwrap();
    let r = Region {
        x: (m.center.0 - m.radius - 2.0) as usize,
        y: (m.center.1 - m.radius - 2.0) as usize,
        width: (7.0 * m.velocity.0 + 2.0 * m.radius + 4.0) as usize,
        height: (7.0 * m.velocity.1 + 2.0 * m.radius + 4.0) as usize,
    };
    let robust = ResolutionParams::default();
    let naive = ResolutionParams {
        robustness: None,
        ..ResolutionParams::default()
    };
    let (grid, with) = merge(&fr, &a, &robust, r);
    let (_, without) = merge(&fr, &a, &naive, r);
    let ideal = scene.truth(&grid, 0);
    let (ew, eo) = (rmse(&with, &ideal), rmse(&without, &ideal));
    println!("moving subject RMSE vs reference scene: robust {ew:.5}  without robustness {eo:.5}");
    assert!(ew < 0.4 * eo, "robust {ew} vs naive {eo}");
}

#[test]
fn dng_strips_and_header() {
    let scene = Scene::chart(W, H);
    let frames = burst(&scene, &handheld_transforms(3, W, H, 9), Some(NOISE));
    let fr = refs(&frames);
    let a = align(&fr, 0, &test_align_params()).unwrap();
    let r = Region {
        x: 150,
        y: 60,
        width: 200,
        height: 101,
    };
    let params = ResolutionParams {
        region: Some(r),
        strip_rows: 16,
        ..ResolutionParams::default()
    };
    let grid = OutputGrid::new(r, params.scale);
    let path = std::env::temp_dir().join(format!("multishot-{}.dng", std::process::id()));
    let info = DngInfo {
        make: "FUJIFILM".into(),
        model: "synthetic".into(),
        wb: WB,
        color_matrix: XYZ_TO_SRGB,
    };
    let mut dng =
        DngWriter::create(&path, grid.width, grid.height, params.strip_rows, &info).unwrap();
    let report = merge_resolution(&fr, &a, &params, &mut dng).unwrap();
    dng.finish().unwrap();
    let bytes = std::fs::read(&path).unwrap();
    std::fs::remove_file(&path).ok();
    let t = Tiff::new(&bytes).unwrap();
    let ifd = t.ifd(t.first_ifd().unwrap()).unwrap();
    let get = |tag| t.uints(find(&ifd, tag).unwrap()).unwrap();
    assert_eq!(get(256), vec![grid.width as u32]);
    assert_eq!(get(257), vec![grid.height as u32]);
    assert_eq!(get(262), vec![34892]);
    let offsets = get(273);
    let counts = get(279);
    assert_eq!(offsets.len(), grid.height.div_ceil(16));
    assert_eq!(report.strips, offsets.len());
    let last = offsets.len() - 1;
    assert_eq!((offsets[last] + counts[last]) as usize, bytes.len());
    assert_eq!(
        counts.iter().sum::<u32>() as usize,
        grid.width * grid.height * 6
    );
    println!(
        "DNG {}x{}, {} strips, peak strip memory {} bytes",
        grid.width, grid.height, report.strips, report.peak_strip_bytes
    );
}

#[test]
fn noise_model_from_manual_alignment_is_used() {
    // A merge with robustness and no noise model is refused, not silently
    // merged without motion rejection.
    let scene = Scene::chart(W, H);
    let frames = burst(&scene, &handheld_transforms(2, W, H, 1), None);
    let fr = refs(&frames);
    let frame = |label: &str, t: Affine| FrameAlignment {
        label: label.into(),
        to_frame: Some(t),
        block_fit: None,
        block_refined: None,
        matches: MatchStats::default(),
        inliers: 0,
        fit_rms: 0.0,
        refine: None,
        fine: None,
        rejected: None,
        gain: 1.0,
    };
    let mut a = Alignment {
        reference: 0,
        width: W,
        height: H,
        frames: vec![
            frame("a", Affine::IDENTITY),
            frame("b", handheld_transforms(2, W, H, 1)[1]),
        ],
        noise: None,
        peak_bytes: 0,
    };
    let r = Region {
        x: 60,
        y: 60,
        width: 60,
        height: 60,
    };
    let p = ResolutionParams {
        region: Some(r),
        ..ResolutionParams::default()
    };
    let mut sink = ImageSink::new(90, 90);
    assert!(merge_resolution(&fr, &a, &p, &mut sink).is_err());
    a.noise = Some(NoiseModel {
        a: [1e-5; 3],
        b: [1e-7; 3],
    });
    merge_resolution(&fr, &a, &p, &mut sink).unwrap();
    let c = crop(&sink.rgb, 90, 3, 10, 10, 10, 10);
    assert!(c.iter().all(|v| v.is_finite()));
}

#[test]
fn local_alignment_corrects_residual_shift() {
    // Each frame's true transform is pushed 0.4 sites off, alternating in
    // sign across the near-vertical edge (the noise model comes from a real
    // alignment). The residual field must measure
    // the error back and the merge must regain the correctly aligned
    // sharpness, which the global-only merge loses.
    let scene = Scene::chart(W, H);
    let truth = handheld_transforms(8, W, H, 11);
    let frames = burst(&scene, &truth, Some(NOISE));
    let fr = refs(&frames);
    let mut aligned = align(&fr, 0, &test_align_params()).unwrap();
    assert_eq!(aligned.accepted(), 8);
    for (f, t) in aligned.frames.iter_mut().zip(&truth) {
        f.to_frame = Some(*t);
    }
    let mut shifted = aligned.clone();
    let mut errors = vec![0.0; 8];
    for (k, f) in shifted.frames.iter_mut().enumerate().skip(1) {
        let dx = if k % 2 == 0 { 0.4 } else { -0.4 };
        errors[k] = dx;
        let mut t = f.to_frame.unwrap();
        t.0[4] += dx;
        f.to_frame = Some(t);
    }
    let edge = region(scene.edge_region());
    let params = ResolutionParams {
        local: Some(LocalParams::default()),
        ..ResolutionParams::default()
    };
    let (fields, report) =
        residual_fields(&fr, &shifted, edge, 4.0, params.local.as_ref().unwrap()).unwrap();
    let (cx, cy) = (
        edge.x as f64 + edge.width as f64 / 2.0,
        edge.y as f64 + edge.height as f64 / 2.0,
    );
    for (k, &dx) in errors.iter().enumerate().skip(1) {
        let (sx, sy) = fields[k].as_ref().unwrap().at(cx, cy);
        assert!(
            (sx + dx).abs() < 0.1 && sy.abs() < 0.1,
            "frame {k}: field ({sx:.3}, {sy:.3}) for an error of {dx}"
        );
    }
    // The edge panel is a lone edge; its tiles are filled from the star.
    assert!(report.frames.iter().all(|f| f.measured > 0));

    let global = ResolutionParams {
        local: None,
        ..params.clone()
    };
    let (grid, good) = merge(&fr, &aligned, &global, edge);
    let (_, off) = merge(&fr, &shifted, &global, edge);
    let (_, fixed) = merge(&fr, &shifted, &params, edge);
    let (mg, mo, mf) = (
        mtf50(&good, &grid),
        mtf50(&off, &grid),
        mtf50(&fixed, &grid),
    );
    println!("MTF50: aligned {mg:.4}  0.4-site error {mo:.4}  error with local alignment {mf:.4}");
    assert!(
        mo < 0.95 * mg,
        "the shifted alignment {mo} should lose sharpness against {mg}"
    );
    assert!(mf > 0.97 * mg, "local alignment {mf} should recover {mg}");
}

#[test]
fn bracket_merge_extends_range() {
    // Exposures 1, 2.2 and 3.5 of the chart; the shortest is the reference.
    // The long frames clip the edge panel's bright side, and their shadows
    // are cleaner, so the merge must take highlights from the short frame
    // and shadows mostly from the long ones.
    let scene = Scene::chart(W, H);
    let truth = handheld_transforms(3, W, H, 21);
    let exposures = [1.0f32, 2.2, 3.5];
    let frames: Vec<MemoryFrame> = std::thread::scope(|s| {
        let handles: Vec<_> = truth
            .iter()
            .zip(exposures)
            .enumerate()
            .map(|(k, (t, e))| {
                let scene = &scene;
                s.spawn(move || render_exposed(scene, W, H, t, k, Some(NOISE), 7, e))
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let fr = refs(&frames);
    let a = align(&fr, 0, &test_align_params()).unwrap();
    assert_eq!(a.accepted(), 3);
    for (f, e) in a.frames.iter().zip(exposures) {
        assert!(
            (f.gain * e as f64 - 1.0).abs() < 0.02,
            "{}: gain {} for exposure {e}",
            f.label,
            f.gain
        );
    }
    let bracket = ResolutionParams {
        bracket: true,
        scale: 1.0,
        ..ResolutionParams::default()
    };
    // Without exposure scaling robustness would reject the brighter frames
    // as motion; with it off they are averaged in unscaled.
    let plain = ResolutionParams {
        bracket: false,
        robustness: None,
        ..bracket.clone()
    };
    let green = |rgb: &[f32], grid: &OutputGrid| {
        let c = crop(rgb, grid.width, 3, 4, 4, grid.width - 8, grid.height - 8);
        patch_noise(&c, grid.width - 8, WB)
    };
    let mean_truth = |grid: &OutputGrid| {
        let t = scene.truth(grid, 0);
        t.chunks_exact(3).map(|p| p[1] as f64).sum::<f64>() / (t.len() / 3) as f64
    };

    // Shadows: the merge against the inverse-variance combination of the
    // frames merged alone (each in its own units; times its gain in the
    // reference's).
    let dark = region((10, 60, 40, 300));
    let (grid, merged) = merge(&fr, &a, &bracket, dark);
    let m = green(&merged, &grid);
    let mut inverse = 0.0;
    let mut alone_std = Vec::new();
    for k in 0..3 {
        let mut alone = a.clone();
        for (j, f) in alone.frames.iter_mut().enumerate() {
            if j != k {
                f.to_frame = None;
            }
        }
        let (_, rgb) = merge(&fr, &alone, &bracket, dark);
        let s = green(&rgb, &grid).std[1] * a.frames[k].gain;
        inverse += 1.0 / (s * s);
        alone_std.push(s);
    }
    let ideal = inverse.sqrt().recip();
    let t = mean_truth(&grid);
    println!(
        "bracket shadows: merge std {:.6} (ideal {ideal:.6}; alone {alone_std:.6?}), mean {:.5} vs truth {t:.5}",
        m.std[1], m.mean[1]
    );
    assert!(
        m.std[1] < 1.15 * ideal,
        "merge noise {} vs ideal {ideal}",
        m.std[1]
    );
    assert!(m.std[1] < 0.6 * alone_std[0]);
    assert!((m.mean[1] / t - 1.0).abs() < 0.02);
    // Control: without exposure scaling the shadows come out too bright.
    let (_, unscaled) = merge(&fr, &a, &plain, dark);
    let u = green(&unscaled, &grid);
    assert!(
        u.mean[1] / t > 1.3,
        "unscaled shadows {} vs truth {t}",
        u.mean[1]
    );

    // Highlights clipped in both long frames come from the short one.
    let bright = region((100, 60, 40, 300));
    let (bgrid, bmerged) = merge(&fr, &a, &bracket, bright);
    let b = green(&bmerged, &bgrid);
    let bt = mean_truth(&bgrid);
    println!(
        "bracket highlights: merge mean {:.5} vs truth {bt:.5}",
        b.mean[1]
    );
    assert!(
        bt * 2.2 > 1.0,
        "the bright side must clip in the long frames"
    );
    assert!((b.mean[1] / bt - 1.0).abs() < 0.02);
}
