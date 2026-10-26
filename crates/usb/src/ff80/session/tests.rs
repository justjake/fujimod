use super::*;
use crate::{
    error::TransferStage,
    ff80::commands::memory::{Address, DebugMemoryLimits, MemoryRange},
};
use futures_lite::future::{block_on, poll_once};
use std::{cell::RefCell, collections::VecDeque, rc::Rc};

enum Step {
    Control([u8; 12]),
    Write(Vec<u8>),
    Read(usize, Result<Vec<u8>>),
    PendingRead,
}
struct MockIo(Rc<RefCell<VecDeque<Step>>>);
impl Io for MockIo {
    async fn control(&mut self, _: RequestKind, bytes: [u8; 12]) -> Result<()> {
        match self.0.borrow_mut().pop_front().expect("unexpected control") {
            Step::Control(expected) => assert_eq!(bytes, expected),
            _ => panic!("expected a different exchange"),
        }
        Ok(())
    }
    async fn write(&mut self, bytes: &[u8]) -> Result<()> {
        match self.0.borrow_mut().pop_front().expect("unexpected write") {
            Step::Write(expected) => assert_eq!(bytes, expected),
            _ => panic!("expected a different exchange"),
        }
        Ok(())
    }
    async fn read(&mut self, maximum: usize) -> Result<Vec<u8>> {
        let step = self.0.borrow_mut().pop_front().expect("unexpected read");
        match step {
            Step::Read(expected, result) => {
                assert_eq!(maximum, expected);
                result
            }
            Step::PendingRead => std::future::pending().await,
            _ => panic!("expected a different exchange"),
        }
    }
}
fn nz(n: u32) -> NonZeroU32 {
    NonZeroU32::new(n).unwrap()
}
fn read_request() -> ReadMemory {
    ReadMemory::new(
        MemoryRange::new(Address(0x12345678), nz(4)).unwrap(),
        DebugMemoryLimits {
            read: nz(256),
            write: nz(256),
        },
    )
    .unwrap()
}
fn opened(extra: Vec<Step>) -> (Core<MockIo>, Rc<RefCell<VecDeque<Step>>>) {
    let mut steps = VecDeque::from([
        Step::Control([0xc0, 0, 0, 0, 0, 0, 0, 0, 0, 8, 0, 0]),
        Step::Read(8, Ok(vec![0x34, 0x12, 0x56, 0x78, 0, 0, 1, 0])),
    ]);
    steps.extend(extra);
    let shared = Rc::new(RefCell::new(steps));
    let core = block_on(Core::open(MockIo(shared.clone()), 1024)).unwrap();
    (core, shared)
}
fn command_steps(bytes: Vec<u8>, maximum: u32, result: Result<Vec<u8>>) -> Vec<Step> {
    vec![
        Step::Control(envelope(
            RequestKind::WriteCommand,
            bytes.len() as u32,
            0x1234,
        )),
        Step::Write(bytes),
        Step::Control(envelope(RequestKind::ReadCommand, maximum, 0x1234)),
        Step::Read(maximum as usize, result),
    ]
}

#[test]
fn session_id_decodes_little_endian_but_requests_encode_big_endian() {
    let (mut core, steps) = opened(vec![
        Step::Control([0xc2, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0x12, 0x34]),
        Step::Read(2, Ok(vec![0x34, 0x12])),
        Step::Control([0xc4, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0x12, 0x34]),
        Step::Read(2, Ok(vec![0, 0])),
    ]);
    assert_eq!(core.info.id, 0x1234);
    assert_eq!(core.info.advertised_packet_bytes.get(), 256);
    assert_eq!(core.info.raw_open_reply[2..4], [0x56, 0x78]);
    assert_eq!(block_on(core.ping()).unwrap(), [0x34, 0x12]);
    block_on(core.close()).unwrap();
    assert!(steps.borrow().is_empty());
}

#[test]
fn direct_read_performs_one_command_and_returns_only_data() {
    let request = read_request();
    let mut reply = request.encode();
    reply.extend_from_slice(&[1, 2, 3, 4]);
    let (mut core, steps) = opened(command_steps(request.encode(), 40, Ok(reply)));
    assert_eq!(block_on(core.read_memory(request)).unwrap(), [1, 2, 3, 4]);
    assert_eq!(core.state, SessionState::Ready);
    assert!(steps.borrow().is_empty());
}

#[test]
fn direct_write_requires_a_matching_acknowledgment() {
    let command = WriteMemory::new(
        Address(4),
        vec![1, 2, 3, 4],
        DebugMemoryLimits {
            read: nz(256),
            write: nz(256),
        },
    )
    .unwrap();
    let reply = command.encode()[..36].to_vec();
    let (mut core, steps) = opened(command_steps(command.encode(), 36, Ok(reply)));
    block_on(core.write_memory(command)).unwrap();
    assert_eq!(core.state, SessionState::Ready);
    assert!(steps.borrow().is_empty());
}

#[test]
fn dropped_future_prevents_reusing_an_outstanding_read() {
    let (mut core, steps) = opened(vec![
        Step::Control(envelope(RequestKind::Ping, 2, 0x1234)),
        Step::PendingRead,
    ]);
    let mut pending = Box::pin(core.ping());
    assert!(block_on(poll_once(pending.as_mut())).is_none());
    drop(pending);
    assert_eq!(core.state, SessionState::CompletionUnknown);
    assert!(matches!(
        block_on(core.ping()),
        Err(Error::CompletionUnknown)
    ));
    assert!(matches!(
        block_on(core.close()),
        Err(Error::CompletionUnknown)
    ));
    assert!(steps.borrow().is_empty());
}

#[test]
fn device_error_is_completed_not_an_instruction_to_wait() {
    let request = read_request();
    let mut reply = request.encode()[..20].to_vec();
    reply[4..8].copy_from_slice(&1u32.to_le_bytes());
    reply[8..12].copy_from_slice(&0xdeadbeefu32.to_le_bytes());
    let mut script = command_steps(request.encode(), 40, Ok(reply));
    script.extend([
        Step::Control(envelope(RequestKind::Ping, 2, 0x1234)),
        Step::Read(2, Ok(vec![0, 0])),
    ]);
    let (mut core, steps) = opened(script);
    assert!(matches!(
        block_on(core.read_memory(request)),
        Err(Error::DeviceStatus {
            primary: 1,
            secondary: 0xdeadbeef,
            ..
        })
    ));
    assert_eq!(core.state, SessionState::Ready);
    block_on(core.ping()).unwrap();
    assert!(steps.borrow().is_empty());
}

#[test]
fn malformed_reply_poisons_session_and_is_not_retried() {
    let request = read_request();
    let mut reply = request.encode();
    reply.extend_from_slice(&[1, 2, 3, 4]);
    reply[28] ^= 1;
    let (mut core, steps) = opened(command_steps(request.encode(), 40, Ok(reply)));
    assert!(matches!(
        block_on(core.read_memory(request)),
        Err(Error::Protocol(ProtocolError::ParameterEcho))
    ));
    assert!(matches!(
        block_on(core.read_memory(request)),
        Err(Error::CompletionUnknown)
    ));
    assert!(steps.borrow().is_empty());
}

#[test]
fn short_reply_and_disconnect_preserve_uncertain_state() {
    for result in [
        Ok(vec![0; 3]),
        Err(Error::Transfer {
            stage: TransferStage::BulkIn,
            source: nusb::transfer::TransferError::Disconnected,
            transferred: Some(2),
            partial: vec![4, 5],
        }),
    ] {
        let request = read_request();
        let (mut core, steps) = opened(command_steps(request.encode(), 40, result));
        let error = block_on(core.read_memory(request)).unwrap_err();
        if let Error::Transfer {
            partial,
            transferred,
            ..
        } = error
        {
            assert_eq!(partial, [4, 5]);
            assert_eq!(transferred, Some(2));
        }
        assert_eq!(core.state, SessionState::CompletionUnknown);
        assert!(matches!(
            block_on(core.ping()),
            Err(Error::CompletionUnknown)
        ));
        assert!(steps.borrow().is_empty());
    }
}

#[test]
fn negotiated_limit_rejects_before_any_io() {
    let (mut core, steps) = opened(vec![]);
    core.info.advertised_packet_bytes = nz(2);
    assert!(matches!(
        block_on(core.read_memory(read_request())),
        Err(Error::LengthLimit {
            requested: 4,
            limit: 2
        })
    ));
    assert_eq!(core.state, SessionState::Ready);
    assert!(steps.borrow().is_empty());
}

#[test]
fn host_reply_budget_rejects_before_any_io() {
    let (mut core, steps) = opened(vec![]);
    core.max_reply_bytes = 39;
    assert!(matches!(
        block_on(core.read_memory(read_request())),
        Err(Error::LengthLimit {
            requested: 40,
            limit: 39
        })
    ));
    assert_eq!(core.state, SessionState::Ready);
    assert!(steps.borrow().is_empty());
}

#[test]
fn zero_advertised_limit_is_rejected() {
    let script = Rc::new(RefCell::new(VecDeque::from([
        Step::Control(envelope(RequestKind::Open, 8, 0)),
        Step::Read(8, Ok(vec![0; 8])),
    ])));
    assert!(matches!(
        block_on(Core::open(MockIo(script), 1024)),
        Err(Error::Protocol(ProtocolError::ZeroPacketLimit))
    ));
}

#[test]
fn device_error_with_wrong_function_does_not_unlock_session() {
    let request = read_request();
    let mut reply = request.encode()[..20].to_vec();
    reply[4..8].copy_from_slice(&1u32.to_le_bytes());
    reply[16] = 3;
    let (mut core, steps) = opened(command_steps(request.encode(), 40, Ok(reply)));
    assert!(matches!(
        block_on(core.read_memory(request)),
        Err(Error::Protocol(ProtocolError::FunctionEcho { .. }))
    ));
    assert_eq!(core.state, SessionState::CompletionUnknown);
    assert!(steps.borrow().is_empty());
}

#[test]
fn short_lifecycle_reply_and_timeout_block_following_commands() {
    for result in [Ok(vec![0]), Err(Error::Timeout(TransferStage::BulkIn))] {
        let (mut core, steps) = opened(vec![
            Step::Control(envelope(RequestKind::Ping, 2, 0x1234)),
            Step::Read(2, result),
        ]);
        assert!(block_on(core.ping()).is_err());
        assert_eq!(core.state, SessionState::CompletionUnknown);
        assert!(matches!(
            block_on(core.ping()),
            Err(Error::CompletionUnknown)
        ));
        assert!(steps.borrow().is_empty());
    }
}

#[test]
fn data_read_uses_exact_offset_and_collects_short_usb_transfers() {
    let (mut core, steps) = opened(vec![
        Step::Control([0xe1, 0, 0x89, 0xab, 0xcd, 0xef, 0, 0, 0, 5, 0x12, 0x34]),
        Step::Read(5, Ok(vec![1, 2])),
        Step::Read(3, Ok(vec![3, 4, 5])),
    ]);
    assert_eq!(
        block_on(core.read_data(0x89abcdef, nz(5))).unwrap(),
        [1, 2, 3, 4, 5]
    );
    assert_eq!(core.state, SessionState::Ready);
    assert!(steps.borrow().is_empty());
}
#[test]
fn failed_data_read_keeps_completed_bytes_and_blocks_reuse() {
    let (mut core, steps) = opened(vec![
        Step::Control(super::super::framing::data_envelope(
            RequestKind::ReadData,
            0,
            5,
            0x1234,
        )),
        Step::Read(5, Ok(vec![1, 2])),
        Step::Read(3, Ok(vec![])),
    ]);
    assert!(
        matches!(block_on(core.read_data(0,nz(5))),Err(Error::PartialRead { data_len:2, data, .. }) if data == [1,2])
    );
    assert_eq!(core.state, SessionState::CompletionUnknown);
    assert!(steps.borrow().is_empty());
}
#[test]
fn data_upload_does_not_implicitly_prepare_or_finalize() {
    let (mut core, steps) = opened(vec![
        Step::Control(envelope(RequestKind::WriteData, 3, 0x1234)),
        Step::Write(vec![1, 2, 3]),
        Step::Control(envelope(RequestKind::EmptyResponse, 32, 0x1234)),
        Step::Read(32, Ok(vec![42, 0, 0, 0])),
    ]);
    block_on(core.write_data(&[1, 2, 3])).unwrap();
    assert_eq!(block_on(core.empty_response()).unwrap(), [42, 0, 0, 0]);
    assert!(steps.borrow().is_empty());
}

#[test]
fn geometry_error_must_belong_to_requested_function_and_have_valid_header() {
    use crate::ff80::commands::configuration::ConfigurationView;
    for (reserved, function, ready) in [(0u32, 1u32, true), (1, 1, false), (0, 2, false)] {
        let request = ConfigurationView::Ram.encode_geometry();
        let mut reply = request[..20].to_vec();
        reply[4..8].copy_from_slice(&1u32.to_le_bytes());
        reply[8..12].copy_from_slice(&0xffffu32.to_le_bytes());
        reply[12..16].copy_from_slice(&reserved.to_le_bytes());
        reply[16..20].copy_from_slice(&function.to_le_bytes());
        let (mut core, steps) = opened(command_steps(request, 48, Ok(reply)));
        let result = block_on(core.configuration_geometry(ConfigurationView::Ram));
        if ready {
            assert!(matches!(
                result,
                Err(Error::DeviceStatus {
                    primary: 1,
                    secondary: 0xffff,
                    ..
                })
            ));
        } else {
            assert!(matches!(result, Err(Error::Protocol(_))));
        }
        assert_eq!(
            core.state,
            if ready {
                SessionState::Ready
            } else {
                SessionState::CompletionUnknown
            }
        );
        assert!(steps.borrow().is_empty());
    }
}

#[test]
fn capabilities_selects_data_without_a_hidden_download() {
    let mut request = vec![0; 36];
    request[..4].copy_from_slice(&0x580000u32.to_le_bytes());
    request[20..24].copy_from_slice(&0x100u32.to_le_bytes());
    let mut reply = vec![0; 32];
    reply[..4].copy_from_slice(&0x580000u32.to_le_bytes());
    reply[20..24].copy_from_slice(&128u32.to_le_bytes());
    let (mut core, steps) = opened(command_steps(request, 32, Ok(reply)));
    let metadata = block_on(core.prepare_capabilities()).unwrap();
    assert_eq!(metadata.byte_length, 128);
    assert_eq!(core.state, SessionState::Ready);
    assert!(steps.borrow().is_empty());
}

#[test]
fn capabilities_error_and_bad_function_have_different_session_outcomes() {
    use crate::ff80::commands::capabilities;
    for function in [0u32, 1] {
        let mut reply = vec![0; 32];
        reply[..4].copy_from_slice(&0x580000u32.to_le_bytes());
        reply[4..8].copy_from_slice(&1u32.to_le_bytes());
        reply[8..12].copy_from_slice(&0xffffu32.to_le_bytes());
        reply[16..20].copy_from_slice(&function.to_le_bytes());
        let (mut core, steps) = opened(command_steps(capabilities::request(), 32, Ok(reply)));
        let result = block_on(core.prepare_capabilities());
        if function == 0 {
            assert!(matches!(result, Err(Error::DeviceStatus { .. })));
            assert_eq!(core.state, SessionState::Ready);
        } else {
            assert!(matches!(result, Err(Error::Protocol(_))));
            assert_eq!(core.state, SessionState::CompletionUnknown);
            assert!(matches!(
                block_on(core.close()),
                Err(Error::CompletionUnknown)
            ));
        }
        assert!(steps.borrow().is_empty());
    }
}

#[test]
fn capabilities_checks_metadata_budget_before_sending() {
    let (mut core, steps) = opened(vec![]);
    core.max_reply_bytes = 31;
    assert!(matches!(
        block_on(core.prepare_capabilities()),
        Err(Error::LengthLimit {
            requested: 32,
            limit: 31
        })
    ));
    assert_eq!(core.state, SessionState::Ready);
    assert!(steps.borrow().is_empty());
}

#[test]
fn download_state_is_read_once_and_only_explicit_clear_mutates_status() {
    use crate::ff80::commands::info::{DownloadStatusAction, FirmwareDownloadState};
    fn reply(text: &str) -> Vec<u8> {
        let mut bytes = vec![0; 16];
        bytes[..4].copy_from_slice(&0x150000u32.to_le_bytes());
        for word in text.encode_utf16().chain([0]) {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        bytes
    }
    let mut script = command_steps(
        DownloadStatusAction::Read.encode(),
        128,
        Ok(reply("WRITING")),
    );
    script.extend(command_steps(
        DownloadStatusAction::Clear.encode(),
        128,
        Ok(reply("NORMAL")),
    ));
    let (mut core, steps) = opened(script);
    assert_eq!(
        block_on(core.download_status(DownloadStatusAction::Read))
            .unwrap()
            .state,
        FirmwareDownloadState::Writing
    );
    assert_eq!(core.state, SessionState::Ready);
    assert_eq!(steps.borrow().len(), 4);
    assert_eq!(
        block_on(core.download_status(DownloadStatusAction::Clear))
            .unwrap()
            .state,
        FirmwareDownloadState::Normal
    );
    assert_eq!(core.state, SessionState::Ready);
    assert!(steps.borrow().is_empty());
}

#[test]
fn research_statuses_do_not_drive_wait_retry_or_cleanup() {
    use crate::ff80::commands::{
        diagnostic::CommandStatus,
        research::{CommandCode, Request},
    };
    let request = Request::new(CommandCode(0x1a0001), vec![0; 16], nz(32)).unwrap();
    let mut reply = request.encode();
    reply[4..8].copy_from_slice(&1u32.to_le_bytes());
    reply[8..12].copy_from_slice(&0xffffu32.to_le_bytes());
    let (mut core, steps) = opened(command_steps(request.encode(), 32, Ok(reply)));
    assert_eq!(
        block_on(core.research_command(&request)).unwrap().status,
        CommandStatus::DeviceError {
            primary: 1,
            secondary: 0xffff
        }
    );
    assert_eq!(core.state, SessionState::Ready);
    assert!(steps.borrow().is_empty());
}

#[test]
fn canceled_research_exchange_owns_the_unfinished_reply() {
    use crate::ff80::commands::research::{CommandCode, Request};
    let request = Request::new(CommandCode(0x1a0001), vec![0; 16], nz(32)).unwrap();
    let mut script = command_steps(request.encode(), 32, Ok(vec![]));
    *script.last_mut().unwrap() = Step::PendingRead;
    let (mut core, steps) = opened(script);
    assert!(block_on(poll_once(core.research_command(&request))).is_none());
    assert_eq!(core.state, SessionState::CompletionUnknown);
    assert!(matches!(
        block_on(core.ping()),
        Err(Error::CompletionUnknown)
    ));
    assert!(steps.borrow().is_empty());
}

#[test]
fn research_budget_is_checked_before_sending_and_bad_echo_fences() {
    use crate::ff80::commands::research::{CommandCode, Request};
    let request = Request::new(CommandCode(0x1a0001), vec![0; 16], nz(32)).unwrap();
    let (mut core, steps) = opened(vec![]);
    core.max_reply_bytes = 31;
    assert!(matches!(
        block_on(core.research_command(&request)),
        Err(Error::LengthLimit {
            requested: 32,
            limit: 31
        })
    ));
    assert_eq!(core.state, SessionState::Ready);
    assert!(steps.borrow().is_empty());
    let mut reply = request.encode();
    reply[0] ^= 1;
    let (mut core, steps) = opened(command_steps(request.encode(), 32, Ok(reply)));
    assert!(matches!(
        block_on(core.research_command(&request)),
        Err(Error::Protocol(_))
    ));
    assert_eq!(core.state, SessionState::CompletionUnknown);
    assert!(steps.borrow().is_empty());
}
