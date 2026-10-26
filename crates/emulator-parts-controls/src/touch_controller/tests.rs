use super::*;

fn e(tick: u64, contact: usize, phase: Phase, x: u16, y: u16) -> ContactInput {
    ContactInput {
        tick,
        contact,
        phase,
        x,
        y,
    }
}

fn run(script: &[ContactInput], extra_ticks: bool) -> Vec<(u64, [u8; 11])> {
    let mut m = TouchController::new(1000).unwrap();
    m.inject(0, script).unwrap();
    let (mut now, mut reads) = (0, 0);
    let mut out = Vec::new();
    while let Some(due) = m.next_event_tick(now, reads, true) {
        if extra_ticks {
            for t in now..due {
                assert!(m.tick(t, reads, true).unwrap().is_none());
            }
        }
        now = due;
        if let Some(frame) = m.tick(now, reads, true).unwrap() {
            out.push((now, frame));
            reads += 1;
        }
    }
    assert_eq!(m.reports_completed, out.len() as u64);
    out
}

#[test]
fn tap_flick_and_sample_edges_do_not_depend_on_tick_frequency() {
    let script = [
        e(25, 0, Phase::Down, 500, 210),
        e(27, 0, Phase::Move, 490, 210),
        e(29, 0, Phase::Move, 480, 210),
        e(38, 0, Phase::Move, 440, 210),
        e(45, 0, Phase::Up, 440, 210),
        e(100, 0, Phase::Down, 4095, 4094),
        e(120, 0, Phase::Up, 4095, 4094),
    ];
    let a = run(&script, false);
    assert_eq!(a, run(&script, true));
    assert_eq!(
        a.iter().map(|(_, f)| f[1]).collect::<Vec<_>>(),
        [2, 3, 3, 4, 0, 2, 1, 0]
    );
    assert_eq!(&a[5].1[3..7], &[255, 254, 255, 128]);
}

#[test]
fn pinch_spread_preserve_both_contacts() {
    let reports = run(
        &[
            e(0, 0, Phase::Down, 100, 100),
            e(0, 1, Phase::Down, 300, 100),
            e(10, 1, Phase::Move, 200, 100),
            e(20, 1, Phase::Move, 400, 100),
            e(30, 1, Phase::Up, 400, 100),
            e(40, 0, Phase::Up, 100, 100),
        ],
        false,
    );
    assert_eq!(
        reports.iter().map(|(_, f)| f[1]).collect::<Vec<_>>(),
        [2, 2, 5, 6, 0, 0]
    );
    assert_eq!(&reports[2].1[..3], &[2, 5, 0x11]);
    assert_eq!(&reports[2].1[7..11], &[200, 100, 0, 128]);
}

#[test]
fn unread_reports_have_no_poll_deadline_and_snapshots_are_observational() {
    let mut m = TouchController::new(1000).unwrap();
    m.inject(
        0,
        &[e(0, 0, Phase::Down, 10, 10), e(1, 0, Phase::Up, 10, 10)],
    )
    .unwrap();
    assert!(m.tick(0, 0, true).unwrap().is_some());
    m.tick(1, 0, true).unwrap();
    assert_eq!(m.next_event_tick(100, 0, true), None);
    let saved = serde_json::to_string(&m).unwrap();
    for _ in 0..10 {
        assert_eq!(m.next_event_tick(100, 0, true), None);
    }
    assert_eq!(serde_json::to_string(&m).unwrap(), saved);
    let mut restored: TouchController = serde_json::from_str(&saved).unwrap();
    assert_eq!(restored.next_event_tick(100, 1, false), Some(100));
    assert!(restored.tick(100, 1, false).unwrap().is_none());
    assert_eq!(restored.next_event_tick(100, 1, false), None);
    assert_eq!(restored.next_event_tick(100, 1, true), Some(100));
    assert_eq!(restored.tick(100, 1, true).unwrap().unwrap()[1], 1);
}

#[test]
fn invalid_scripts_are_atomic_and_never_accept_past_or_impossible_contacts() {
    let mut m = TouchController::new(1000).unwrap();
    for script in [
        vec![e(9, 0, Phase::Down, 0, 0)],
        vec![e(10, 2, Phase::Down, 0, 0)],
        vec![e(10, 0, Phase::Down, 4096, 0)],
        vec![e(10, 0, Phase::Move, 0, 0)],
        vec![e(u64::MAX, 0, Phase::Down, 0, 0)],
        vec![e(10, 0, Phase::Down, 0, 0), e(11, 0, Phase::Down, 0, 0)],
    ] {
        assert!(m.inject(10, &script).is_err());
        assert_eq!(m.pending_inputs(), 0);
    }
}

#[test]
fn coalesced_moves_slow_release_and_long_hold_do_not_invent_a_flick_or_tap() {
    let reports = run(
        &[
            e(0, 0, Phase::Down, 100, 200),
            e(3, 0, Phase::Move, 110, 200),
            e(4, 0, Phase::Move, 120, 200),
            e(7, 0, Phase::Move, 125, 200),
            e(500, 0, Phase::Up, 125, 200),
            e(600, 0, Phase::Down, 10, 20),
            e(900, 0, Phase::Up, 10, 20),
        ],
        false,
    );
    assert_eq!(
        reports.iter().map(|(t, f)| (*t, f[1])).collect::<Vec<_>>(),
        [(0, 2), (10, 3), (500, 0), (600, 2), (730, 9), (900, 0)]
    );
    assert_eq!(reports[1].1[3], 125);
}

#[test]
fn a_held_contact_reports_long_press_and_press_within_the_slop_as_on_the_camera() {
    let reports = run(
        &[
            e(0, 0, Phase::Down, 300, 200),
            e(200, 0, Phase::Move, 302, 201),
            e(400, 0, Phase::Move, 299, 200),
            e(600, 0, Phase::Up, 299, 200),
            e(700, 0, Phase::Down, 300, 200),
            e(760, 0, Phase::Move, 320, 200),
            e(900, 0, Phase::Up, 320, 200),
            e(1000, 0, Phase::Down, 300, 200),
            e(1110, 0, Phase::Up, 300, 200),
            e(1200, 0, Phase::Down, 300, 200),
            e(1311, 0, Phase::Up, 300, 200),
        ],
        false,
    );
    assert_eq!(
        reports.iter().map(|(t, f)| (*t, f[1])).collect::<Vec<_>>(),
        [
            (0, 2),
            (130, 9),
            (200, 2),
            (400, 2),
            (600, 0),
            (700, 2),
            (760, 3),
            (900, 0),
            (1000, 2),
            (1110, 1),
            (1120, 0),
            (1200, 2),
            (1311, 0)
        ]
    );
    assert_eq!(&reports[1].1[..7], &[1, 9, 1, 44, 200, 1, 128]);
}

#[test]
fn fragmented_scripts_keep_contact_state_and_report_spacing() {
    let mut m = TouchController::new(1000).unwrap();
    m.inject(0, &[e(0, 0, Phase::Down, 12, 34)]).unwrap();
    assert_eq!(m.tick(0, 0, true).unwrap().unwrap()[1], 2);
    m.inject(1, &[e(1, 0, Phase::Up, 12, 34)]).unwrap();
    assert!(m.tick(1, 1, true).unwrap().is_none());
    assert_eq!(m.next_event_tick(1, 1, true), Some(10));
    assert_eq!(m.tick(10, 1, true).unwrap().unwrap()[1], 1);
    assert!(m.tick(11, 2, true).unwrap().is_none());
    assert_eq!(m.next_event_tick(11, 2, true), Some(20));
    assert_eq!(m.tick(20, 2, true).unwrap().unwrap()[1], 0);
}
