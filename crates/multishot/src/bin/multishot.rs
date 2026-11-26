//! `fujimod-multishot`: align, merge and measure X-Trans RAF bursts.

use fujimod_multishot::align::{
    AlignParams, Alignment, Pyramid, align_with_priors, register, sharpest, shortest_exposure,
};
use fujimod_multishot::demosaic::{self, Demosaic};
use fujimod_multishot::dng::{DngInfo, DngWriter, XYZ_TO_SRGB};
use fujimod_multishot::fit::Affine;
use fujimod_multishot::local::{LocalParams, residual_fields};
use fujimod_multishot::measure::{crop, edge_raggedness, luma, patch_noise, slanted_edge};
use fujimod_multishot::merge::{
    ColorMerge, ImageSink, MergeReport, OutputGrid, Region, ResolutionParams, SharpnessParams,
    StripSink, Tee, merge_resolution,
};
use fujimod_multishot::pixelshift::{ShiftPlan, predicted_alignment, report as shift_report};
use fujimod_multishot::plane::read_block_planes;
use fujimod_multishot::preview::{PreviewSink, auto_gain, write_png};
use fujimod_multishot::raf::Raf;
use fujimod_multishot::source::FrameSource;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "\
usage:
  fujimod-multishot info RAF...
  fujimod-multishot look RAF --out PNG [--region X,Y,W,H]
  fujimod-multishot align --out DIR [--reference N] RAF...
  fujimod-multishot align-jpeg --out DIR [--reference N] JPG...
  fujimod-multishot merge --out DIR [options] RAF...
  fujimod-multishot synth --out DIR [--width W] [--height H] [--frames N]
  fujimod-multishot measure --out DIR --edge X,Y,W,H [--flat X,Y,W,H] [options] RAF...
  fujimod-multishot pixelshift --out DIR --commanded <X,Y X,Y ...> [--sites-per-count SX,SY] RAF...
  fujimod-multishot measure-hdr --out DIR --dark X,Y,W,H --bright X,Y,W,H [options] RAF...

options:
  --scale S            output pixels per sensor site (default 1.5)
  --sigma S            kernel standard deviation in sites (default 0.38)
  --radius R           kernel radius in sites (default 1.5)
  --color MODE         green-guided (default) or independent
  --bracket            merge an exposure bracket (HDR): scale frames by exposure, weight by noise
  --sharpness P        weight frames by local sharpness relative to the reference, power P
  --sharpness-radius N sharpness averaging radius in 3x3 blocks (default 4)
  --local              add per-tile residual shifts to each frame's global alignment
  --local-tile N       residual alignment tile size in sites (default 96)
  --reference N        reference frame index (default: the shortest exposure of a
                       bracket, else the sharpest frame)
  --alignment FILE     reuse an alignment.json
  --region X,Y,W,H     merge only this reference region (sites)
  --align-region X,Y,W,H  fit alignment only on this subject region (sites)
  --strip-rows N       output rows per strip (default 48)
  --frames N           use only the first N frames
  --no-robustness      merge without motion rejection
  --preview-factor N   preview downscale (default 4)";

struct Args {
    command: String,
    files: Vec<PathBuf>,
    opts: std::collections::HashMap<String, String>,
    flags: std::collections::HashSet<String>,
}

fn parse_args() -> Result<Args, String> {
    let mut it = std::env::args().skip(1);
    let command = it.next().ok_or(USAGE)?;
    let mut files = Vec::new();
    let mut opts = std::collections::HashMap::new();
    let mut flags = std::collections::HashSet::new();
    while let Some(a) = it.next() {
        if let Some(name) = a.strip_prefix("--") {
            if name == "no-robustness" || name == "local" || name == "bracket" {
                flags.insert(name.to_string());
            } else {
                let v = it.next().ok_or(format!("--{name} needs a value"))?;
                opts.insert(name.to_string(), v);
            }
        } else {
            files.push(PathBuf::from(a));
        }
    }
    Ok(Args {
        command,
        files,
        opts,
        flags,
    })
}

impl Args {
    fn num<T: std::str::FromStr>(&self, name: &str, default: T) -> Result<T, String> {
        match self.opts.get(name) {
            None => Ok(default),
            Some(v) => v
                .parse()
                .map_err(|_| format!("--{name}: cannot parse {v:?}")),
        }
    }
    fn region(&self, name: &str) -> Result<Option<Region>, String> {
        let Some(v) = self.opts.get(name) else {
            return Ok(None);
        };
        let p: Vec<usize> = v
            .split(',')
            .map(|s| {
                s.trim()
                    .parse()
                    .map_err(|_| format!("--{name}: expected X,Y,W,H"))
            })
            .collect::<Result<_, _>>()?;
        if p.len() != 4 {
            return Err(format!("--{name}: expected X,Y,W,H"));
        }
        Ok(Some(Region {
            x: p[0],
            y: p[1],
            width: p[2],
            height: p[3],
        }))
    }
    fn out(&self) -> Result<PathBuf, String> {
        self.opts
            .get("out")
            .map(PathBuf::from)
            .ok_or_else(|| "--out is required".to_string())
    }
}

fn open_all(files: &[PathBuf], limit: Option<usize>) -> Result<Vec<Raf>, String> {
    if files.is_empty() {
        return Err("no RAF files given".into());
    }
    let mut files = files.to_vec();
    files.sort();
    if let Some(n) = limit {
        files.truncate(n);
    }
    files
        .iter()
        .map(|p| Raf::open(p).map_err(|e| format!("{}: {e}", p.display())))
        .collect()
}

fn info(args: &Args) -> Result<(), String> {
    for raf in open_all(&args.files, None)? {
        let cal = raf.calibration();
        println!("{}", raf.path.display());
        println!(
            "  model {}  plane {}x{}  active {:?}  bits {}  {:?}",
            raf.model, raf.raw_width, raf.raw_height, raf.active, raf.bits, raf.compression
        );
        println!(
            "  black {:?}  white {}  wb_grb {:?}",
            &raf.black_recorded[..raf.black_recorded.len().min(6)],
            cal.white,
            raf.wb_grb
        );
        let rows = raf.cfa_raw.rows();
        for r in rows {
            println!(
                "  cfa {}",
                r.iter()
                    .map(|c| ["R", "G", "B"][*c as usize])
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        }
        println!("  exposure {:?}", raf.exposure);
        // Per-tile-position means over the central quarter, to check the
        // CFA phase: positions of one color should agree.
        let (w, h) = (raf.width(), raf.height());
        let (y0, rows_n) = ((h * 3 / 8) / 6 * 6, (h / 4) / 6 * 6);
        let mut raw = vec![0u16; rows_n * w];
        raf.read_rows(y0, &mut raw).map_err(|e| e.to_string())?;
        let mut sum = [[0f64; 6]; 6];
        let mut n = [[0f64; 6]; 6];
        let mut max = 0u16;
        for y in 0..rows_n {
            for x in w * 3 / 8..w * 5 / 8 {
                let v = raw[y * w + x];
                max = max.max(v);
                sum[(y0 + y) % 6][x % 6] += v as f64;
                n[(y0 + y) % 6][x % 6] += 1.0;
            }
        }
        let cfa = raf.cfa();
        let mut by_color = [Vec::new(), Vec::new(), Vec::new()];
        for y in 0..6 {
            for x in 0..6 {
                by_color[cfa.color(x, y) as usize]
                    .push(sum[y][x] / n[y][x] - cal.black[y][x] as f64);
            }
        }
        for (c, v) in by_color.iter().enumerate() {
            let mean = v.iter().sum::<f64>() / v.len() as f64;
            let spread = v.iter().map(|x| (x - mean).abs()).fold(0.0, f64::max);
            println!(
                "  center mean {} {:.1} (max deviation {:.1})",
                ["R", "G", "B"][c],
                mean,
                spread
            );
        }
        println!("  center max raw {max}");
    }
    Ok(())
}

fn look(args: &Args) -> Result<(), String> {
    if args
        .files
        .first()
        .and_then(|p| p.extension())
        .is_some_and(|e| e.eq_ignore_ascii_case("jpg"))
    {
        let out = args.out()?;
        let plane =
            fujimod_multishot::jpeg::block_luma(&args.files[0]).map_err(|e| e.to_string())?;
        let rgb: Vec<f32> = plane.data.iter().flat_map(|&v| [v, v, v]).collect();
        write_png(&out, &rgb, plane.width, plane.height, [1.0; 3], 1.0)
            .map_err(|e| e.to_string())?;
        println!("wrote {} (1 pixel = 3x3 JPEG pixels)", out.display());
        return Ok(());
    }
    let rafs = open_all(&args.files, Some(1))?;
    let raf = &rafs[0];
    let out = args.out()?;
    let wb = raf.calibration().wb;
    if let Some(region) = args.region("region")? {
        let grid = OutputGrid::new(region, 1.0);
        let rgb = demosaic::baseline(raf, &grid, Demosaic::Gaussian).map_err(|e| e.to_string())?;
        write_png(&out, &rgb, grid.width, grid.height, wb, auto_gain(&rgb, wb))
            .map_err(|e| e.to_string())?;
    } else {
        let planes = read_block_planes(raf, true).map_err(|e| e.to_string())?;
        let rgb: Vec<f32> = planes
            .rgb
            .iter()
            .flat_map(|p| [if p[0] < 0.0 { -p[0] - 1.0 } else { p[0] }, p[1], p[2]])
            .collect();
        write_png(
            &out,
            &rgb,
            planes.grid.width,
            planes.grid.height,
            wb,
            auto_gain(&rgb, wb),
        )
        .map_err(|e| e.to_string())?;
        println!(
            "block preview: 1 pixel = 3x3 sites; site x = 3*px + {} , y = 3*py + {}",
            planes.grid.ox, planes.grid.oy
        );
    }
    println!("wrote {}", out.display());
    Ok(())
}

fn get_alignment(
    args: &Args,
    frames: &[&dyn FrameSource],
    out: &Path,
    shift: Option<&ShiftPlan>,
) -> Result<Alignment, String> {
    if let Some(path) = args.opts.get("alignment") {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
        let a: Alignment = serde_json::from_str(&text).map_err(|e| format!("{path}: {e}"))?;
        if a.frames.len() != frames.len() {
            return Err(format!(
                "{path} aligns {} frames, {} given",
                a.frames.len(),
                frames.len()
            ));
        }
        for (f, s) in a.frames.iter().zip(frames) {
            if f.label != s.label() {
                return Err(format!(
                    "{path} lists {} where {} was given",
                    f.label,
                    s.label()
                ));
            }
        }
        return Ok(a);
    }
    let started = std::time::Instant::now();
    let reference = match args.opts.get("reference") {
        Some(v) => v
            .parse()
            .map_err(|_| "--reference: expected a frame index".to_string())?,
        // A pixel-shift sequence is registered to its first frame at the
        // starting position.
        None => match (shift, shortest_exposure(frames)) {
            (Some(plan), _) => plan
                .counts
                .iter()
                .position(|c| *c == [0.0, 0.0])
                .unwrap_or(0),
            (None, Some(k)) => k,
            (None, None) => sharpest(frames).map_err(|e| e.to_string())?,
        },
    };
    if reference >= frames.len() {
        return Err("--reference is past the last frame".into());
    }
    let mut params = AlignParams::default();
    if let Some(r) = args.region("align-region")? {
        // Sites to block-plane coordinates; coverage is judged within the
        // subject, so the whole-frame grid gate does not apply.
        params.subject = Some([
            r.x as f64 / 3.0,
            r.y as f64 / 3.0,
            r.width as f64 / 3.0,
            r.height as f64 / 3.0,
        ]);
        params.fit.min_grid_cells = 1;
    }
    let priors = shift.map(|plan| plan.priors(reference));
    let a = align_with_priors(frames, reference, &params, priors.as_deref())
        .map_err(|e| e.to_string())?;
    eprintln!(
        "aligned {} frames in {:.1} s (reference {}: {})",
        frames.len(),
        started.elapsed().as_secs_f64(),
        reference,
        frames[reference].label()
    );
    for f in &a.frames {
        match (&f.to_frame, &f.rejected) {
            (Some(t), _) => eprintln!(
                "  {:<16} shift {:+8.3} {:+8.3}  rot {:+.5}  inliers {:4}/{:4}  fit rms {:.3}  refine rms {:.5}",
                f.label,
                t.0[4] + (t.0[0] - 1.0) * a.width as f64 / 2.0 + t.0[1] * a.height as f64 / 2.0,
                t.0[5] + t.0[2] * a.width as f64 / 2.0 + (t.0[3] - 1.0) * a.height as f64 / 2.0,
                t.0[2].atan2(t.0[0]),
                f.inliers,
                f.matches.accepted,
                f.fit_rms,
                f.refine.as_ref().map_or(0.0, |r| r.residual_rms),
            ),
            (None, Some(r)) => eprintln!("  {:<16} REJECTED: {r}", f.label),
            (None, None) => {}
        }
    }
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    std::fs::write(
        out.join("alignment.json"),
        serde_json::to_string_pretty(&a).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    Ok(a)
}

fn resolution_params(args: &Args) -> Result<ResolutionParams, String> {
    let d = ResolutionParams::default();
    Ok(ResolutionParams {
        scale: args.num("scale", d.scale)?,
        sigma: args.num("sigma", d.sigma)?,
        radius: args.num("radius", d.radius)?,
        strip_rows: args.num("strip-rows", d.strip_rows)?,
        robustness: if args.flags.contains("no-robustness") {
            None
        } else {
            d.robustness.clone()
        },
        color: match args.opts.get("color").map(String::as_str) {
            None => d.color,
            Some("green-guided") => ColorMerge::GreenGuided,
            Some("independent") => ColorMerge::Independent,
            Some(v) => return Err(format!("--color: unknown mode {v:?}")),
        },
        local: if args.flags.contains("local") {
            let l = LocalParams::default();
            Some(LocalParams {
                tile: args.num("local-tile", l.tile)?,
                ..l
            })
        } else {
            None
        },
        bracket: args.flags.contains("bracket"),
        sharpness: match args.opts.get("sharpness") {
            None => None,
            Some(_) => {
                let d = SharpnessParams::default();
                Some(SharpnessParams {
                    power: args.num("sharpness", d.power)?,
                    radius: args.num("sharpness-radius", d.radius)?,
                })
            }
        },
        region: args.region("region")?,
        ..d
    })
}

fn merge_cmd(args: &Args) -> Result<(), String> {
    let out = args.out()?;
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let limit = args
        .opts
        .get("frames")
        .map(|v| {
            v.parse()
                .map_err(|_| "--frames: expected a count".to_string())
        })
        .transpose()?;
    let rafs = open_all(&args.files, limit)?;
    let frames: Vec<&dyn FrameSource> = rafs.iter().map(|r| r as &dyn FrameSource).collect();
    let alignment = get_alignment(args, &frames, &out, None)?;
    let params = resolution_params(args)?;
    let reference = &rafs[alignment.reference];
    let region = params.region.unwrap_or(Region {
        x: 0,
        y: 0,
        width: reference.width(),
        height: reference.height(),
    });
    let grid = OutputGrid::new(region, params.scale);
    let wb = reference.calibration().wb;
    let info = DngInfo {
        make: "FUJIFILM".into(),
        model: reference.model.clone(),
        wb,
        color_matrix: XYZ_TO_SRGB,
    };
    let mut dng = DngWriter::create(
        out.join("merged.dng"),
        grid.width,
        grid.height,
        params.strip_rows,
        &info,
    )
    .map_err(|e| e.to_string())?;
    let factor = args.num(
        "preview-factor",
        if params.region.is_some() { 1 } else { 4 },
    )?;
    let mut preview = PreviewSink::new(grid.width, grid.height, factor);
    let report = {
        let mut tee = Tee(vec![&mut dng as &mut dyn StripSink, &mut preview]);
        merge_resolution(&frames, &alignment, &params, &mut tee).map_err(|e| e.to_string())?
    };
    dng.finish().map_err(|e| e.to_string())?;
    preview
        .finish(out.join("preview.png"), wb)
        .map_err(|e| e.to_string())?;
    std::fs::write(
        out.join("report.json"),
        serde_json::to_string_pretty(&json!({ "params": params, "merge": report, "alignment_peak_bytes": alignment.peak_bytes })).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    println!(
        "merged {} of {} frames into {}x{} in {:.1} s; {} strips of {} rows, peak strip memory {:.1} MiB",
        alignment.accepted(),
        frames.len(),
        report.width,
        report.height,
        report.seconds,
        report.strips,
        report.strip_rows,
        report.peak_strip_bytes as f64 / (1 << 20) as f64
    );
    for f in &report.frames {
        println!(
            "  {:<16} used {:5}  mean weight {:.3}  rejected {:.3}",
            f.label, f.used, f.mean_weight, f.rejected_fraction
        );
    }
    println!("wrote {}", out.display());
    Ok(())
}

struct Variant {
    name: &'static str,
    rgb: Vec<f32>,
}

fn run_variants(
    frames: &[&dyn FrameSource],
    alignment: &Alignment,
    params: &ResolutionParams,
    region: Region,
) -> Result<(OutputGrid, Vec<Variant>, MergeReport), String> {
    let p = ResolutionParams {
        region: Some(region),
        ..params.clone()
    };
    let grid = OutputGrid::new(region, p.scale);
    let mut merged = ImageSink::new(grid.width, grid.height);
    let report = merge_resolution(frames, alignment, &p, &mut merged).map_err(|e| e.to_string())?;
    let mut single_alignment = alignment.clone();
    for (k, f) in single_alignment.frames.iter_mut().enumerate() {
        if k != alignment.reference {
            f.to_frame = None;
        }
    }
    let mut single = ImageSink::new(grid.width, grid.height);
    merge_resolution(frames, &single_alignment, &p, &mut single).map_err(|e| e.to_string())?;
    let baseline = demosaic::baseline(frames[alignment.reference], &grid, Demosaic::Gaussian)
        .map_err(|e| e.to_string())?;
    let markesteijn = demosaic::baseline(
        frames[alignment.reference],
        &grid,
        Demosaic::Markesteijn { passes: 3 },
    )
    .map_err(|e| e.to_string())?;
    Ok((
        grid,
        vec![
            Variant {
                name: "merge",
                rgb: merged.rgb,
            },
            Variant {
                name: "single-frame-merge",
                rgb: single.rgb,
            },
            Variant {
                name: "demosaic-upscale",
                rgb: baseline,
            },
            Variant {
                name: "markesteijn3-upscale",
                rgb: markesteijn,
            },
        ],
        report,
    ))
}

fn measure_cmd(args: &Args) -> Result<(), String> {
    let out = args.out()?;
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let limit = args
        .opts
        .get("frames")
        .map(|v| {
            v.parse()
                .map_err(|_| "--frames: expected a count".to_string())
        })
        .transpose()?;
    let rafs = open_all(&args.files, limit)?;
    let frames: Vec<&dyn FrameSource> = rafs.iter().map(|r| r as &dyn FrameSource).collect();
    let alignment = get_alignment(args, &frames, &out, None)?;
    let params = resolution_params(args)?;
    let wb = rafs[alignment.reference].calibration().wb;
    let edge = args.region("edge")?.ok_or("--edge X,Y,W,H is required")?;
    let mut results = serde_json::Map::new();
    let (grid, variants, report) = run_variants(&frames, &alignment, &params, edge)?;
    let gain = auto_gain(&variants[2].rgb, wb);
    println!(
        "edge region {:?} at scale {} ({} frames merged)",
        edge,
        params.scale,
        alignment.accepted()
    );
    println!(
        "  {:<20} {:>12} {:>14} {:>10}",
        "variant", "angle", "MTF50 cy/site", "MTF@0.25"
    );
    let mut mtf = serde_json::Map::new();
    for v in &variants {
        write_png(
            out.join(format!("edge-{}.png", v.name)),
            &v.rgb,
            grid.width,
            grid.height,
            wb,
            gain,
        )
        .map_err(|e| e.to_string())?;
        let l = luma(&v.rgb, wb);
        match slanted_edge(&l, grid.width, grid.height) {
            Ok(m) => {
                println!(
                    "  {:<20} {:>11.2}° {:>14.4} {:>10.3}",
                    v.name,
                    m.angle_deg,
                    m.mtf50 * params.scale,
                    m.mtf_at_quarter
                );
                mtf.insert(v.name.into(), json!({ "mtf50_cycles_per_site": m.mtf50 * params.scale, "mtf50_cycles_per_output_pixel": m.mtf50, "angle_deg": m.angle_deg, "curve": m.curve }));
                if let Some(r) = edge_raggedness(&l, grid.width, grid.height, m.line) {
                    let r = r / params.scale;
                    println!(
                        "  {:<20} {:>12} {:>14.4}  (edge raggedness, sites)",
                        "", "", r
                    );
                    if let Some(e) = mtf.get_mut(v.name) {
                        e["raggedness_sites"] = json!(r);
                    }
                }
            }
            Err(e) => println!("  {:<20} {e}", v.name),
        }
        // Green alone: the densest CFA color, so its MTF separates
        // registration from red/blue sampling density.
        let g: Vec<f32> = v.rgb.as_chunks::<3>().0.iter().map(|p| p[1]).collect();
        if let Ok(m) = slanted_edge(&g, grid.width, grid.height) {
            println!(
                "  {:<20} {:>12} {:>14.4}  (green)",
                "",
                "",
                m.mtf50 * params.scale
            );
            if let Some(e) = mtf.get_mut(v.name) {
                e["green_mtf50_cycles_per_site"] = json!(m.mtf50 * params.scale);
            }
        }
    }
    // The reference's edge line in site coordinates, to check where each
    // frame's own edge lands after its transform.
    let ref_line = slanted_edge(&luma(&variants[2].rgb, wb), grid.width, grid.height)
        .ok()
        .map(|m| m.line.map(|(x, y)| grid.to_site(x, y)));
    // Each accepted frame's own sharpness at the edge: its demosaiced,
    // upscaled MTF50 over the region shifted by its motion, and the distance
    // of its edge, mapped into the reference, from the reference's edge.
    println!("  per-frame demosaic MTF50 at the edge (focus and blur differ between frames):");
    let mut per_frame = Vec::new();
    let fields = match &params.local {
        Some(lp) => {
            residual_fields(&frames, &alignment, edge, params.radius + 2.0, lp)
                .map_err(|e| e.to_string())?
                .0
        }
        None => vec![None; frames.len()],
    };
    for (k, f) in alignment.frames.iter().enumerate() {
        let Some(t) = f.to_frame else { continue };
        let (cx, cy) = (
            edge.x as f64 + edge.width as f64 / 2.0,
            edge.y as f64 + edge.height as f64 / 2.0,
        );
        let (u, v) = t.apply(cx, cy);
        let (dx, dy) = ((u - cx).round() as i64, (v - cy).round() as i64);
        let (x, y) = (edge.x as i64 + dx, edge.y as i64 + dy);
        if x < 0
            || y < 0
            || x as usize + edge.width > frames[k].width()
            || y as usize + edge.height > frames[k].height()
        {
            continue;
        }
        let r = Region {
            x: x as usize,
            y: y as usize,
            ..edge
        };
        let g = OutputGrid::new(r, params.scale);
        let rgb =
            demosaic::baseline(frames[k], &g, Demosaic::Gaussian).map_err(|e| e.to_string())?;
        let edge_fit = slanted_edge(&luma(&rgb, wb), g.width, g.height);
        // Perpendicular offset (sites) of this frame's edge midpoint, mapped
        // to the reference, from the reference edge line.
        // The same after the frame's residual field, when local alignment
        // is enabled.
        let offsets = match (&edge_fit, ref_line, t.inverse()) {
            (Ok(m), Some([(x0, y0), (x1, y1)]), Some(inv)) => {
                let (px, py) = g.to_site(
                    0.5 * (m.line[0].0 + m.line[1].0),
                    0.5 * (m.line[0].1 + m.line[1].1),
                );
                let (dx, dy) = (x1 - x0, y1 - y0);
                let across =
                    |(qx, qy): (f64, f64)| ((qx - x0) * dy - (qy - y0) * dx) / dx.hypot(dy);
                let global = inv.apply(px, py);
                let local = fields[k].as_ref().map(|field| {
                    let (sx, sy) = field.at(global.0, global.1);
                    across(inv.apply(px - sx, py - sy))
                });
                Some((across(global), local))
            }
            _ => None,
        };
        let offset = offsets.map(|o| o.0);
        let local_offset = offsets.and_then(|o| o.1);
        match &edge_fit {
            Ok(m) => println!(
                "    {:<16} {:.4}   edge offset {:+.3} sites, after local field {:+.3}",
                f.label,
                m.mtf50 * params.scale,
                offset.unwrap_or(f64::NAN),
                local_offset.unwrap_or(f64::NAN)
            ),
            Err(e) => println!("    {:<16} {e}", f.label),
        }
        per_frame.push(json!({ "frame": f.label, "mtf50_cycles_per_site": edge_fit.as_ref().ok().map(|m| m.mtf50 * params.scale), "edge_offset_sites": offset, "local_edge_offset_sites": local_offset }));
    }
    results.insert(
        "edge".into(),
        json!({ "region": edge, "mtf": mtf, "per_frame_demosaic": per_frame, "merge": report }),
    );
    if let Some(flat) = args.region("flat")? {
        let (fgrid, fvariants, _) = run_variants(&frames, &alignment, &params, flat)?;
        println!("flat region {flat:?}");
        let mut noise = serde_json::Map::new();
        for v in &fvariants {
            // Inset to stay clear of the region border.
            let m = 4usize;
            let c = crop(
                &v.rgb,
                fgrid.width,
                3,
                m,
                m,
                fgrid.width - 2 * m,
                fgrid.height - 2 * m,
            );
            let n = patch_noise(&c, fgrid.width - 2 * m, wb);
            println!(
                "  {:<20} luma relative std {:.5}  rgb std {:.5} {:.5} {:.5}",
                v.name, n.luma_relative_std, n.std[0], n.std[1], n.std[2]
            );
            noise.insert(v.name.into(), serde_json::to_value(&n).unwrap());
        }
        results.insert("flat".into(), json!({ "region": flat, "noise": noise }));
    }
    results.insert("params".into(), serde_json::to_value(&params).unwrap());
    results.insert("frames".into(), json!(frames.len()));
    results.insert("merged_frames".into(), json!(alignment.accepted()));
    std::fs::write(
        out.join("measure.json"),
        serde_json::to_string_pretty(&results).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    println!("wrote {}", out.display());
    Ok(())
}

/// Registers camera JPEGs (no RAW) with the same matcher, fit and refinement,
/// reporting the motion between frames.
fn align_jpeg(args: &Args) -> Result<(), String> {
    let out = args.out()?;
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let mut files = args.files.clone();
    files.sort();
    if files.len() < 2 {
        return Err("align-jpeg needs at least two JPEGs".into());
    }
    let reference: usize = args.num("reference", 0)?;
    let params = AlignParams::default();
    let started = std::time::Instant::now();
    let ref_plane =
        fujimod_multishot::jpeg::block_luma(&files[reference]).map_err(|e| e.to_string())?;
    let pyr = Pyramid::new(ref_plane.clone(), params.matching.levels);
    let (w, h) = (ref_plane.width as f64 * 3.0, ref_plane.height as f64 * 3.0);
    println!(
        "reference {} ({}x{} pixels)",
        files[reference].display(),
        w,
        h
    );
    println!(
        "  {:<14} {:>9} {:>9} {:>9} {:>8} {:>9} {:>8} {:>9}",
        "frame", "dx px", "dy px", "rot deg", "scale", "inliers", "fit rms", "ref rms"
    );
    let mut prior = Affine::IDENTITY;
    let mut rows = Vec::new();
    for (k, f) in files.iter().enumerate() {
        if k == reference {
            continue;
        }
        let plane = fujimod_multishot::jpeg::block_luma(f).map_err(|e| e.to_string())?;
        let reg = register(&pyr, &plane, &prior, &params);
        let name = f
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        match reg.refined {
            Some(t) => {
                prior = t;
                let site = t.rescaled(3.0, 1.0, 1.0);
                let (cx, cy) = site.apply(w / 2.0, h / 2.0);
                let (dx, dy) = (cx - w / 2.0, cy - h / 2.0);
                let rot = site.0[2].atan2(site.0[0]).to_degrees();
                let scale = site.determinant().sqrt();
                let fit = reg.fit.as_ref().unwrap();
                let rr = reg.refine.as_ref().map_or(0.0, |r| r.residual_rms);
                println!(
                    "  {:<14} {:>9.3} {:>9.3} {:>9.4} {:>8.5} {:>4}/{:<4} {:>8.3} {:>9.5}",
                    name, dx, dy, rot, scale, fit.inlier_count, reg.matches.accepted, fit.rms, rr
                );
                rows.push(json!({ "frame": name, "to_frame": site, "center_shift_px": [dx, dy], "rotation_deg": rot, "scale": scale,
                    "inliers": fit.inlier_count, "matches": reg.matches, "fit_rms_blocks": fit.rms, "refine": reg.refine }));
            }
            None => {
                println!(
                    "  {:<14} REJECTED: {}  ({:?})",
                    name,
                    reg.rejected.clone().unwrap_or_default(),
                    reg.matches
                );
                rows.push(
                    json!({ "frame": name, "rejected": reg.rejected, "matches": reg.matches }),
                );
            }
        }
    }
    println!(
        "{} frames in {:.1} s",
        files.len(),
        started.elapsed().as_secs_f64()
    );
    let report = json!({ "reference": files[reference].file_name().map(|n| n.to_string_lossy().to_string()), "frames": rows, "params": params });
    std::fs::write(
        out.join("alignment-jpeg.json"),
        serde_json::to_string_pretty(&report).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    println!("wrote {}", out.display());
    Ok(())
}

/// Writes uncompressed RAFs of the synthetic chart seen through handheld-like
/// motion, for end-to-end timing and memory checks at full size.
fn synth(args: &Args) -> Result<(), String> {
    use fujimod_multishot::raf::{RafFixture, Rect};
    use fujimod_multishot::synthetic::{NoiseSpec, Scene, handheld_transforms, render_fast};
    let out = args.out()?;
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let w: usize = args.num("width", 6240)?;
    let h: usize = args.num("height", 4158)?;
    let n: usize = args.num("frames", 20)?;
    let scene = Scene::chart(w, h);
    let noise = Some(NoiseSpec {
        shot: 2e-5,
        read: 0.001,
    });
    let transforms = handheld_transforms(n, w, h, 17);
    for (k, t) in transforms.iter().enumerate() {
        let data = render_fast(&scene, w, h, t, k, noise, 7);
        let fixture = RafFixture {
            model: "synthetic",
            raw_width: w,
            raw_height: h,
            active: Rect {
                left: 0,
                top: 0,
                width: w,
                height: h,
            },
            cfa_raw: fujimod_multishot::xtrans::XTrans::standard(),
            bits: 14,
            black: 1024,
            wb_grb: [1000, 2000, 1667],
            exposure_seconds: (1, 1000),
            iso: 400,
            data: &data,
        };
        fixture
            .write(out.join(format!("SYNTH{k:03}.RAF")))
            .map_err(|e| e.to_string())?;
    }
    std::fs::write(
        out.join("truth.json"),
        serde_json::to_string_pretty(&transforms).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    println!("wrote {n} frames of {w}x{h} to {}", out.display());
    Ok(())
}

/// `region` moved by the frame's motion at its center (rounded to sites), or
/// `None` when the moved region leaves the frame.
fn follow(region: Region, t: &Affine, frame: &dyn FrameSource) -> Option<Region> {
    let (cx, cy) = (
        region.x as f64 + region.width as f64 / 2.0,
        region.y as f64 + region.height as f64 / 2.0,
    );
    let (u, v) = t.apply(cx, cy);
    let (x, y) = (
        region.x as i64 + (u - cx).round() as i64,
        region.y as i64 + (v - cy).round() as i64,
    );
    (x >= 0
        && y >= 0
        && x as usize + region.width <= frame.width()
        && y as usize + region.height <= frame.height())
    .then_some(Region {
        x: x as usize,
        y: y as usize,
        ..region
    })
}

/// Fraction of a frame's sites in `region` at the clip level.
fn clipped_fraction(frame: &dyn FrameSource, region: Region) -> Result<f64, String> {
    let w = frame.width();
    let mut raw = vec![0u16; region.height * w];
    frame
        .read_rows(region.y, &mut raw)
        .map_err(|e| e.to_string())?;
    let cal = frame.calibration();
    let mut clipped = 0usize;
    for r in 0..region.height {
        for x in region.x..region.x + region.width {
            if cal.linear(raw[r * w + x], x, region.y + r).1 {
                clipped += 1;
            }
        }
    }
    Ok(clipped as f64 / (region.width * region.height) as f64)
}

/// Dynamic range of a bracket merge against each single frame: the green
/// noise floor in a dark flat patch (plane removed, inset from the border)
/// relative to the clip level, in stops, and the clipped fraction of a
/// bright region. Each single frame goes through the same merge alone, so
/// the kernels filter its noise alike; it comes out in its own units (white
/// 1.0), and the merge's white is the shortest exposure's.
fn measure_hdr_cmd(args: &Args) -> Result<(), String> {
    let out = args.out()?;
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let rafs = open_all(&args.files, None)?;
    let frames: Vec<&dyn FrameSource> = rafs.iter().map(|r| r as &dyn FrameSource).collect();
    let alignment = get_alignment(args, &frames, &out, None)?;
    let params = ResolutionParams {
        bracket: true,
        ..resolution_params(args)?
    };
    let dark = args.region("dark")?.ok_or("--dark X,Y,W,H is required")?;
    let bright = args
        .region("bright")?
        .ok_or("--bright X,Y,W,H is required")?;
    let wb = rafs[alignment.reference].calibration().wb;
    let inset = 4usize;
    let noise_of = |rgb: &[f32], grid: &OutputGrid| {
        let c = crop(
            rgb,
            grid.width,
            3,
            inset,
            inset,
            grid.width - 2 * inset,
            grid.height - 2 * inset,
        );
        patch_noise(&c, grid.width - 2 * inset, wb)
    };
    let p = ResolutionParams {
        region: Some(dark),
        ..params.clone()
    };
    let grid = OutputGrid::new(dark, p.scale);
    let mut merged = ImageSink::new(grid.width, grid.height);
    let report =
        merge_resolution(&frames, &alignment, &p, &mut merged).map_err(|e| e.to_string())?;
    let mn = noise_of(&merged.rgb, &grid);
    let merged_dr = (1.0 / mn.std[1]).log2();
    println!(
        "dark {dark:?}: merge ({} frames, output scale {:.4}) green mean {:.6} std {:.6} -> {:.2} stops",
        alignment.accepted(),
        report.output_scale,
        mn.mean[1],
        mn.std[1],
        merged_dr
    );
    let mut singles = Vec::new();
    let mut best: Option<(String, f64)> = None;
    for (k, f) in alignment.frames.iter().enumerate() {
        let Some(t) = f.to_frame else { continue };
        let (Some(d), Some(b)) = (follow(dark, &t, frames[k]), follow(bright, &t, frames[k]))
        else {
            println!("  {:<16} regions leave the frame", f.label);
            continue;
        };
        let _ = d;
        let mut alone = alignment.clone();
        for (j, a) in alone.frames.iter_mut().enumerate() {
            if j != k {
                a.to_frame = None;
            }
        }
        let mut sink = ImageSink::new(grid.width, grid.height);
        merge_resolution(&frames, &alone, &p, &mut sink).map_err(|e| e.to_string())?;
        let n = noise_of(&sink.rgb, &grid);
        let dr = (1.0 / n.std[1]).log2();
        let clip = clipped_fraction(frames[k], b)?;
        println!(
            "  {:<16} gain {:>6.3}  green mean {:.6} std {:.6} -> {:.2} stops  bright clipped {:.2}%",
            f.label,
            f.gain,
            n.mean[1],
            n.std[1],
            dr,
            100.0 * clip
        );
        if best.as_ref().is_none_or(|b| dr > b.1) {
            best = Some((f.label.clone(), dr));
        }
        singles.push(json!({ "frame": f.label, "gain": f.gain, "dark_noise": n, "dynamic_range_stops": dr, "bright_clipped_fraction": clip }));
    }
    // The merge clips only where every frame clips: at most the shortest
    // exposure's (largest gain's) clipped fraction.
    let shortest = alignment
        .frames
        .iter()
        .enumerate()
        .filter(|(_, f)| f.to_frame.is_some())
        .max_by(|a, b| a.1.gain.total_cmp(&b.1.gain))
        .map(|(k, _)| k)
        .ok_or("no accepted frames")?;
    let merged_clip = match follow(
        bright,
        &alignment.frames[shortest].to_frame.unwrap(),
        frames[shortest],
    ) {
        Some(b) => clipped_fraction(frames[shortest], b)?,
        None => f64::NAN,
    };
    let (best_label, best_dr) = best.ok_or("no single frame measured")?;
    println!(
        "merge {:.2} stops vs best single frame {} {:.2} stops: +{:.2} stops; merge bright clipped at most {:.2}%",
        merged_dr,
        best_label,
        best_dr,
        merged_dr - best_dr,
        100.0 * merged_clip
    );
    let results = json!({
        "dark": dark,
        "bright": bright,
        "merge": { "dark_noise": mn, "dynamic_range_stops": merged_dr, "bright_clipped_fraction_at_most": merged_clip, "report": report },
        "frames": singles,
        "best_single": { "frame": best_label, "dynamic_range_stops": best_dr },
        "gain_stops": merged_dr - best_dr,
        "params": params,
    });
    std::fs::write(
        out.join("measure-hdr.json"),
        serde_json::to_string_pretty(&results).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    println!("wrote {}", out.display());
    Ok(())
}

/// Registers a pixel-shift sequence with the commanded offsets as priors and
/// reports commanded against measured displacement per frame. `--commanded`
/// lists one offset per file, in the order the files are given.
fn pixelshift_cmd(args: &Args) -> Result<(), String> {
    let out = args.out()?;
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let rafs = open_all(&args.files, None)?;
    let frames: Vec<&dyn FrameSource> = rafs.iter().map(|r| r as &dyn FrameSource).collect();
    let pair = |s: &str, what: &str| -> Result<[f64; 2], String> {
        let v: Vec<f64> = s
            .split(',')
            .map(|p| p.trim().parse::<f64>())
            .collect::<Result<_, _>>()
            .map_err(|_| format!("{what}: expected X,Y, got {s:?}"))?;
        match v[..] {
            [x, y] => Ok([x, y]),
            _ => Err(format!("{what}: expected X,Y, got {s:?}")),
        }
    };
    let counts: Vec<[f64; 2]> = args
        .opts
        .get("commanded")
        .ok_or("--commanded \"X,Y X,Y ...\" (counts per frame) is required")?
        .split_whitespace()
        .map(|p| pair(p, "--commanded"))
        .collect::<Result<_, _>>()?;
    if counts.len() != frames.len() {
        return Err(format!(
            "--commanded gives {} offsets for {} frames",
            counts.len(),
            frames.len()
        ));
    }
    // Offsets follow the files as given; frames are opened in path order.
    let mut given: Vec<(&PathBuf, [f64; 2])> = args.files.iter().zip(counts).collect();
    given.sort_by(|a, b| a.0.cmp(b.0));
    let counts: Vec<[f64; 2]> = given.into_iter().map(|g| g.1).collect();
    let plan = ShiftPlan {
        counts,
        sites_per_count: match args.opts.get("sites-per-count") {
            Some(s) => pair(s, "--sites-per-count")?,
            None => ShiftPlan::XH2S_SITES_PER_COUNT,
        },
    };
    let alignment = get_alignment(args, &frames, &out, Some(&plan))?;
    let r = shift_report(&plan, &alignment);
    println!(
        "  {:<14} {:>14} {:>18} {:>18} {:>18} {:>18}",
        "frame", "counts", "commanded (sites)", "measured", "measured-commanded", "predicted"
    );
    for f in &r.frames {
        let fmt = |v: Option<[f64; 2]>| match v {
            Some([x, y]) => format!("{x:+7.3} {y:+7.3}"),
            None => "rejected".into(),
        };
        println!(
            "  {:<14} {:>6} {:>6}   {}   {}   {}   {}",
            f.label,
            f.counts[0],
            f.counts[1],
            fmt(Some(f.commanded)),
            fmt(f.measured),
            fmt(f.difference),
            fmt(f.predicted)
        );
    }
    if let Some([x, y]) = r.zero_spread {
        println!("  frames at the reference position spread {x:.3} x {y:.3} sites");
    }
    for (axis, fit) in ["X", "Y"].iter().zip(&r.fit) {
        if let Some(f) = fit {
            println!(
                "  {axis}: {:+.5} sites per count (1 site per {:+.1} counts), intercept {:+.3}, rms residual {:.3} over {} frames",
                f.sites_per_count,
                1.0 / f.sites_per_count,
                f.intercept,
                f.rms_residual,
                f.frames
            );
        }
    }
    std::fs::write(
        out.join("pixelshift.json"),
        serde_json::to_string_pretty(&r).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    // For `merge --alignment`: every frame at its predicted position.
    std::fs::write(
        out.join("alignment-predicted.json"),
        serde_json::to_string_pretty(&predicted_alignment(&r, &alignment)).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    println!("wrote {}", out.display());
    Ok(())
}

fn main() -> ExitCode {
    let result = parse_args().and_then(|args| match args.command.as_str() {
        "info" => info(&args),
        "look" => look(&args),
        "align" => {
            let out = args.out()?;
            let rafs = open_all(&args.files, None)?;
            let frames: Vec<&dyn FrameSource> =
                rafs.iter().map(|r| r as &dyn FrameSource).collect();
            get_alignment(&args, &frames, &out, None).map(|_| ())
        }
        "align-jpeg" => align_jpeg(&args),
        "merge" => merge_cmd(&args),
        "synth" => synth(&args),
        "measure" => measure_cmd(&args),
        "measure-hdr" => measure_hdr_cmd(&args),
        "pixelshift" => pixelshift_cmd(&args),
        _ => Err(USAGE.into()),
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
