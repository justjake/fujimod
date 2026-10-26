use super::*;

fn configuration() -> Configuration {
    Configuration {
        peer_parameters: 0xb801,
        parameters: 0xb801,
        identification: (0..128).collect(),
        status: 0x0080,
        aperture_ring: 0,
        focus_ring: 0xffff,
        detail_status: 0x14,
        read_catalogs: vec![],
    }
}
fn pins() -> Pins {
    Pins {
        supplies: [Some(true); 2],
        reset_released: Some(true),
    }
}
fn port() -> Port {
    Port::new(configuration(), 240).unwrap()
}
fn frame(word: u8) -> Frame {
    Frame {
        bits: 8,
        mode: 3,
        word: word as u16,
    }
}
fn byte(port: &mut Port, word: u8, now: u64) -> u16 {
    let f = frame(word);
    let token = port.begin(f).unwrap();
    port.finish(token, f, now).unwrap()
}

#[test]
fn all_driven_supplies_and_reset_start_one_exact_deadline() {
    let mut p = port();
    assert!(p.ready_high());
    assert_eq!(p.next_event_tick(0, Pins::default()), None);
    assert!(p.begin(frame(0)).is_err());
    let partial = Pins {
        supplies: [Some(true), None],
        reset_released: Some(true),
    };
    assert_eq!(p.next_event_tick(10, partial), Some(10));
    p.tick(10, partial).unwrap();
    assert_eq!(p.next_event_tick(10, partial), None);
    p.tick(100, pins()).unwrap();
    assert_eq!(p.next_event_tick(100, pins()), Some(340));
    let before = p.save();
    for at in [100, 101, 339] {
        p.tick(at, pins()).unwrap();
        assert_eq!(p.save(), before);
        assert!(p.ready_high());
    }
    p.tick(340, pins()).unwrap();
    assert!(!p.ready_high());
    assert_eq!(p.next_event_tick(340, pins()), None);
    for (i, v) in [0, 0, 0x3f, 0xc6].into_iter().enumerate() {
        assert_eq!(byte(&mut p, v, 350 + i as u64), 0);
    }
    assert_eq!(p.save()["link"]["state"]["frames"], 4);
}

#[test]
fn startup_is_independent_of_tick_frequency_and_power_loss_cancels_it() {
    let a = port();
    let b = port();
    for p in [&a, &b] {
        p.tick(10, pins()).unwrap();
    }
    for at in [11, 100, 249, 250] {
        a.tick(at, pins()).unwrap();
    }
    b.tick(1000, pins()).unwrap();
    assert_eq!(a.save(), b.save());
    let p = port();
    p.tick(10, pins()).unwrap();
    let off = Pins {
        supplies: [Some(false), Some(true)],
        ..pins()
    };
    p.tick(249, off).unwrap();
    assert!(p.ready_high());
    p.tick(1000, off).unwrap();
    assert_eq!(p.next_event_tick(1000, off), None);
    p.tick(1001, pins()).unwrap();
    assert_eq!(p.next_event_tick(1001, pins()), Some(1241));
}

#[test]
fn reset_clears_protocol_and_invalidates_frames_from_an_earlier_power_epoch() {
    let mut p = port();
    p.tick(10, pins()).unwrap();
    p.tick(250, pins()).unwrap();
    byte(&mut p, 0, 260);
    byte(&mut p, 0, 270);
    let token = p.begin(frame(0x3f)).unwrap();
    let reset = Pins {
        reset_released: Some(false),
        ..pins()
    };
    p.tick(280, reset).unwrap();
    assert!(p.ready_high());
    p.tick(300, pins()).unwrap();
    p.tick(540, pins()).unwrap();
    let before = p.save();
    assert!(
        p.finish(token, frame(0x3f), 550)
            .unwrap_err()
            .contains("power/reset")
    );
    assert_eq!(p.save(), before);
    assert_eq!(p.save()["link"]["state"]["frames"], 0);
    assert_eq!(byte(&mut p, 0, 560), 0);
}

#[test]
fn checkpoint_restore_is_atomic_and_resumes_power_deadlines_and_partial_frames() {
    let mut a = port();
    a.tick(10, pins()).unwrap();
    let mut b = port();
    b.restore(&a.save()).unwrap();
    for p in [&a, &b] {
        p.tick(250, pins()).unwrap();
    }
    byte(&mut a, 0, 260);
    byte(&mut a, 0, 270);
    b.restore(&a.save()).unwrap();
    for (i, v) in [0x3f, 0xc6, 8, 0x10, 0x80, 0x22].into_iter().enumerate() {
        assert_eq!(
            byte(&mut a, v, 280 + i as u64),
            byte(&mut b, v, 280 + i as u64)
        );
        assert_eq!(a.save(), b.save());
    }
    let before = a.save();
    for bad in [
        {
            let mut v = before.clone();
            v["startup_ticks"] = json!(1);
            v
        },
        {
            let mut v = before.clone();
            v["state"]["phase"] = json!({"kind":"off"});
            v
        },
        {
            let mut v = before.clone();
            v["link"]["state"]["receive_counter"] = json!(8);
            v
        },
    ] {
        assert!(a.restore(&bad).is_err());
        assert_eq!(a.save(), before);
    }
    assert!(Port::new(configuration(), 0).is_err());
}
