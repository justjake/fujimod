use super::*;
use crate::backend::Backend;
#[cfg(test)]
use crate::connection::Connection;
use serde_json::json;

pub fn facts() -> Facts {
    let hash = "00".repeat(32);
    let keys = [
        ("menu_ok_button", "MENU"),
        ("disp_back_button", "BACK"),
        ("selector.up", "UP"),
        ("selector.down", "DOWN"),
        ("selector.left", "LEFT"),
        ("selector.right", "RIGHT"),
        ("playback_button", "PLAY"),
        ("wifi_button", "FN"),
        ("shutter_button", "HALF"),
        ("shutter_button", "FULL"),
        ("position_dial", "AUTO"),
        ("command_control", "PREVIOUS"),
        ("command_control", "NEXT"),
    ];
    let button = |name: &str, key: &str| json!({"name":name,"location":"rear","manual_page":null,"face":null,"icon":null,"connection_ids":[],"assignable":false,"pushable":true,"positions":[],"default_function":null,"role":null,"mechanical":false,"behavior":{"kind":"button","press":{"kind":"debug_key","name":key}}});
    let physical_controls = json!({
        "$schema":fujimod_catalog_controls::PHYSICAL_CONTROLS_SCHEMA_URI,"schema":1,"id":"synthetic-controls","body_id":"synthetic",
        "manual":{"url":"https://example.test/manual","title":"Synthetic controls","pages":[]},"analog_levels":1,
        "controls":{
            "menu_ok_button":button("MENU/OK button","MENU"),
            "playback_button":button("Playback button","PLAY"),
            "wifi_button":button("Function button","FN"),
            "shutter_button":{"name":"Shutter","location":"top","manual_page":null,"face":null,"icon":null,"connection_ids":[],"assignable":false,"pushable":true,"positions":[],"default_function":null,"role":null,"mechanical":false,"behavior":{"kind":"shutter","stages":{"half_press":[{"kind":"debug_key","name":"HALF"}],"full_press":[{"kind":"debug_key","name":"HALF"},{"kind":"debug_key","name":"FULL"}]}}},
            "position_dial":{"name":"Position dial","location":"top","manual_page":null,"face":null,"icon":null,"connection_ids":[],"assignable":false,"pushable":false,"positions":[{"id":"A","label":"A","description":null}],"default_function":null,"role":null,"mechanical":false,"behavior":{"kind":"dial","motion":{"kind":"positions","positions":[{"id":"A","key":{"kind":"debug_key","name":"AUTO"},"asserted":true}]},"press":null,"endless":false}},
            "command_control":{"name":"Command dial","location":"rear","manual_page":null,"face":null,"icon":null,"connection_ids":[],"assignable":false,"pushable":false,"positions":[],"default_function":null,"role":null,"mechanical":false,"behavior":{"kind":"dial","motion":{"kind":"steps","decrease":{"kind":"debug_key","name":"PREVIOUS"},"increase":{"kind":"debug_key","name":"NEXT"}},"press":null,"endless":true}},
            "disp_back_button":button("DISP/BACK button","BACK"),
            "selector":{"name":"Selector","location":"rear","manual_page":null,"face":null,"icon":null,"connection_ids":[],"assignable":false,"pushable":false,"positions":[],"default_function":null,"role":null,"mechanical":false,
                "behavior":{"kind":"pad","press":null,"directions":{"up":{"kind":"debug_key","name":"UP"},"down":{"kind":"debug_key","name":"DOWN"},"left":{"kind":"debug_key","name":"LEFT"},"right":{"kind":"debug_key","name":"RIGHT"}}}}
        }
    });
    let control_ids = json!({
        "$schema":fujimod_catalog_controls::CONTROL_IDS_SCHEMA_URI,"schema":1,"id":"synthetic-1-control-ids","firmware_id":"synthetic-1",
        "scanner":{"base":16,"count":1,"stride":4,"lookup":16,"sha256":hash},
        "input_readers":{"calibration":16,"scanner":{"stable_offset":0,"stable_width":1,"pressed_value":1,"released_value":0,"routine":{"address":16,"size":4,"sha256":hash}}},
        "native_key_names":{"rows":[],"reader":16,"size":4,"sha256":hash},
        "debug_keys":keys.iter().enumerate().map(|(n,(_,key))|json!({"name":key,"event":n})).collect::<Vec<_>>(),
        "inputs":keys.iter().enumerate().map(|(n,(id,key))|json!({"id":format!("input-{n}"),"event":n,"debug_key":key,"target":{"kind":"control","control_id":id},"scanner":null,"electrical":null})).collect::<Vec<_>>(),
        "undelivered_rows":[],"interfaces":{}
    });
    serde_json::from_value(json!({
        "physical_controls":physical_controls,
        "control_ids":control_ids,
        "firmware":{
            "$schema":fujimod_catalog_remote::FIRMWARE_FACTS_SCHEMA_URI,"schema":1,"firmware_id":"synthetic-1","firmware_sha256":hash,"body_id":"synthetic",
            "uncached_alias":0,
            "identity_checks":[{"address":16,"size":4,"sha256":hash}],"configuration":{"kind":"direct","address":1024},
            "state":{"ui_mode":768,"system_mode":772,"menu_active":776,"pending_mode":780,"menu_node":784,"rec_mode":3,"setup_mode":5,"readiness":{"category_one":800,"category_two":808,"input_pending":816,"excluded_mask":0,"operation_alert_timer":820,"override_address":824}},
            "menus":{"roots_address":256,"root_count":2,"roles":{"shooting":0},"root_titles":{},"root_routes":{"0":{"entry":"shooting_menu","tab":null}},"page_tabs":{},"navigation":{"shooting_first_tab":0,"shooting_last_tab":0,"setup_first_tab":1},
                "layout":{"size":32,"fields":{"menu_id":0,"type_code":4,"setting_id":8,"rows_per_page":12,"flags_word":16,"parent_pointer":20,"item_array":24,"handlers_pointer":28},"row_size":8,"row_key":0,"row_child":4,"caption_words":2,"sentinel":4294967295u32,"max_kind":8,"nowrap_flags":0},
                "cursor_address":512,"cursor":{"size":32,"fields":{"pointer":0,"cursor":8,"subcursor":12,"visible_count":16,"value_cursor":20,"raw_count":24,"filtered_count":28},"status_offset":32,"status_stride":4,"status_width":4},"q_grid":{"rows":1,"columns":1,"menu_code":1}},
            "scanout":{"address":32768,"width":1,"height":1,"pixel_format":"bgra8888","stride_bytes":4,"instance":0,"layer":0,"channel":0,"pointer_address":832,"address_mask":4294967295u32},
            "resources":{"glyphs":{"registry_address":24576,"record_size":8,"icon_index":0,"data_address_offset":0,"data_size_offset":4},"language":{"kind":"direct","address":24576},"text_record_size":8,"text_variants_offset":2,"text_metrics_offset":4,"text_pointer_offset":0,"text_unit_bytes":2,"text_max_units":64,"private_text_first":57600,"private_text_last":57855,"private_text_bias":57600,"glyph_first":61440,"glyph_last":63743,"supplementary_address":25088,"supplementary_first":16,"last_caption":16,"supplementary_stride":66,"supplementary_max_units":28,
                "bitmaps":{"table":24576,"stride":8,"final_table":24576,"final_first_index":0,"last_index":0,"last_record":24576,"layout":{"data_bytes":4,"data_offset":0,"height_bytes":2,"height_offset":4,"kind_bytes":1,"kind_offset":6,"width_bytes":2,"width_offset":8}}}
        },"menu_manifest":null
    })).unwrap()
}
/// Synthetic facts with a touch surface whose decoded report sits at 1536.
pub fn touch_facts() -> Facts {
    let mut facts = facts();
    facts.physical_controls.controls.insert(
        "touchscreen".into(),
        serde_json::from_value(json!({"name":"Touch screen","location":"rear","manual_page":null,"face":null,"icon":null,
            "connection_ids":["controls-touch"],"assignable":false,"pushable":false,"positions":[],
            "default_function":null,"role":null,"mechanical":false,"behavior":{"kind":"touch"}}))
        .unwrap(),
    );
    facts.control_ids.interfaces.insert(
        "controls-touch".into(),
        serde_json::from_value(json!({"kind":"prefixed_touch","connection_id":"controls-touch","mode":1,
            "register":0,"report_bytes":11,"contacts":2,"coordinate_bits":12,"width":720,"height":480,
            "offset_tags":[0,1],"irq":15,"priority":10,"irq_mode":2,
            "decoded":{"address":1536,"count":0,"contacts":[{"active":8,"x":10,"y":12},{"active":20,"x":22,"y":24}]}}))
        .unwrap(),
    );
    facts
}
pub struct Synthetic {
    bytes: Vec<u8>,
    now: u64,
    sequence: u64,
    edges: Vec<Input>,
    waits: Vec<ReadSet>,
    value: u32,
    stuck: bool,
    reorder_on_down: bool,
    remembered_submenu: bool,
    /// The command dial's motion is two contacts; the scanner accepts each
    /// contact edge after one continuation.
    contact_dial: bool,
    contacts: Vec<(String, bool)>,
    contact_seen: Option<(String, bool)>,
}
impl Synthetic {
    pub fn new() -> Self {
        let mut s = Self {
            bytes: vec![0; 65536],
            now: 0,
            sequence: 0,
            edges: vec![],
            waits: vec![],
            value: 0,
            stuck: false,
            reorder_on_down: false,
            remembered_submenu: false,
            contact_dial: false,
            contacts: vec![],
            contact_seen: None,
        };
        for (address, value) in [(256, 4096), (260, 4352), (768, 3), (824, 1), (832, 32768)] {
            s.word(address, value);
        }
        s.node(4096, 0, 0, 8192, 12288);
        s.node(4352, 1, 4096, 8448, 12544);
        for (a, words) in [
            (8192, vec![10, 0, 20, 4352, u32::MAX, 0]),
            (8448, vec![0, 0, 1, 0, u32::MAX, 0]),
            (12288, vec![28672, 0, 28688, 0]),
            (12544, vec![28704, 0, 28720, 0]),
        ] {
            for (n, v) in words.iter().enumerate() {
                s.word(a + n * 4, *v);
            }
        }
        for (a, label) in [
            (28672, "FIRST"),
            (28688, "SETTING"),
            (28704, "OFF"),
            (28720, "ON"),
        ] {
            s.bytes[a..a + label.len()].copy_from_slice(label.as_bytes());
        }
        s
    }
    fn word(&mut self, a: usize, v: u32) {
        self.bytes[a..a + 4].copy_from_slice(&v.to_le_bytes());
    }
    fn get(&self, a: usize) -> u32 {
        u32::from_le_bytes(self.bytes[a..a + 4].try_into().unwrap())
    }
    fn node(&mut self, a: usize, code: u32, parent: u32, rows: u32, captions: u32) {
        for (n, v) in [code, 1, 0, 7, 0, parent, rows, captions]
            .iter()
            .enumerate()
        {
            self.word(a + n * 4, *v);
        }
    }
    fn active(&mut self, node: u32, cursor: u32) {
        self.word(776, 1);
        self.word(784, node);
        self.word(512, node);
        self.word(520, cursor);
        self.word(532, 0);
    }
}
impl Backend for Synthetic {
    fn now(&self) -> u64 {
        self.now
    }
    fn memory<'a>(
        &'a mut self,
        a: u64,
        n: usize,
        reads: &'a mut ReadSet,
    ) -> LocalFuture<'a, Vec<u8>> {
        Box::pin(async move {
            reads.stores.push((a, a + n as u64));
            self.bytes
                .get(a as usize..a as usize + n)
                .map(|b| b.to_vec())
                .ok_or("Unmapped synthetic memory".into())
        })
    }
    fn input<'a>(&'a mut self, input: &'a Input, _: &'a Facts) -> LocalFuture<'a, ()> {
        Box::pin(async move {
            self.edges.push(input.clone());
            if input.state != InputState::Pressed {
                return Ok(());
            }
            match input.control.id() {
                "menu_ok_button" if self.get(776) == 0 => {
                    self.active(if self.remembered_submenu { 4352 } else { 4096 }, 0)
                }
                "menu_ok_button" => {
                    self.value = self.get(520);
                    self.active(4096, 1);
                }
                "selector.right" => self.active(4352, self.value),
                "selector.down" => {
                    self.word(520, 1);
                    if self.reorder_on_down {
                        self.bytes[8192..8208].rotate_left(8);
                        self.reorder_on_down = false;
                    }
                }
                "selector.up" => self.word(520, 0),
                "disp_back_button" if self.remembered_submenu && self.get(784) == 4352 => {
                    self.remembered_submenu = false;
                    let parent = self.get(4352 + 20);
                    if parent == 0 {
                        self.word(776, 0);
                    } else {
                        self.active(parent, 0);
                    }
                }
                "disp_back_button" => self.word(776, 0),
                _ => {}
            }
            Ok(())
        })
    }
    fn accepted(&mut self, input: &Input, facts: &Facts, reads: &mut ReadSet) -> Result<bool> {
        if !input.state.is_touch() {
            return Ok(true);
        }
        let layout = facts
            .physical_controls
            .touch(&facts.control_ids, input.control.id())?
            .decoded;
        let start = layout.address as usize;
        let size = memory::touch_report_size(layout);
        reads
            .stores
            .push((layout.address, layout.address + size as u64));
        memory::touch_decoded(layout, &self.bytes[start..start + size], &input.state)
    }
    fn frame<'a>(&'a mut self, _: &'a Facts, reads: &'a mut ReadSet) -> LocalFuture<'a, Capture> {
        Box::pin(async move {
            reads.devices.insert(7);
            Ok(Capture {
                frame: fujimod_recording::Frame::rgb(1, 1, vec![1, 2, 3])?,
                time: self.now,
                clock: Clock::GuestTicks,
                output: "lcd".into(),
                sequence: self.sequence,
                reads: reads.clone(),
                completion_reads: reads.clone(),
            })
        })
    }
    fn wait<'a>(&'a mut self, reads: &'a ReadSet, deadline: u64) -> LocalFuture<'a, ()> {
        Box::pin(async move {
            self.waits.push(reads.clone());
            self.contact_seen = self.contacts.last().cloned();
            self.now = (self.now + 1).min(deadline);
            if !self.stuck {
                self.sequence += 1;
            }
            futures_lite::future::yield_now().await;
            Ok(())
        })
    }
    fn control_position(&mut self, _: &str) -> Result<Option<String>> {
        Ok(None)
    }
    fn turn_contact(&mut self, input: &Input) -> Result<Option<String>> {
        let InputState::Turn(steps) = input.state else {
            return Ok(None);
        };
        Ok(self.contact_dial.then(|| {
            let direction = if steps < 0 { "decrease" } else { "increase" };
            format!("{}.{direction}", input.control.id())
        }))
    }
    fn contact(&mut self, actuator: &str, pressed: bool) -> Result<()> {
        self.contacts.push((actuator.into(), pressed));
        Ok(())
    }
    fn contact_accepted(
        &mut self,
        actuator: &str,
        pressed: bool,
        reads: &mut ReadSet,
    ) -> Result<bool> {
        reads.stores.push((900, 904));
        Ok(self.contact_seen == Some((actuator.into(), pressed)))
    }
}
#[test]
fn a_two_contact_dial_turns_one_scanner_accepted_pulse_per_step() {
    futures_lite::future::block_on(async {
        let mut remote = Connection::connect(Synthetic::new(), facts()).unwrap();
        remote.backend.contact_dial = true;
        let dial = control("command_control");
        remote.turn(&dial, 2, 100).await.unwrap();
        let increase = "command_control.increase".to_string();
        assert_eq!(
            remote.backend.contacts,
            [
                (increase.clone(), true),
                (increase.clone(), false),
                (increase.clone(), true),
                (increase, false)
            ]
        );
        assert!(
            remote.backend.edges.is_empty(),
            "the turn is never applied as one input"
        );
        remote.backend.contacts.clear();
        remote.turn(&dial, -1, 200).await.unwrap();
        let decrease = "command_control.decrease".to_string();
        assert_eq!(
            remote.backend.contacts,
            [(decrease.clone(), true), (decrease, false)]
        );
        // A later step's press waits for input readiness like a button's.
        remote.backend.contacts.clear();
        remote.backend.word(800, 1 << 9);
        assert!(
            remote
                .turn(&dial, 2, 300)
                .await
                .unwrap_err()
                .contains("input readiness")
        );
        assert!(remote.backend.contacts.len() <= 2);
    });
}
#[test]
fn navigate_select_reopen_and_capture() {
    futures_lite::future::block_on(async {
        let mut r = Connection::connect(Synthetic::new(), facts()).unwrap();
        let item = menu("shooting").with_key(20);
        let v = r.navigate(&item, 100).await.unwrap();
        assert!(v.menu().unwrap().selected);
        r.select(&option(item.clone(), "ON"), 100).await.unwrap();
        r.navigate(&item, 100).await.unwrap();
        operations::press(&mut r, &control("selector.right"), 100)
            .await
            .unwrap();
        let v = r.read(&option(item, "ON").into()).await.unwrap();
        assert!(v.menu().unwrap().selected);
        assert_eq!(v.menu().unwrap().value.as_deref(), Some("ON"));
        let frame = r.screenshot(100).await.unwrap();
        assert_eq!(frame.time, 1);
        assert_eq!(r.backend.waits.last().unwrap().devices, [7].into());
    });
}
#[test]
fn root_navigation_backs_out_of_a_remembered_submenu() {
    futures_lite::future::block_on(async {
        let mut backend = Synthetic::new();
        backend.remembered_submenu = true;
        let mut remote = Connection::connect(backend, facts()).unwrap();
        let result = remote
            .navigate(&menu("shooting").with_key(20), 100)
            .await
            .unwrap();
        assert!(result.menu().unwrap().selected);
        assert!(remote.backend.edges.iter().any(
            |edge| edge.control.id() == "disp_back_button" && edge.state == InputState::Pressed
        ));
    });
}
#[test]
fn root_navigation_reopens_after_back_dismisses_a_startup_modal() {
    futures_lite::future::block_on(async {
        let mut backend = Synthetic::new();
        backend.remembered_submenu = true;
        // This modal's descriptor belongs to the menu graph, but BACK closes
        // it instead of displaying its descriptor's parent.
        backend.word(4352 + 20, 0);
        let mut remote = Connection::connect(backend, facts()).unwrap();
        let result = remote
            .navigate(&menu("shooting").with_key(20), 100)
            .await
            .unwrap();
        assert!(result.menu().unwrap().selected);
        assert_eq!(
            remote
                .backend
                .edges
                .iter()
                .filter(|edge| edge.control.id() == "menu_ok_button"
                    && edge.state == InputState::Pressed)
                .count(),
            2
        );
    });
}
#[test]
fn option_value_uses_selected_row_instead_of_viewport_origin() {
    futures_lite::future::block_on(async {
        let mut backend = Synthetic::new();
        backend.active(4352, 1);
        let mut remote = Connection::connect(backend, facts()).unwrap();
        let item = menu("shooting").with_key(20);
        for (label, selected) in [("ON", true), ("OFF", false)] {
            let read_result = remote
                .read(&option(item.clone(), label).into())
                .await
                .unwrap();
            let value = read_result.menu().unwrap();
            assert_eq!(value.selected, selected);
            assert_eq!(value.value.as_deref(), Some("ON"));
        }
        assert_eq!(
            remote
                .read(&item.into())
                .await
                .unwrap()
                .menu()
                .unwrap()
                .value,
            None
        );
    });
}
#[test]
fn option_matching_and_readback_prefer_full_labels_over_compact_summaries() {
    futures_lite::future::block_on(async {
        let mut backend = Synthetic::new();
        backend.bytes[28720..28723].copy_from_slice(b"N\0\0");
        backend.bytes[28800..28807].copy_from_slice(b"NORMAL\0");
        backend.word(12544 + 8 + 4, 28800);
        let mut remote = Connection::connect(backend, facts()).unwrap();
        let item = menu("shooting").with_key(20);
        let normal = option(item.clone(), "NORMAL");
        remote.select(&normal, 100).await.unwrap();
        remote.navigate(&item, 100).await.unwrap();
        remote.press(&control("selector.right"), 100).await.unwrap();
        let result = remote.read(&normal.into()).await.unwrap();
        assert!(result.menu().unwrap().selected);
        assert_eq!(result.menu().unwrap().value.as_deref(), Some("NORMAL"));
    });
}

#[test]
fn resolution_is_live_and_strict() {
    futures_lite::future::block_on(async {
        let mut r = Connection::connect(Synthetic::new(), facts()).unwrap();
        let reference = menu("shooting").with_key(20).into();
        assert_eq!(
            r.read(&reference).await.unwrap().menu().unwrap().label,
            "SETTING"
        );
        r.backend.bytes[28688..28695].copy_from_slice(b"CHANGED");
        assert_eq!(
            r.read(&reference).await.unwrap().menu().unwrap().label,
            "CHANGED"
        );
        r.backend.word(8192, 20);
        assert!(
            r.read(&reference)
                .await
                .unwrap_err()
                .contains("matched 2 rows")
        );
    });
}
#[test]
fn frame_timeout_names_condition() {
    futures_lite::future::block_on(async {
        let mut backend = Synthetic::new();
        backend.stuck = true;
        let mut r = Connection::connect(backend, facts()).unwrap();
        assert!(
            r.screenshot(3)
                .await
                .err()
                .unwrap()
                .contains("next completed lcd frame")
        );
    });
}
#[test]
fn rec_readiness_gates_parameter_changes_without_blocking_other_category_one_work() {
    futures_lite::future::block_on(async {
        let mut remote = Connection::connect(Synthetic::new(), facts()).unwrap();
        for bit in 0..64 {
            let category = 1u64 << bit;
            remote.backend.bytes[800..808].copy_from_slice(&category.to_le_bytes());
            let result = remote.readiness(None).await.unwrap();
            assert_eq!(result.ready, bit != 9, "category-one bit {bit}");
            assert!(
                result
                    .reads
                    .stores
                    .iter()
                    .any(|&(start, end)| start <= 800 && end >= 808)
            );
        }
        remote.backend.word(800, 0x28);
        remote.backend.word(804, 0);
        remote
            .navigate(&menu("shooting").with_key(20), 100)
            .await
            .unwrap();
    });
}
#[test]
fn predicate_tracks_only_its_reads() {
    futures_lite::future::block_on(async {
        let mut r = Connection::connect(Synthetic::new(), facts()).unwrap();
        operations::wait_until(
            &mut r,
            |view| {
                Box::pin(async move {
                    let _ = view.read(&control("selector.down").into()).await?;
                    Ok(view.now() > 0)
                })
            },
            3,
            "clock progress",
        )
        .await
        .unwrap();
        assert!(!r.backend.waits[0].stores.is_empty());
        assert!(r.backend.waits[0].devices.is_empty());
    });
}

#[test]
fn raw_memory_observation_tracks_the_requested_range_for_waiting() {
    futures_lite::future::block_on(async {
        let mut remote = Connection::connect(Synthetic::new(), facts()).unwrap();
        let read = remote.read_memory(768, 4).await.unwrap();
        assert_eq!(read.address, 768);
        assert_eq!(read.bytes, 3u32.to_le_bytes());
        assert_eq!(read.reads.stores, [(768, 772)]);
    });
}

#[test]
fn wait_until_can_repeat_raw_observations_and_wait_on_their_ranges() {
    futures_lite::future::block_on(async {
        let mut remote = Connection::connect(Synthetic::new(), facts()).unwrap();
        let mut checks = 0;
        operations::wait_until(
            &mut remote,
            |view| {
                checks += 1;
                let ready = checks == 3;
                Box::pin(async move {
                    let value = view.read_memory(768, 4).await?;
                    Ok(value.bytes == 3u32.to_le_bytes() && ready)
                })
            },
            10,
            "synthetic raw condition",
        )
        .await
        .unwrap();
        assert_eq!(checks, 3);
        assert_eq!(remote.backend.waits.len(), 2);
        assert_eq!(remote.backend.waits[0].stores, [(768, 772)]);
    });
}

#[test]
fn standing_observer_runs_before_each_wait_and_joins_its_watch_set() {
    futures_lite::future::block_on(async {
        let mut remote = Connection::connect(Synthetic::new(), facts()).unwrap();
        let calls = std::rc::Rc::new(std::cell::Cell::new(0));
        let observed = calls.clone();
        remote.observe(Some(Box::new(move || {
            observed.set(observed.get() + 1);
            Ok(ReadSet {
                stores: vec![(900, 904)],
                devices: [9].into(),
            })
        })));
        let mut checks = 0;
        operations::wait_until(
            &mut remote,
            |view| {
                checks += 1;
                let ready = checks == 3;
                Box::pin(async move {
                    view.read_memory(768, 4).await?;
                    Ok(ready)
                })
            },
            10,
            "synthetic observed condition",
        )
        .await
        .unwrap();
        assert_eq!(calls.get(), 2);
        assert_eq!(remote.backend.waits.len(), 2);
        assert_eq!(remote.backend.waits[0].stores, [(768, 772), (900, 904)]);
        assert_eq!(remote.backend.waits[0].devices, [9].into());
    });
}

#[test]
fn navigation_resolves_again_after_each_edge() {
    futures_lite::future::block_on(async {
        let mut backend = Synthetic::new();
        backend.reorder_on_down = true;
        let mut remote = Connection::connect(backend, facts()).unwrap();
        let value = remote
            .navigate(&menu("shooting").with_key(20), 100)
            .await
            .unwrap();
        assert!(value.menu().unwrap().selected);
        assert_eq!(value.menu().unwrap().index, Some(0));
        assert!(
            remote
                .backend
                .edges
                .iter()
                .any(|i| i.control.id() == "selector.up")
        );
    });
}
#[test]
fn predicate_timeout_reports_last_read_result() {
    futures_lite::future::block_on(async {
        let mut remote = Connection::connect(Synthetic::new(), facts()).unwrap();
        let error = operations::wait_until(
            &mut remote,
            |view| {
                Box::pin(async move {
                    view.read(&control("selector.down").into()).await?;
                    Ok(false)
                })
            },
            2,
            "test condition",
        )
        .await
        .unwrap_err();
        assert!(error.contains("test condition"));
        assert!(error.contains("last remote reads"));
        assert!(error.contains("ControlSnapshot"));
    });
}

#[test]
fn input_task_boundary_is_live_and_tracks_tables_and_receive_states() {
    use fujimod_catalog_remote::{InputTasks, ModeBusy};
    futures_lite::future::block_on(async {
        let mut facts = facts();
        facts.input_tasks = Some(InputTasks {
            headers: vec![900, 908],
            entries: vec![0x10, 0x20],
            address_mask: 0x3fffffff,
            address_limit: 65536,
            max_count: 128,
            stride: 0x38,
            descriptor_offset: 8,
            entry_offset: 8,
            descriptor_size: 0x28,
            state_offset: 12,
            receive_state: 0x12,
            busy: vec![ModeBusy {
                mode: 3,
                address: 920,
                width: 8,
            }],
        });
        let mut backend = Synthetic::new();
        for (address, value) in [
            (900, 35000),
            (904, 2),
            (908, 35500),
            (912, 1),
            (35008, 36000),
            (35064, 36040),
            (35508, 36100),
            (36008, 0x10),
            (36048, 0x20),
            (36108, 0x30),
            (35012, 0x12),
            (35068, 0x12),
        ] {
            backend.word(address, value);
        }
        let mut r = Connection::connect(backend, facts).unwrap();
        let ready = r.readiness(None).await.unwrap();
        assert!(ready.ready);
        assert!(
            ready
                .reads
                .stores
                .iter()
                .any(|&(lo, hi)| lo <= 35012 && hi > 35012)
        );
        // A state read watches no readiness check.
        let state = r.read(&control("selector.down").into()).await.unwrap();
        assert!(
            !state
                .reads
                .stores
                .iter()
                .any(|&(lo, hi)| lo < 36110 && hi > 800)
        );
        r.backend.word(35068, 0);
        assert!(!r.readiness(None).await.unwrap().ready);
        r.backend.word(35068, 0x12);
        r.backend.word(920, 1);
        assert!(!r.readiness(None).await.unwrap().ready);
        r.backend.word(920, 0);
        r.backend.word(36108, 0x10);
        assert!(r.readiness(None).await.unwrap_err().contains("ambiguous"));
        r.backend.word(36108, 0x30);
        r.backend.word(36048, 0x40);
        assert!(r.readiness(None).await.unwrap_err().contains("missing"));
        r.backend.word(904, 129);
        assert!(r.readiness(None).await.unwrap_err().contains("unavailable"));
    });
}

#[cfg(test)]
impl Synthetic {
    /// Write the firmware's decoded report: one contact at a point, or none.
    fn decoded_touch(&mut self, contact: Option<(u16, u16)>) {
        self.bytes[1536..1564].fill(0);
        if let Some((x, y)) = contact {
            self.bytes[1536] = 1;
            self.bytes[1544] = 1;
            self.bytes[1546..1548].copy_from_slice(&x.to_le_bytes());
            self.bytes[1548..1550].copy_from_slice(&y.to_le_bytes());
        }
    }
}
#[test]
fn touch_waits_for_readiness_only_before_the_finger_goes_down() {
    futures_lite::future::block_on(async {
        let mut remote = Connection::connect(Synthetic::new(), touch_facts()).unwrap();
        let screen = control("touchscreen");
        let touch = |state| Input {
            control: screen.clone(),
            state,
        };
        let snapshot = remote.read(&screen.clone().into()).await.unwrap();
        assert!(matches!(
            snapshot.value,
            Value::Control(ControlSnapshot {
                supported: true,
                ..
            })
        ));
        for state in [InputState::TouchMove { x: 1, y: 1 }, InputState::TouchEnd] {
            assert!(
                remote
                    .set_input(&touch(state), 10)
                    .await
                    .unwrap_err()
                    .contains("No finger is down")
            );
        }
        // Category-one bit 9 withholds input.
        remote.backend.word(800, 1 << 9);
        let start = touch(InputState::TouchStart { x: 100, y: 200 });
        assert!(
            remote
                .set_input(&start, 3)
                .await
                .unwrap_err()
                .contains("input readiness")
        );
        assert!(remote.backend.edges.is_empty());
        remote.backend.word(800, 0);
        remote.backend.decoded_touch(Some((100, 200)));
        remote.set_input(&start, 10).await.unwrap();
        assert!(
            remote
                .set_input(&start, 10)
                .await
                .unwrap_err()
                .contains("already down")
        );
        // Moving and lifting a finger already down does not wait for readiness.
        remote.backend.word(800, 1 << 9);
        remote.backend.decoded_touch(Some((300, 200)));
        remote
            .set_input(&touch(InputState::TouchMove { x: 300, y: 200 }), 10)
            .await
            .unwrap();
        remote.backend.decoded_touch(None);
        remote
            .set_input(&touch(InputState::TouchEnd), 10)
            .await
            .unwrap();
        assert_eq!(remote.backend.edges.len(), 3);
        assert!(
            remote
                .set_input(&touch(InputState::TouchStart { x: 720, y: 0 }), 10)
                .await
                .unwrap_err()
                .contains("outside")
        );
    });
}
#[test]
fn touch_is_accepted_when_the_decoded_report_shows_it() {
    futures_lite::future::block_on(async {
        let mut remote = Connection::connect(Synthetic::new(), touch_facts()).unwrap();
        let screen = control("touchscreen");
        // The report still shows an earlier position, so acceptance waits on it.
        remote.backend.decoded_touch(Some((5, 6)));
        let error = remote
            .set_input(
                &Input {
                    control: screen.clone(),
                    state: InputState::TouchStart { x: 7, y: 6 },
                },
                3,
            )
            .await
            .unwrap_err();
        assert!(error.contains("input acceptance"));
        assert_eq!(remote.backend.waits.last().unwrap().stores, [(1536, 1562)]);
        // The finger stays down after the timeout; its release awaits an empty report.
        let end = Input {
            control: screen,
            state: InputState::TouchEnd,
        };
        remote.backend.decoded_touch(None);
        remote.set_input(&end, 10).await.unwrap();
        let layout = touch_facts();
        let layout = &layout
            .physical_controls
            .touch(&layout.control_ids, "touchscreen")
            .unwrap()
            .decoded;
        let mut report = [0u8; 26];
        report[0] = 2;
        report[8] = 1;
        report[10] = 7;
        report[12] = 6;
        report[20] = 1;
        let start = InputState::TouchStart { x: 7, y: 6 };
        assert!(!memory::touch_decoded(layout, &report, &start).unwrap());
        report[0] = 1;
        report[20] = 0;
        assert!(memory::touch_decoded(layout, &report, &start).unwrap());
        assert!(!memory::touch_decoded(layout, &report, &InputState::TouchEnd).unwrap());
        assert!(memory::touch_decoded(layout, &report[..12], &start).is_err());
    });
}

#[test]
fn parameter_independent_control_requires_every_declared_state_and_keeps_other_gates() {
    futures_lite::future::block_on(async {
        let mut f = facts();
        f.parameter_independent_inputs = serde_json::from_value(json!([{
            "control":"wifi_button", "conditions":[
                {"address":768,"width":4,"values":[3]},
                {"address":776,"width":4,"values":[0]},
                {"address":840,"width":2,"values":[184,185,186]}]
        }]))
        .unwrap();
        let mut r = Connection::connect(Synthetic::new(), f.clone()).unwrap();
        r.backend.word(768, 3);
        r.backend.word(776, 0);
        r.backend.word(840, 184);
        r.backend.word(800, 512);
        r.backend.word(808, 16);
        r.backend.word(816, 1);
        assert!(!r.readiness(None).await.unwrap().ready);
        assert!(
            !r.readiness(Some(&control("menu_ok_button")))
                .await
                .unwrap()
                .ready
        );
        let ready = r.readiness(Some(&control("wifi_button"))).await.unwrap();
        assert!(ready.ready);
        assert!(
            ready
                .reads
                .stores
                .iter()
                .any(|&(a, b)| a <= 840 && b >= 842)
        );
        for (address, wrong) in [(768, 5), (776, 1), (840, 182)] {
            let old = r.backend.bytes[address..address + 4].to_vec();
            r.backend.word(address, wrong);
            assert!(
                !r.readiness(Some(&control("wifi_button")))
                    .await
                    .unwrap()
                    .ready
            );
            r.backend.bytes[address..address + 4].copy_from_slice(&old);
        }
        for address in [780, 820] {
            r.backend.word(address, 1);
            assert!(
                !r.readiness(Some(&control("wifi_button")))
                    .await
                    .unwrap()
                    .ready
            );
            r.backend.word(address, 0);
        }
        for (button, page) in [("menu_ok_button", 184), ("wifi_button", 182)] {
            r.backend.word(840, page);
            let count = r.backend.edges.len();
            let deadline = r.backend.now;
            assert!(
                r.set_input(
                    &Input {
                        control: control(button),
                        state: InputState::Pressed
                    },
                    deadline
                )
                .await
                .unwrap_err()
                .contains("input readiness")
            );
            assert_eq!(r.backend.edges.len(), count);
        }
        r.backend.word(840, 184);
        // The set_input path uses the same qualified readiness, rather than
        // reapplying the generic REC gate after an explicit wait.
        r.set_input(
            &Input {
                control: control("wifi_button"),
                state: InputState::Pressed,
            },
            100,
        )
        .await
        .unwrap();
        r.set_input(
            &Input {
                control: control("wifi_button"),
                state: InputState::Released,
            },
            100,
        )
        .await
        .unwrap();
        f.parameter_independent_inputs[0].conditions.clear();
        assert!(
            Connection::connect(Synthetic::new(), f)
                .err()
                .unwrap()
                .contains("conditions")
        );
    });
}
