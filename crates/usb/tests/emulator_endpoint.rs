#![cfg(not(target_arch = "wasm32"))]
use async_lock::Mutex;
use fujimod_usb::{
    Error,
    connection::{Connection, InterfaceSelection, TransferOptions},
    emulator::{
        Completion, ControlRequest, ControlType, EndpointAddress, EndpointDescriptor,
        EndpointFuture, EndpointKind, EndpointProvider, Endpoints, InterfaceDescriptor, PacketSize,
        Recipient, ResourceOperation, TransferError,
    },
    error::{ProtocolError, TransferStage},
    ff80::{Session, SessionState, commands::info::InfoSelector},
};
use futures_lite::future::{block_on, poll_once};
use std::{num::NonZeroU32, sync::Arc, time::Duration};

const SELECTION: InterfaceSelection = InterfaceSelection {
    configuration: 1,
    interface: 2,
    alternate: 0,
};

#[derive(Clone, Copy, Default)]
enum ReadBehavior {
    #[default]
    Normal,
    Pending,
    Disconnected,
    Cancelled,
    Overreported,
    Short,
    Malformed,
}
#[derive(Clone, Copy, Default)]
enum ResourceBehavior {
    #[default]
    Normal,
    Pending,
    Disconnected,
}
#[derive(Default)]
struct Model {
    claimed: bool,
    session_open: bool,
    reply: Vec<u8>,
    read_behavior: ReadBehavior,
    claim_behavior: ResourceBehavior,
    release_behavior: ResourceBehavior,
    short_control: bool,
    short_write: bool,
    memory: std::collections::BTreeMap<u32, u8>,
    callback_disconnect: bool,
    triggered: bool,
    close_reply: Option<[u8; 2]>,
    close_controls: usize,
    release_attempts: usize,
}
struct Provider {
    descriptors: Vec<InterfaceDescriptor>,
    model: Arc<Mutex<Model>>,
}
impl Provider {
    fn new() -> Self {
        Self {
            descriptors: vec![InterfaceDescriptor {
                selection: SELECTION,
                endpoints: [0x83, 0x04]
                    .into_iter()
                    .map(|address| EndpointDescriptor {
                        address: EndpointAddress::new(address).unwrap(),
                        kind: EndpointKind::Bulk,
                        packet_size: PacketSize::new(64).unwrap(),
                    })
                    .collect(),
            }],
            model: Arc::new(Mutex::new(Model::default())),
        }
    }
}
impl EndpointProvider for Provider {
    fn interfaces(&self) -> &[InterfaceDescriptor] {
        &self.descriptors
    }
    fn claim(
        &mut self,
        selection: InterfaceSelection,
    ) -> EndpointFuture<'_, Result<Box<dyn Endpoints>, TransferError>> {
        let mut model = self.model.try_lock().expect("serialized endpoint state");
        if selection != SELECTION || model.claimed {
            return Box::pin(async { Err(TransferError::Stall) });
        }
        model.claimed = true;
        let behavior = model.claim_behavior;
        let endpoints = ModelEndpoints(self.model.clone());
        Box::pin(async move {
            match behavior {
                ResourceBehavior::Normal => Ok(Box::new(endpoints) as Box<dyn Endpoints>),
                ResourceBehavior::Pending => {
                    let _claim = endpoints;
                    std::future::pending().await
                }
                ResourceBehavior::Disconnected => {
                    drop(endpoints);
                    Err(TransferError::Disconnected)
                }
            }
        })
    }
}
struct ModelEndpoints(Arc<Mutex<Model>>);
impl Drop for ModelEndpoints {
    fn drop(&mut self) {
        self.0
            .try_lock()
            .expect("serialized endpoint state")
            .claimed = false;
    }
}
fn completed(transferred: usize) -> Completion {
    Completion {
        transferred,
        status: Ok(()),
    }
}
impl Endpoints for ModelEndpoints {
    fn control_out<'a>(
        &'a mut self,
        request: ControlRequest,
        data: &'a [u8],
    ) -> EndpointFuture<'a, Completion> {
        Box::pin(async move {
            assert!(matches!(request.control_type, ControlType::Vendor));
            assert!(matches!(request.recipient, Recipient::Other));
            assert_eq!((request.request, request.value, request.index), (0, 0, 0));
            assert_eq!(data.len(), 12);
            let mut model = self.0.try_lock().expect("serialized endpoint state");
            if model.short_control {
                return completed(1);
            }
            match data[0] {
                0x1d => {
                    assert_eq!(data, &[0x1d, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
                    model.session_open = false;
                    model.reply = vec![0x29, 0];
                }
                0xc0 => {
                    model.reply = if model.session_open {
                        vec![0, 0, 0, 0, 0, 0, 1, 0]
                    } else {
                        vec![42, 0, 0, 0, 0, 0, 0, 64]
                    };
                    model.session_open = true;
                }
                0xc2 => {
                    assert!(model.session_open);
                    model.reply = vec![42, 0];
                }
                0xc4 => {
                    assert!(model.session_open);
                    assert_eq!(&data[10..12], &42u16.to_be_bytes());
                    model.close_controls += 1;
                    model.session_open = false;
                    model.reply = model.close_reply.unwrap_or([0, 0]).to_vec();
                }
                0xd0 => model.reply.clear(),
                0xd1 => {}
                _ => {
                    return Completion {
                        transferred: 0,
                        status: Err(TransferError::Stall),
                    };
                }
            }
            completed(data.len())
        })
    }
    fn bulk_out<'a>(
        &'a mut self,
        endpoint: EndpointAddress,
        data: &'a [u8],
    ) -> EndpointFuture<'a, Completion> {
        Box::pin(async move {
            assert_eq!(endpoint.get(), 0x04);
            let mut model = self.0.try_lock().expect("serialized endpoint state");
            if model.short_write {
                return completed(1);
            }
            if data.len() >= 36 && data[..4] == 0x200001u32.to_le_bytes() {
                let function = u32::from_le_bytes(data[16..20].try_into().unwrap());
                let address = u32::from_le_bytes(data[28..32].try_into().unwrap());
                let length = u32::from_le_bytes(data[32..36].try_into().unwrap());
                let mut reply = data[..36].to_vec();
                match function {
                    2 => reply.extend(
                        (0..length).map(|i| *model.memory.get(&(address + i)).unwrap_or(&0)),
                    ),
                    3 => {
                        assert_eq!(data.len(), 36 + length as usize);
                        for (i, byte) in data[36..].iter().enumerate() {
                            model.memory.insert(address + i as u32, *byte);
                        }
                    }
                    _ => panic!("unsupported synthetic memory command"),
                }
                model.reply = reply;
                return completed(data.len());
            }
            if data.len() == 32 && data[..4] == 0x450006u32.to_le_bytes() {
                model.triggered = true;
                if model.callback_disconnect {
                    model.read_behavior = ReadBehavior::Disconnected;
                }
                model.reply = data.to_vec();
                return completed(data.len());
            }
            if data.len() != 32 || data[..4] != 0x150000u32.to_le_bytes() {
                return Completion {
                    transferred: 0,
                    status: Err(TransferError::Stall),
                };
            }
            let mut reply = vec![0; 128];
            reply[..4].copy_from_slice(&0x150000u32.to_le_bytes());
            for (pair, word) in reply[16..]
                .chunks_exact_mut(2)
                .zip("Synthetic endpoint".encode_utf16())
            {
                pair.copy_from_slice(&word.to_le_bytes());
            }
            model.reply = reply;
            completed(data.len())
        })
    }
    fn bulk_in<'a>(
        &'a mut self,
        endpoint: EndpointAddress,
        data: &'a mut [u8],
    ) -> EndpointFuture<'a, Completion> {
        Box::pin(async move {
            assert_eq!(endpoint.get(), 0x83);
            let behavior = self
                .0
                .try_lock()
                .expect("serialized endpoint state")
                .read_behavior;
            if matches!(behavior, ReadBehavior::Pending) {
                return std::future::pending().await;
            }
            let model = self.0.try_lock().expect("serialized endpoint state");
            let length = data.len().min(model.reply.len());
            data[..length].copy_from_slice(&model.reply[..length]);
            match behavior {
                ReadBehavior::Disconnected => Completion {
                    transferred: 1,
                    status: Err(TransferError::Disconnected),
                },
                ReadBehavior::Cancelled => Completion {
                    transferred: 1,
                    status: Err(TransferError::Cancelled),
                },
                ReadBehavior::Overreported => completed(data.len() + 1),
                ReadBehavior::Short => completed(1),
                ReadBehavior::Malformed => {
                    data.fill(0);
                    completed(length)
                }
                _ => completed(length),
            }
        })
    }
    fn release(&mut self) -> EndpointFuture<'_, Result<(), TransferError>> {
        Box::pin(async move {
            let behavior = {
                let mut model = self.0.try_lock().expect("serialized endpoint state");
                model.release_attempts += 1;
                model.release_behavior
            };
            match behavior {
                ResourceBehavior::Normal => {
                    self.0
                        .try_lock()
                        .expect("serialized endpoint state")
                        .claimed = false;
                    Ok(())
                }
                ResourceBehavior::Pending => std::future::pending().await,
                ResourceBehavior::Disconnected => Err(TransferError::Disconnected),
            }
        })
    }
}
fn options(max_reply_bytes: u32) -> TransferOptions {
    TransferOptions {
        timeout: Duration::from_millis(20),
        max_reply_bytes: NonZeroU32::new(max_reply_bytes).unwrap(),
    }
}
fn open(provider: &mut Provider, maximum: u32) -> Session {
    let connection = block_on(Connection::open_emulated(
        provider,
        SELECTION,
        options(maximum),
    ))
    .unwrap();
    block_on(connection.begin_session(None)).unwrap()
}

#[test]
fn external_provider_runs_existing_commands_and_explicit_close() {
    let mut provider = Provider::new();
    let selection = provider.interfaces()[0].selection;
    assert!(!provider.model.try_lock().unwrap().claimed);
    let connection = block_on(Connection::open_emulated(
        &mut provider,
        selection,
        options(128),
    ))
    .unwrap();
    assert!(!provider.model.try_lock().unwrap().session_open);
    assert!(provider.model.try_lock().unwrap().claimed);
    let mut session = block_on(connection.begin_session(None)).unwrap();
    assert_eq!(session.info().id, 42);
    assert_eq!(session.info().advertised_packet_bytes.get(), 64);
    assert_eq!(block_on(session.ping()).unwrap(), [42, 0]);
    assert_eq!(
        block_on(session.device_info(InfoSelector::Product))
            .unwrap()
            .text,
        "Synthetic endpoint"
    );
    assert_eq!(session.state(), SessionState::Ready);
    assert_eq!(block_on(session.close()).unwrap(), [0, 0]);
    let model = provider
        .model
        .try_lock()
        .expect("serialized endpoint state");
    assert!(!model.claimed);
    assert!(!model.session_open);
}

#[test]
fn checked_close_validates_reply_before_explicit_host_release() {
    let mut provider = Provider::new();
    let session = open(&mut provider, 128);
    let expected = session.info().id.to_le_bytes();
    provider.model.try_lock().unwrap().close_reply = Some(expected);
    assert_eq!(block_on(session.close_checked(expected)).unwrap(), expected);
    let model = provider.model.try_lock().unwrap();
    assert_eq!((model.close_controls, model.release_attempts), (1, 1));
    assert!(!model.claimed && !model.session_open);
}

#[test]
fn checked_close_wrong_reply_consumes_session_and_only_drops_host_resources() {
    for reply in [[0, 0], [43, 0], [0, 42]] {
        let mut provider = Provider::new();
        let session = open(&mut provider, 128);
        let expected = session.info().id.to_le_bytes();
        provider.model.try_lock().unwrap().close_reply = Some(reply);
        assert!(matches!(
            block_on(session.close_checked(expected)),
            Err(Error::Protocol(ProtocolError::ExpectationMismatch))
        ));
        let model = provider.model.try_lock().unwrap();
        assert_eq!((model.close_controls, model.release_attempts), (1, 0));
        assert!(!model.claimed);
        // The test peer completed C4 before supplying an invalid reply. The
        // caller must still report failure rather than infer that completion.
        assert!(!model.session_open);
    }
}

#[test]
fn checked_close_mismatch_is_recorded_as_a_protocol_failure() {
    let mut provider = Provider::new();
    let trace = fujimod_usb::trace::Trace::new(
        NonZeroU32::new(64).unwrap(),
        NonZeroU32::new(4096).unwrap(),
    );
    let connection = block_on(Connection::open_emulated(
        &mut provider,
        SELECTION,
        options(128),
    ))
    .unwrap();
    let session = block_on(connection.begin_session(Some(trace.clone()))).unwrap();
    let expected = session.info().id.to_le_bytes();
    assert!(matches!(
        block_on(session.close_checked(expected)),
        Err(Error::Protocol(ProtocolError::ExpectationMismatch))
    ));
    let batch = trace.drain();
    assert!(batch.protocol_events.iter().any(|event| {
        event.command == 0xc4
            && matches!(
                &event.action,
                fujimod_usb::trace::Action::Failed {
                    data,
                    failure: fujimod_usb::trace::Failure::Other(_),
                    ..
                } if data == &[0, 0]
            )
    }));
}

#[test]
fn checked_close_transfer_errors_drop_resources_without_cleanup_or_retry() {
    for behavior in [
        ReadBehavior::Disconnected,
        ReadBehavior::Cancelled,
        ReadBehavior::Short,
        ReadBehavior::Pending,
    ] {
        let mut provider = Provider::new();
        let session = open(&mut provider, 128);
        let expected = session.info().id.to_le_bytes();
        {
            let mut model = provider.model.try_lock().unwrap();
            model.close_reply = Some(expected);
            model.read_behavior = behavior;
        }
        assert!(block_on(session.close_checked(expected)).is_err());
        let model = provider.model.try_lock().unwrap();
        assert_eq!((model.close_controls, model.release_attempts), (1, 0));
        assert!(!model.claimed);
    }
}

#[test]
fn cancelled_checked_close_drops_resources_without_another_camera_command() {
    let mut provider = Provider::new();
    let session = open(&mut provider, 128);
    let expected = session.info().id.to_le_bytes();
    provider.model.try_lock().unwrap().read_behavior = ReadBehavior::Pending;
    let mut close = Box::pin(session.close_checked(expected));
    assert!(block_on(poll_once(close.as_mut())).is_none());
    drop(close);
    let model = provider.model.try_lock().unwrap();
    assert_eq!((model.close_controls, model.release_attempts), (1, 0));
    assert!(!model.claimed);
}

#[test]
fn checked_close_reports_release_failure_without_repeating_verified_close() {
    for behavior in [ResourceBehavior::Pending, ResourceBehavior::Disconnected] {
        let mut provider = Provider::new();
        let session = open(&mut provider, 128);
        let expected = session.info().id.to_le_bytes();
        {
            let mut model = provider.model.try_lock().unwrap();
            model.close_reply = Some(expected);
            model.release_behavior = behavior;
        }
        let error = block_on(session.close_checked(expected)).unwrap_err();
        assert!(matches!(
            error,
            Error::EmulatorResourceTimeout(ResourceOperation::Release)
                | Error::EmulatorResource {
                    operation: ResourceOperation::Release,
                    ..
                }
        ));
        let model = provider.model.try_lock().unwrap();
        assert_eq!((model.close_controls, model.release_attempts), (1, 1));
        assert!(!model.claimed && !model.session_open);
    }
}

#[test]
fn connection_and_session_drop_release_claim_without_protocol_close() {
    let mut provider = Provider::new();
    let connection = block_on(Connection::open_emulated(
        &mut provider,
        SELECTION,
        options(128),
    ))
    .unwrap();
    drop(connection);
    assert!(!provider.model.try_lock().unwrap().claimed);
    let session = open(&mut provider, 128);
    drop(session);
    let model = provider
        .model
        .try_lock()
        .expect("serialized endpoint state");
    assert!(!model.claimed);
    assert!(model.session_open);
}

#[test]
fn descriptor_selection_rejects_missing_duplicate_and_ambiguous_interfaces() {
    let mut provider = Provider::new();
    let missing = InterfaceSelection {
        alternate: 1,
        ..SELECTION
    };
    assert!(matches!(
        block_on(Connection::open_emulated(
            &mut provider,
            missing,
            options(128)
        )),
        Err(Error::InvalidInterface)
    ));
    provider.descriptors.push(provider.descriptors[0].clone());
    assert!(matches!(
        block_on(Connection::open_emulated(
            &mut provider,
            SELECTION,
            options(128)
        )),
        Err(Error::InvalidInterface)
    ));
    provider.descriptors.pop();
    let extra = EndpointDescriptor {
        address: EndpointAddress::new(0x85).unwrap(),
        ..provider.descriptors[0].endpoints[0]
    };
    provider.descriptors[0].endpoints.push(extra);
    assert!(matches!(
        block_on(Connection::open_emulated(
            &mut provider,
            SELECTION,
            options(128)
        )),
        Err(Error::AmbiguousEndpoints)
    ));
    assert!(!provider.model.try_lock().unwrap().claimed);
    for address in [0, 0x80, 0x91, 0x71] {
        assert!(EndpointAddress::new(address).is_none());
    }
}

#[test]
fn budgets_are_validated_before_claim_or_command() {
    let mut provider = Provider::new();
    assert!(matches!(
        block_on(Connection::open_emulated(
            &mut provider,
            SELECTION,
            options(7)
        )),
        Err(Error::LengthLimit { .. })
    ));
    let invalid = TransferOptions {
        timeout: Duration::ZERO,
        ..options(128)
    };
    assert!(matches!(
        block_on(Connection::open_emulated(&mut provider, SELECTION, invalid)),
        Err(Error::InvalidTimeout)
    ));
    assert!(!provider.model.try_lock().unwrap().claimed);
    let mut session = open(&mut provider, 8);
    assert!(matches!(
        block_on(session.device_info(InfoSelector::Product)),
        Err(Error::LengthLimit {
            requested: 128,
            limit: 8
        })
    ));
    assert_eq!(session.state(), SessionState::Ready);
    assert_eq!(block_on(session.ping()).unwrap(), [42, 0]);
    block_on(session.close()).unwrap();
}

#[test]
fn cancellation_fences_session_without_sending_close() {
    let mut provider = Provider::new();
    let mut session = open(&mut provider, 128);
    provider
        .model
        .try_lock()
        .expect("serialized endpoint state")
        .read_behavior = ReadBehavior::Pending;
    let mut ping = Box::pin(session.ping());
    assert!(block_on(poll_once(ping.as_mut())).is_none());
    drop(ping);
    assert_eq!(session.state(), SessionState::CompletionUnknown);
    assert!(matches!(
        block_on(session.ping()),
        Err(Error::CompletionUnknown)
    ));
    assert!(matches!(
        block_on(session.close()),
        Err(Error::CompletionUnknown)
    ));
    assert!(!provider.model.try_lock().unwrap().claimed);
    assert!(provider.model.try_lock().unwrap().session_open);
}

#[test]
fn pending_transfers_have_deadlines_and_fence_session() {
    let mut provider = Provider::new();
    let mut session = open(&mut provider, 128);
    provider
        .model
        .try_lock()
        .expect("serialized endpoint state")
        .read_behavior = ReadBehavior::Pending;
    assert!(matches!(
        block_on(session.ping()),
        Err(Error::Timeout(TransferStage::BulkIn))
    ));
    assert_eq!(session.state(), SessionState::CompletionUnknown);
    drop(session);
    assert!(!provider.model.try_lock().unwrap().claimed);
}

#[test]
fn failed_cancelled_and_invalid_completions_preserve_error_and_fence() {
    for behavior in [
        ReadBehavior::Disconnected,
        ReadBehavior::Cancelled,
        ReadBehavior::Overreported,
        ReadBehavior::Short,
    ] {
        let mut provider = Provider::new();
        let mut session = open(&mut provider, 128);
        provider
            .model
            .try_lock()
            .expect("serialized endpoint state")
            .read_behavior = behavior;
        let error = block_on(session.ping()).unwrap_err();
        match behavior {
            ReadBehavior::Disconnected | ReadBehavior::Cancelled => {
                let Error::Transfer {
                    stage,
                    source,
                    transferred,
                    partial,
                } = error
                else {
                    panic!("expected transfer error")
                };
                assert_eq!(stage, TransferStage::BulkIn);
                assert_eq!(
                    source,
                    if matches!(behavior, ReadBehavior::Cancelled) {
                        TransferError::Cancelled
                    } else {
                        TransferError::Disconnected
                    }
                );
                assert_eq!(transferred, Some(1));
                assert_eq!(partial, [42]);
            }
            ReadBehavior::Overreported => assert!(matches!(
                error,
                Error::EmulatorCompletion {
                    stage: TransferStage::BulkIn,
                    capacity: 2,
                    actual: 3
                }
            )),
            ReadBehavior::Short => assert!(matches!(
                error,
                Error::Protocol(ProtocolError::ReplyLength {
                    expected: 2,
                    actual: 1
                })
            )),
            _ => unreachable!(),
        }
        assert_eq!(session.state(), SessionState::CompletionUnknown);
        assert!(matches!(
            block_on(session.ping()),
            Err(Error::CompletionUnknown)
        ));
    }
}

#[test]
fn short_control_and_out_completions_fence_session() {
    for control in [true, false] {
        let mut provider = Provider::new();
        let mut session = open(&mut provider, 128);
        {
            let mut model = provider
                .model
                .try_lock()
                .expect("serialized endpoint state");
            model.short_control = control;
            model.short_write = !control;
        }
        assert!(matches!(
            block_on(session.device_info(InfoSelector::Product)),
            Err(Error::ShortTransfer { actual: 1, .. })
        ));
        assert_eq!(session.state(), SessionState::CompletionUnknown);
    }
}

#[test]
fn malformed_command_reply_fences_and_failed_open_releases_claim() {
    let mut provider = Provider::new();
    let mut session = open(&mut provider, 128);
    provider
        .model
        .try_lock()
        .expect("serialized endpoint state")
        .read_behavior = ReadBehavior::Malformed;
    assert!(matches!(
        block_on(session.device_info(InfoSelector::Product)),
        Err(Error::Protocol(ProtocolError::CommandEcho { .. }))
    ));
    assert_eq!(session.state(), SessionState::CompletionUnknown);
    drop(session);
    let mut provider = Provider::new();
    provider
        .model
        .try_lock()
        .expect("serialized endpoint state")
        .read_behavior = ReadBehavior::Malformed;
    let connection = block_on(Connection::open_emulated(
        &mut provider,
        SELECTION,
        options(128),
    ))
    .unwrap();
    assert!(matches!(
        block_on(connection.begin_session(None)),
        Err(Error::Protocol(ProtocolError::ZeroPacketLimit))
    ));
    assert!(!provider.model.try_lock().unwrap().claimed);
}

#[test]
fn claim_and_release_failures_and_deadlines_release_resources() {
    for behavior in [ResourceBehavior::Pending, ResourceBehavior::Disconnected] {
        let mut provider = Provider::new();
        provider
            .model
            .try_lock()
            .expect("serialized endpoint state")
            .claim_behavior = behavior;
        let result = block_on(Connection::open_emulated(
            &mut provider,
            SELECTION,
            options(128),
        ));
        match behavior {
            ResourceBehavior::Pending => assert!(matches!(
                result,
                Err(Error::EmulatorResourceTimeout(ResourceOperation::Claim))
            )),
            ResourceBehavior::Disconnected => assert!(matches!(
                result,
                Err(Error::EmulatorResource {
                    operation: ResourceOperation::Claim,
                    source: TransferError::Disconnected
                })
            )),
            _ => unreachable!(),
        }
        assert!(!provider.model.try_lock().unwrap().claimed);
        let mut provider = Provider::new();
        let session = open(&mut provider, 128);
        provider
            .model
            .try_lock()
            .expect("serialized endpoint state")
            .release_behavior = behavior;
        let result = block_on(session.close());
        match behavior {
            ResourceBehavior::Pending => assert!(matches!(
                result,
                Err(Error::EmulatorResourceTimeout(ResourceOperation::Release))
            )),
            ResourceBehavior::Disconnected => assert!(matches!(
                result,
                Err(Error::EmulatorResource {
                    operation: ResourceOperation::Release,
                    source: TransferError::Disconnected
                })
            )),
            _ => unreachable!(),
        }
        assert!(!provider.model.try_lock().unwrap().claimed);
        assert!(!provider.model.try_lock().unwrap().session_open);
    }
}

#[test]
fn cancelled_claim_and_session_open_release_resources() {
    let mut provider = Provider::new();
    provider.model.try_lock().unwrap().claim_behavior = ResourceBehavior::Pending;
    let mut claim = Box::pin(Connection::open_emulated(
        &mut provider,
        SELECTION,
        options(128),
    ));
    assert!(block_on(poll_once(claim.as_mut())).is_none());
    drop(claim);
    assert!(!provider.model.try_lock().unwrap().claimed);
    provider.model.try_lock().unwrap().claim_behavior = ResourceBehavior::Normal;
    provider.model.try_lock().unwrap().read_behavior = ReadBehavior::Pending;
    let connection = block_on(Connection::open_emulated(
        &mut provider,
        SELECTION,
        options(128),
    ))
    .unwrap();
    let mut opening = Box::pin(connection.begin_session(None));
    assert!(block_on(poll_once(opening.as_mut())).is_none());
    drop(opening);
    assert!(!provider.model.try_lock().unwrap().claimed);
    assert!(provider.model.try_lock().unwrap().session_open);
}

#[test]
fn an_owned_claim_prevents_a_second_connection() {
    let mut provider = Provider::new();
    let connection = block_on(Connection::open_emulated(
        &mut provider,
        SELECTION,
        options(128),
    ))
    .unwrap();
    assert!(matches!(
        block_on(Connection::open_emulated(
            &mut provider,
            SELECTION,
            options(128)
        )),
        Err(Error::EmulatorResource {
            operation: ResourceOperation::Claim,
            source: TransferError::Stall,
        })
    ));
    assert!(provider.model.try_lock().unwrap().claimed);
    drop(connection);
    assert!(!provider.model.try_lock().unwrap().claimed);
}

#[test]
fn stale_emulated_session_can_be_explicitly_reclaimed_and_reopened() {
    let mut provider = Provider::new();
    drop(open(&mut provider, 256));
    let stale = block_on(Connection::open_emulated(
        &mut provider,
        SELECTION,
        options(256),
    ))
    .unwrap();
    assert!(matches!(
        block_on(stale.begin_session(None)),
        Err(Error::SessionAlreadyOpen)
    ));
    let mut connection = block_on(Connection::open_emulated(
        &mut provider,
        SELECTION,
        options(256),
    ))
    .unwrap();
    assert!(block_on(connection.reclaim_session()).unwrap());
    let session = block_on(connection.begin_session(None)).unwrap();
    block_on(session.close()).unwrap();
}

#[test]
fn callback_invocation_uses_emulated_endpoint_memory_and_disconnect() {
    use fujimod_usb::{
        ff80::commands::{
            memory::{Address, DebugMemoryLimits},
            research,
        },
        operations::callback::*,
    };
    for disconnect in [false, true] {
        let mut provider = Provider::new();
        let mut session = open(&mut provider, 256);
        provider.model.try_lock().unwrap().callback_disconnect = disconnect;
        let trigger = research::Request::new(
            research::CommandCode(0x450006),
            vec![0; 16],
            NonZeroU32::new(32).unwrap(),
        )
        .unwrap();
        let mut reply = vec![0; 32];
        reply[..4].copy_from_slice(&0x450006u32.to_le_bytes());
        let plan = Invocation {
            preconditions: vec![],
            writes: vec![GuardedWrite {
                address: Address(0x200),
                accepted: vec![vec![0; 4]],
                bytes: vec![1, 2, 3, 4],
            }],
            synchronization: vec![],
            slot: CallbackSlot {
                address: Address(0x100),
                required: vec![0; 8],
                value: vec![9; 8],
            },
            trigger,
            expected: if disconnect {
                ExpectedOutcome::NoReply
            } else {
                ExpectedOutcome::Reply(vec![reply])
            },
            limits: DebugMemoryLimits {
                read: NonZeroU32::new(64).unwrap(),
                write: NonZeroU32::new(64).unwrap(),
            },
        };
        let result = block_on(invoke(&mut session, &plan)).unwrap();
        assert_eq!(result.is_none(), disconnect);
        let model = provider.model.try_lock().unwrap();
        assert!(model.triggered);
        assert_eq!(model.memory[&0x200], 1);
        assert_eq!(model.memory[&0x100], if disconnect { 9 } else { 0 });
        assert_eq!(
            session.state(),
            if disconnect {
                SessionState::CompletionUnknown
            } else {
                SessionState::Ready
            }
        );
    }
}

#[test]
fn emulated_optional_reclaim_timeout_still_allows_fresh_open() {
    let mut provider = Provider::new();
    provider.model.try_lock().unwrap().session_open = true;
    let mut connection = block_on(Connection::open_emulated(
        &mut provider,
        SELECTION,
        options(256),
    ))
    .unwrap();
    provider.model.try_lock().unwrap().read_behavior = ReadBehavior::Pending;
    assert!(!block_on(connection.reclaim_session()).unwrap());
    provider.model.try_lock().unwrap().read_behavior = ReadBehavior::Normal;
    let session = block_on(connection.begin_session(None)).unwrap();
    block_on(session.close()).unwrap();
}
