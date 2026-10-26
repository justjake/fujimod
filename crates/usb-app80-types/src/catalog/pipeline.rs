//! Direct firmware pipeline and mode requests. Each action encodes one App80
//! service command; waiting and request sequences are performed by firmware.
use super::{ArgumentKind, CommandError, Toggle};
use fujimod_catalog_service::Codec;

/// A nonzero byte consumed by `adjustment_request_value`.
///
/// The handler reports failure for zero and passes every other byte to its
/// firmware callback unchanged.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PipelineValue(u8);
impl PipelineValue {
    pub fn new(value: u8) -> Result<Self, CommandError> {
        if value == 0 {
            Err(CommandError::Argument(ArgumentKind::PipelineValue))
        } else {
            Ok(Self(value))
        }
    }

    pub fn value(self) -> u8 {
        self.0
    }
}

/// The selector accepted by the pipeline setup handler. Values outside 0–6
/// all take the same default branch, represented by `Default`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PipelinePreparationMode {
    Mode0,
    Mode1,
    Mode2,
    Mode3,
    Mode4,
    Mode5,
    Mode6,
    Default,
}
impl PipelinePreparationMode {
    pub(super) fn wire(self) -> u8 {
        match self {
            Self::Mode0 => 0,
            Self::Mode1 => 1,
            Self::Mode2 => 2,
            Self::Mode3 => 3,
            Self::Mode4 => 4,
            Self::Mode5 => 5,
            Self::Mode6 => 6,
            Self::Default => 7,
        }
    }
}

/// The request-mode selector indexes the firmware's seven-entry mode table.
/// The selector is not the mode byte sent by the lower-level routine.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PipelineRequestMode {
    Mode0,
    Mode1,
    Mode2,
    Mode3,
    Mode4,
    Mode5,
    Mode6,
}
impl PipelineRequestMode {
    pub(super) fn wire(self) -> u8 {
        match self {
            Self::Mode0 => 0,
            Self::Mode1 => 1,
            Self::Mode2 => 2,
            Self::Mode3 => 3,
            Self::Mode4 => 4,
            Self::Mode5 => 5,
            Self::Mode6 => 6,
        }
    }
}

/// The command selector consumed by the firmware's adjustment dispatcher.
/// Selectors 2–25 are preserved by their table index because several cases
/// have no established user-facing name. Any value outside that table follows
/// the same default branch as `DefaultPath`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PipelineDispatchSelector {
    PreviousItem,
    DefaultPath,
    Selector2,
    Selector3,
    Selector4,
    Selector5,
    Selector6,
    Selector7,
    Selector8,
    Selector9,
    Selector10,
    Selector11,
    Selector12,
    Selector13,
    Selector14,
    Selector15,
    Selector16,
    Selector17,
    Selector18,
    Selector19,
    Selector20,
    Selector21,
    Selector22,
    Selector23,
    Selector24,
    Selector25,
}
impl PipelineDispatchSelector {
    pub(super) fn wire(self) -> u8 {
        match self {
            Self::PreviousItem => 0,
            Self::DefaultPath => 1,
            Self::Selector2 => 2,
            Self::Selector3 => 3,
            Self::Selector4 => 4,
            Self::Selector5 => 5,
            Self::Selector6 => 6,
            Self::Selector7 => 7,
            Self::Selector8 => 8,
            Self::Selector9 => 9,
            Self::Selector10 => 10,
            Self::Selector11 => 11,
            Self::Selector12 => 12,
            Self::Selector13 => 13,
            Self::Selector14 => 14,
            Self::Selector15 => 15,
            Self::Selector16 => 16,
            Self::Selector17 => 17,
            Self::Selector18 => 18,
            Self::Selector19 => 19,
            Self::Selector20 => 20,
            Self::Selector21 => 21,
            Self::Selector22 => 22,
            Self::Selector23 => 23,
            Self::Selector24 => 24,
            Self::Selector25 => 25,
        }
    }
}

/// Option values inspected by `adjustment_dispatch_command`. All values above
/// 12 take the branch represented by `Default` in every selector case.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PipelineDispatchOption {
    Option0,
    Option1,
    Option2,
    Option3,
    Option4,
    Option5,
    Option6,
    Option7,
    Option8,
    Option9,
    Option10,
    Option11,
    Option12,
    Default,
}
impl PipelineDispatchOption {
    pub(super) fn wire(self) -> u8 {
        match self {
            Self::Option0 => 0,
            Self::Option1 => 1,
            Self::Option2 => 2,
            Self::Option3 => 3,
            Self::Option4 => 4,
            Self::Option5 => 5,
            Self::Option6 => 6,
            Self::Option7 => 7,
            Self::Option8 => 8,
            Self::Option9 => 9,
            Self::Option10 => 10,
            Self::Option11 => 11,
            Self::Option12 => 12,
            Self::Default => u8::MAX,
        }
    }
}

/// One direct pipeline or mode-related firmware service command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PipelineAction {
    InitializePipeline {
        enabled: Toggle,
    },
    StartOutput,
    RunModeConfigurationSequence,
    RunRequestAndWait,
    RequestValue {
        value: PipelineValue,
    },
    RunFrameConfigurationSequence,
    RequestMessageSequence,
    PrepareMode {
        mode: PipelinePreparationMode,
    },
    RequestReset,
    DispatchCommand {
        selector: PipelineDispatchSelector,
        option: PipelineDispatchOption,
    },
    RequestMode {
        mode: PipelineRequestMode,
    },
}
impl PipelineAction {
    pub(super) fn codec(self) -> Codec {
        match self {
            Self::InitializePipeline { .. } => Codec::PipelineInitialize,
            Self::StartOutput => Codec::PipelineStartOutput,
            Self::RunModeConfigurationSequence => Codec::PipelineModeConfigurationSequence,
            Self::RunRequestAndWait => Codec::PipelineRequestWait,
            Self::RequestValue { .. } => Codec::PipelineRequestValue,
            Self::RunFrameConfigurationSequence => Codec::PipelineFrameConfigurationSequence,
            Self::RequestMessageSequence => Codec::PipelineMessageSequence,
            Self::PrepareMode { .. } => Codec::PipelinePrepareMode,
            Self::RequestReset => Codec::PipelineRequestReset,
            Self::DispatchCommand { .. } => Codec::PipelineDispatchCommand,
            Self::RequestMode { .. } => Codec::PipelineRequestMode,
        }
    }

    pub(super) fn encode(self, put: &mut impl FnMut(usize, u8)) {
        match self {
            Self::InitializePipeline { enabled } => put(0, enabled.wire()),
            Self::RequestValue { value } => put(0, value.value()),
            Self::PrepareMode { mode } => put(0, mode.wire()),
            Self::DispatchCommand { selector, option } => {
                put(0, selector.wire());
                put(1, option.wire());
            }
            Self::RequestMode { mode } => put(0, mode.wire()),
            Self::StartOutput
            | Self::RunModeConfigurationSequence
            | Self::RunRequestAndWait
            | Self::RunFrameConfigurationSequence
            | Self::RequestMessageSequence
            | Self::RequestReset => (),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{
        Catalog, Command, CommandCode, CommandError, PreparedCommand, Reply, ReplyValue,
    };
    use super::*;
    use fujimod_catalog_firmware_identity::FirmwareIdentity;
    use fujimod_catalog_service::{Definition, ServiceModes};

    fn catalog(reverse: bool) -> Catalog {
        let layouts = [
            ("pipeline_initialize", 0xa0, 1, 5, 8),
            ("pipeline_start_output", 0xa1, 0, 24, 1),
            ("pipeline_mode_configuration_sequence", 0xa2, 0, 24, 0),
            ("pipeline_request_wait", 0xa4, 0, 24, 0),
            ("pipeline_request_wait", 0xa5, 0, 16, 0),
            ("pipeline_request_value", 0xa6, 1, 16, 0),
            ("pipeline_request_wait", 0xa7, 0, 16, 0),
            ("pipeline_frame_configuration_sequence", 0xa8, 0, 24, 0),
            ("pipeline_request_wait", 0xa9, 0, 16, 0),
            ("pipeline_message_sequence", 0xac, 0, 24, 0),
            ("pipeline_prepare_mode", 0xb0, 1, 1, 2),
            ("pipeline_request_reset", 0xb1, 0, 2, 1),
            ("pipeline_dispatch_command", 0xb2, 2, 2, 0),
            ("pipeline_request_mode", 0xb3, 1, 2, 0),
        ];
        let entries: Vec<_> = layouts
            .into_iter()
            .enumerate()
            .map(|(index, (codec, code, count, enabled_modes, next_mode))| {
                let slots: Vec<_> = (0..count)
                    .map(|slot| if reverse { 31 - slot } else { slot })
                    .collect();
                serde_json::json!({
                    "code": code,
                    "codec": codec,
                    "label": codec,
                    "slots": slots,
                    "enabled_modes": enabled_modes,
                    "next_mode": next_mode,
                    "firmware_kind": 1,
                    "handler": index,
                    "parameter_table": index
                })
            })
            .collect();
        let doc = serde_json::json!({
            "schema": 1,
            "firmware": {
                "id": "pipeline-test",
                "model": "Synthetic",
                "version": "2",
                "sha256": "ab".repeat(32)
            },
            "mailbox": {"page": 3, "offset": 576, "configuration_limit": 512},
            "entries": entries
        });
        Catalog::new(
            Definition::from_json(
                &doc.to_string(),
                &FirmwareIdentity::new("pipeline-test", &"ab".repeat(32)).unwrap(),
            )
            .unwrap(),
        )
    }

    fn prepared(catalog: &Catalog, modes: ServiceModes, action: PipelineAction) -> PreparedCommand {
        catalog
            .prepare(modes, Command::Pipeline { action })
            .unwrap()
    }

    fn logical(prepared: &PreparedCommand) -> Vec<u8> {
        prepared
            .slots
            .iter()
            .map(|&slot| prepared.request.arguments()[usize::from(slot)])
            .collect()
    }

    fn decoded(prepared: &PreparedCommand) -> ReplyValue {
        prepared
            .decode(Reply::decode(&prepared.request.encode()).unwrap())
            .unwrap()
            .value()
            .clone()
    }

    #[test]
    fn direct_pipeline_requests_use_only_the_firmware_parameter_slots() {
        for reverse in [false, true] {
            let catalog = catalog(reverse);
            let cases = [
                (
                    PipelineAction::InitializePipeline {
                        enabled: Toggle::Disabled,
                    },
                    0xa0,
                    1,
                    vec![0],
                ),
                (
                    PipelineAction::InitializePipeline {
                        enabled: Toggle::Enabled,
                    },
                    0xa0,
                    1,
                    vec![1],
                ),
                (PipelineAction::StartOutput, 0xa1, 8, vec![]),
                (
                    PipelineAction::RunModeConfigurationSequence,
                    0xa2,
                    8,
                    vec![],
                ),
                (PipelineAction::RunRequestAndWait, 0xa4, 8, vec![]),
                (
                    PipelineAction::RequestValue {
                        value: PipelineValue::new(1).unwrap(),
                    },
                    0xa6,
                    16,
                    vec![1],
                ),
                (
                    PipelineAction::RunFrameConfigurationSequence,
                    0xa8,
                    8,
                    vec![],
                ),
                (PipelineAction::RequestMessageSequence, 0xac, 8, vec![]),
                (
                    PipelineAction::PrepareMode {
                        mode: PipelinePreparationMode::Mode6,
                    },
                    0xb0,
                    1,
                    vec![6],
                ),
                (
                    PipelineAction::PrepareMode {
                        mode: PipelinePreparationMode::Default,
                    },
                    0xb0,
                    1,
                    vec![7],
                ),
                (PipelineAction::RequestReset, 0xb1, 2, vec![]),
                (
                    PipelineAction::DispatchCommand {
                        selector: PipelineDispatchSelector::Selector12,
                        option: PipelineDispatchOption::Option4,
                    },
                    0xb2,
                    2,
                    vec![12, 4],
                ),
                (
                    PipelineAction::DispatchCommand {
                        selector: PipelineDispatchSelector::DefaultPath,
                        option: PipelineDispatchOption::Default,
                    },
                    0xb2,
                    2,
                    vec![1, 255],
                ),
                (
                    PipelineAction::RequestMode {
                        mode: PipelineRequestMode::Mode6,
                    },
                    0xb3,
                    2,
                    vec![6],
                ),
            ];
            for (action, code, mode, expected) in cases {
                let prepared = prepared(&catalog, ServiceModes::new(mode).unwrap(), action);
                assert_eq!(prepared.request.code(), CommandCode(code));
                assert_eq!(logical(&prepared), expected);
                assert_eq!(decoded(&prepared), ReplyValue::NoPayload);
            }
        }
    }

    #[test]
    fn duplicate_request_wait_rows_need_explicit_code_selection() {
        let catalog = catalog(false);
        let modes = ServiceModes::new(16).unwrap();
        let action = PipelineAction::RunRequestAndWait;
        assert!(matches!(
            catalog.prepare(modes, Command::Pipeline { action }),
            Err(CommandError::Ambiguous(Codec::PipelineRequestWait))
        ));
        for code in [0xa5, 0xa7, 0xa9] {
            let prepared = catalog
                .prepare_code(CommandCode(code), modes, Command::Pipeline { action })
                .unwrap();
            assert_eq!(prepared.request.code(), CommandCode(code));
            assert!(logical(&prepared).is_empty());
            assert_eq!(decoded(&prepared), ReplyValue::NoPayload);
        }
        assert!(matches!(
            catalog.prepare_code(
                CommandCode(0xa5),
                ServiceModes::new(8).unwrap(),
                Command::Pipeline { action },
            ),
            Err(CommandError::Unavailable(CommandCode(0xa5)))
        ));
    }

    #[test]
    fn pipeline_value_rejects_the_firmware_failure_sentinel() {
        assert!(matches!(
            PipelineValue::new(0),
            Err(CommandError::Argument(ArgumentKind::PipelineValue))
        ));
        for value in [1, 127, 255] {
            assert_eq!(PipelineValue::new(value).unwrap().value(), value);
        }
    }

    #[test]
    fn selector_tables_preserve_the_reviewed_firmware_ranges() {
        assert_eq!(PipelinePreparationMode::Default.wire(), 7);
        assert_eq!(PipelineRequestMode::Mode0.wire(), 0);
        assert_eq!(PipelineRequestMode::Mode6.wire(), 6);
        assert_eq!(PipelineDispatchSelector::PreviousItem.wire(), 0);
        assert_eq!(PipelineDispatchSelector::DefaultPath.wire(), 1);
        assert_eq!(PipelineDispatchSelector::Selector25.wire(), 25);
        assert_eq!(PipelineDispatchOption::Option12.wire(), 12);
        assert_eq!(PipelineDispatchOption::Default.wire(), u8::MAX);
    }
}
