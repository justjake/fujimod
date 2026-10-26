use super::*;
use crate::cache::CaptureCache;
use crate::touches::{self, TouchPhase};
use std::{
    cell::{Cell, RefCell},
    sync::atomic::{AtomicU64, Ordering},
};

struct Output(PathBuf);
impl Output {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "fujimod-recording-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Output {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn frame(value: u8) -> Frame {
    Frame::rgb(320, 240, vec![value; 320 * 240 * 3]).unwrap()
}

#[test]
fn screenshots_need_no_encoder_and_keep_pixels() {
    let output = Output::new();
    let mut recorder = Recorder::new(output.0.clone(), 15, false).unwrap();
    let frame = frame(37);
    recorder.frame(frame.clone(), 0.).unwrap();
    recorder.input("menu_ok_button", true, 0.1).unwrap();
    recorder.still(frame.clone(), 1.5).unwrap();
    let digest = recorder.screenshot(&frame, "screenshots/001.png").unwrap();
    recorder.finish().unwrap();
    recorder.finish().unwrap();
    assert_eq!(digest.len(), 64);
    assert_eq!(
        Frame::from_png(&std::fs::read(output.0.join("screenshots/001.png")).unwrap())
            .unwrap()
            .pixels(),
        frame.pixels()
    );
    assert!(!output.0.join("video.mp4").exists());
    assert!(!output.0.join("ffmpeg.log").exists());
    assert_eq!(recorder.metrics.annotated_frames, 0);
    assert_eq!(recorder.metrics.written_frames, 0);
    assert!(!recorder.due(1., true).unwrap_or(false));
}
#[test]
fn remote_and_emulator_rgb_share_the_same_capture_and_png() {
    let remote = Frame::bgra(
        2,
        2,
        12,
        &[
            0, 0, 255, 255, 0, 255, 0, 255, 0, 0, 0, 0, 255, 0, 0, 255, 255, 255, 255, 0,
        ],
    )
    .unwrap();
    let emulator = Frame::rgb(2, 2, vec![255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0]).unwrap();
    assert_eq!(remote.png().unwrap(), emulator.png().unwrap());
    assert!(Frame::bgra(2, 2, 8, &[0; 15]).is_err());
    assert!(Frame::rgb(0, 2, vec![]).is_err());
    assert!(Frame::from_png(b"bad").is_err());
}
#[test]
fn captured_bytes_and_device_key_invalidate_without_any_guest_execution() {
    let mut cache = CaptureCache::<Vec<u8>>::default();
    let ram = RefCell::new(vec![1, 2, 3]);
    let renders = Cell::new(0);
    let mut sample = |key| {
        cache
            .capture(
                vec![key],
                |_, _| Ok(ram.borrow().clone()),
                |read| {
                    renders.set(renders.get() + 1);
                    read(0x1000, 3)
                },
            )
            .unwrap()
    };
    assert_eq!(sample(1), vec![1, 2, 3]);
    assert_eq!(sample(1), vec![1, 2, 3]);
    assert_eq!(renders.get(), 1);
    ram.borrow_mut()[0] = 9;
    assert_eq!(sample(1), vec![9, 2, 3]);
    assert_eq!(renders.get(), 2);
    sample(2);
    assert_eq!(renders.get(), 3);
    // Restore the original source bytes, as after a native keyframe restore.
    ram.borrow_mut()[0] = 1;
    assert_eq!(sample(1), vec![1, 2, 3]);
    assert_eq!(renders.get(), 4);
    assert_eq!(cache.hits, 1);
}
#[test]
fn failed_captures_never_become_cached_successes() {
    let mut cache = CaptureCache::<u8>::default();
    assert!(
        cache
            .capture(
                vec![1],
                |_, _| Ok(vec![]),
                |_| Err("unsupported pixels".into())
            )
            .is_err()
    );
    assert_eq!(
        cache
            .capture(vec![1], |_, _| Ok(vec![]), |_| Ok(7))
            .unwrap(),
        7
    );
    assert_eq!(cache.renders, 1);
    assert_eq!(cache.hits, 0);
}
#[test]
fn sampling_holds_inputs_and_idle_compaction_preserve_timestamp_order() {
    let output = Output::new();
    let mut r = Recorder::new(output.0.clone(), 10, false).unwrap();
    assert!(r.due(0., false).unwrap());
    r.frame(frame(1), 0.).unwrap();
    r.frame(frame(1), 3.).unwrap();
    assert_eq!(r.frames, 4);
    assert!((r.skipped_seconds - 2.6).abs() < 1e-8);
    assert!(r.frame(frame(2), 2.).is_err());
    assert!(r.input("catalog-control", true, 3.).is_ok());
    assert!(r.input("catalog-control", false, 3.).is_ok());
    assert!(r.input("catalog-control", true, 2.).is_err());
    assert!(r.still(frame(2), f64::NAN).is_err());
    assert!(r.screenshot(&frame(1), "../escape.png").is_err());
    for _ in 0..50 {
        r.still(frame(2), 0.01).unwrap();
    }
    assert_eq!(r.frames, 9);
    r.finish().unwrap();
    assert!(r.frame(frame(1), 5.).is_err());
}
#[test]
fn optional_video_shares_rendering_and_annotates_only_changed_frames() {
    if Encoders::check().is_err() {
        return;
    }
    let output = Output::new();
    let mut r = Recorder::new(output.0.clone(), 10, true).unwrap();
    assert!(r.input("catalog-control", true, 0.).is_err());
    let image = frame(40);
    r.frame(image.clone(), 0.).unwrap();
    r.still(image.clone(), 0.5).unwrap();
    r.input("menu_ok_button", true, 0.).unwrap();
    r.still(image.clone(), 0.5).unwrap();
    r.input("menu_ok_button", false, 0.).unwrap();
    r.still(image.clone(), 0.5).unwrap();
    r.finish().unwrap();
    assert_eq!(r.metrics.changed_frames, 1);
    assert_eq!(r.metrics.annotated_frames, 2);
    assert_eq!(r.metrics.written_frames, 15);
    let decode = |name| {
        std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(output.0.join(name))
            .args(["-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
            .output()
            .unwrap()
            .stdout
    };
    let clean = decode("video-clean.mp4");
    let annotated = decode("video.mp4");
    assert_eq!(clean.len(), 15 * 320 * 240 * 3);
    assert_eq!(clean.len(), annotated.len());
    assert!(clean.iter().all(|v| v.abs_diff(40) < 6));
    assert!(annotated.iter().any(|v| v.abs_diff(40) > 40));
}

#[test]
fn badge_history_keeps_five_presses_and_fades_each_release_independently() {
    let output = Output::new();
    let mut r = Recorder::new(output.0.clone(), 15, false).unwrap();
    r.input("selector.down", true, 0.).unwrap();
    r.input("selector.down", false, 0.).unwrap();
    r.frames = 2;
    r.input("selector.down", true, 0.).unwrap();
    r.input("selector.down", false, 0.).unwrap();
    r.frames = 71;
    assert_eq!(
        r.current_labels(),
        vec![("selector.down".into(), 170), ("selector.down".into(), 255)]
    );
    r.frames = 75;
    assert_eq!(r.current_labels(), vec![("selector.down".into(), 85)]);
    r.frames = 77;
    assert!(r.current_labels().is_empty());
    let names = [
        "selector.up",
        "selector.down",
        "selector.left",
        "selector.right",
        "quick_menu_button",
        "menu_ok_button",
    ];
    for name in names {
        r.input(name, true, 0.).unwrap();
        r.input(name, false, 0.).unwrap();
    }
    assert_eq!(
        r.current_labels()
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        names[1..]
    );
}

#[test]
fn previous_frames_keep_their_input_state_and_final_observation_is_visible() {
    if Encoders::check().is_err() {
        return;
    }
    let output = Output::new();
    let mut r = Recorder::new(output.0.clone(), 10, true).unwrap();
    r.frame(frame(40), 0.).unwrap();
    r.input("selector.right", true, 0.5).unwrap();
    r.frame(frame(40), 1.).unwrap();
    r.frame(frame(90), 1.).unwrap();
    r.finish().unwrap();
    let count = r.frames;
    r.finish().unwrap();
    assert_eq!(r.frames, count);
    let result = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(output.0.join("video.mp4"))
        .args(["-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
        .output()
        .unwrap();
    assert!(result.status.success());
    let frames = result
        .stdout
        .as_chunks::<{ 320 * 240 * 3 }>()
        .0
        .iter()
        .collect::<Vec<_>>();
    assert_eq!(frames.len(), 10);
    assert!(
        frames[..4]
            .iter()
            .all(|f| f.iter().all(|v| v.abs_diff(40) < 6))
    );
    assert!(
        frames[4..9]
            .iter()
            .all(|f| f.iter().any(|v| v.abs_diff(40) > 40))
    );
    // The final changed frame is encoded even with no later timestamp or hold.
    assert!(frames[9][..320 * 60 * 3].iter().all(|v| v.abs_diff(90) < 6));
}

#[test]
fn touches_land_follow_the_finger_and_end_by_their_contact() {
    use touches::Mark;
    let output = Output::new();
    let mut r = Recorder::new(output.0.clone(), 30, false).unwrap();
    assert!(r.touch(TouchPhase::Move, 1., 1., 0.).is_err());
    assert!(r.touch(TouchPhase::End, 1., 1., 0.).is_err());
    assert!(r.touch(TouchPhase::Start, f64::NAN, 1., 0.).is_err());
    // A 320x240 source fitted into 640x480 doubles positions and the radius.
    let marks =
        |r: &mut Recorder| touches::marks(&mut r.touches, r.frames, r.fps, (320, 240), (640, 480));
    let circle = |mark: &Mark| match *mark {
        Mark::Circle {
            x,
            y,
            radius,
            opacity,
        } => (x, y, radius, opacity),
        Mark::Ripple { .. } => panic!("expected the finger's circle, got {mark:?}"),
    };
    r.touch(TouchPhase::Start, 100., 60., 0.).unwrap();
    assert!(r.touch(TouchPhase::Start, 1., 1., 0.).is_err());
    // It lands at 70% of its radius and grows to full size.
    let (x, y, radius, opacity) = circle(&marks(&mut r)[0]);
    assert_eq!((x, y, opacity), (200., 120., 1.));
    assert!((radius - 44. * 0.7).abs() < 1e-9, "{radius}");
    r.frames = 3;
    r.touch(TouchPhase::Move, 110., 70., 0.).unwrap();
    assert_eq!(
        marks(&mut r),
        vec![Mark::Circle {
            x: 220.,
            y: 140.,
            radius: 44.,
            opacity: 1.
        }]
    );
    // A 0.1 s tap still shows as down for its 0.15 s minimum...
    r.touch(TouchPhase::End, 120., 70., 0.).unwrap();
    r.frames = 4;
    assert_eq!(
        marks(&mut r),
        vec![Mark::Circle {
            x: 240.,
            y: 140.,
            radius: 44.,
            opacity: 1.
        }]
    );
    // ...then its circle fades while a ring grows from it.
    r.frames = 6;
    let now = marks(&mut r);
    assert_eq!(now.len(), 2);
    assert!(circle(&now[0]).3 < 1.);
    let Mark::Ripple {
        radius, opacity, ..
    } = now[1]
    else {
        panic!("{now:?}")
    };
    assert!(
        radius > 44. && radius < 66. && opacity < 1. && opacity > 0.,
        "{now:?}"
    );
    // The ripple is done 0.35 s after the minimum.
    r.frames = 15;
    assert!(marks(&mut r).is_empty());
    assert!(r.touches.is_empty());

    // A longer contact fades in place without a ripple.
    r.touch(TouchPhase::Start, 10., 10., 0.).unwrap();
    r.frames = 30;
    r.touch(TouchPhase::End, 10., 10., 0.).unwrap();
    r.frames = 34;
    let now = marks(&mut r);
    assert_eq!(now.len(), 1);
    let (x, y, radius, opacity) = circle(&now[0]);
    assert_eq!((x, y, radius), (20., 20., 44.));
    assert!(opacity > 0. && opacity < 1.);
    // Letterboxed sources keep the centered fit offset.
    let (x, y, radius, _) =
        circle(&touches::marks(&mut r.touches, r.frames, r.fps, (320, 240), (640, 240))[0]);
    assert_eq!((x, y, radius), (170., 10., 22.));
    r.frames = 40;
    assert!(marks(&mut r).is_empty());
}

#[test]
fn touch_edges_are_anti_aliased() {
    // A light ring on black: the edge blends through partial coverage, and a
    // half-pixel shift of the center changes the pixels, not a whole step.
    let draw = |cx: f64| {
        let mut canvas = crate::canvas::Canvas::from_rgb(&[0; 64 * 64 * 3], 64, 64);
        touches::draw_on(
            &mut canvas,
            &[touches::Mark::Circle {
                x: cx,
                y: 32.,
                radius: 20.,
                opacity: 1.,
            }],
        );
        canvas.rgb()
    };
    let pixels = draw(32.);
    let row: Vec<u8> = (0..64).map(|x| pixels[(32 * 64 + x) * 3]).collect();
    let partial = row
        .iter()
        .filter(|&&v| v > 0 && v < row.iter().copied().max().unwrap())
        .count();
    assert!(partial >= 2, "edge pixels take partial coverage: {row:?}");
    assert_eq!(row[0], 0);
    assert_eq!(row[11], row[52], "the ring is symmetric about its center");
    assert_ne!(
        draw(32.25),
        pixels,
        "sub-pixel centers move the edge smoothly"
    );
}

#[test]
fn touches_mark_both_videos() {
    if Encoders::check().is_err() {
        return;
    }
    let output = Output::new();
    let mut r = Recorder::new(output.0.clone(), 10, true).unwrap();
    r.frame(frame(0), 0.).unwrap();
    r.touch(TouchPhase::Start, 160., 120., 0.).unwrap();
    r.frame(frame(0), 0.3).unwrap();
    r.touch(TouchPhase::End, 160., 120., 0.3).unwrap();
    r.frame(frame(0), 1.).unwrap();
    r.finish().unwrap();
    let decode = |name: &str| {
        let result = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(output.0.join(name))
            .args(["-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
            .output()
            .unwrap();
        assert!(result.status.success());
        result.stdout
    };
    let center = (120 * 320 + 160) * 3;
    let size = 320 * 240 * 3;
    let annotated = decode("video.mp4");
    let clean = decode("video-clean.mp4");
    assert!(annotated[center] > 80, "the finger is drawn while down");
    assert!(clean[center] > 80, "the clean video shows the finger too");
    let last = annotated.chunks(size).last().unwrap();
    assert!(
        last.iter().all(|&v| v < 12) && clean.chunks(size).last().unwrap().iter().all(|&v| v < 12),
        "the circle is gone after its fade"
    );
}
