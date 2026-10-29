//! Optional strict replay of synthetic USB exchanges through the real ff80 core.
//! This module is excluded unless the `testing` feature is enabled.
use crate::{
    Error, Result,
    connection::Io,
    error::TransferStage,
    ff80::{Session, framing::RequestKind},
};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Exchange {
    Control([u8; 12]),
    ControlFailure {
        bytes: [u8; 12],
        error: nusb::transfer::TransferError,
    },
    Write(Vec<u8>),
    Read {
        maximum: usize,
        data: Vec<u8>,
    },
    /// A synthetic peer that has accepted a read but has not completed it.
    PendingRead {
        maximum: usize,
    },
    /// Complete after an actual timer wakeup, for executor integration tests.
    DelayedRead {
        maximum: usize,
        data: Vec<u8>,
        delay: std::time::Duration,
    },
    ReadFailure {
        maximum: usize,
        error: nusb::transfer::TransferError,
        partial: Vec<u8>,
    },
}
#[derive(Clone)]
pub struct Script {
    state: Arc<Mutex<State>>,
}
struct State {
    position: usize,
    steps: VecDeque<Exchange>,
}
impl Script {
    pub fn new(steps: impl IntoIterator<Item = Exchange>) -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                position: 0,
                steps: steps.into_iter().collect(),
            })),
        }
    }
    pub fn remaining(&self) -> usize {
        self.state.lock().expect("replay lock").steps.len()
    }
    pub fn connection(&self, max_reply_bytes: u32) -> Result<crate::connection::Connection> {
        crate::connection::Connection::replay(self.clone(), max_reply_bytes)
    }
    pub async fn begin_session(
        &self,
        max_reply_bytes: u32,
        trace: Option<crate::trace::Trace>,
    ) -> Result<Session> {
        Session::replay(self.clone(), max_reply_bytes, trace).await
    }
    fn take(&self) -> (usize, Option<Exchange>) {
        let mut state = self.state.lock().expect("replay lock");
        let position = state.position;
        state.position += 1;
        (position, state.steps.pop_front())
    }
    fn expect(&self, actual: Exchange) -> Result<()> {
        let (position, expected) = self.take();
        if expected.as_ref() == Some(&actual) {
            Ok(())
        } else {
            Err(Error::ReplayMismatch {
                position,
                expected: expected.map(Box::new),
                actual: Box::new(actual),
            })
        }
    }
}
impl Io for Script {
    async fn control(&mut self, kind: RequestKind, bytes: [u8; 12]) -> Result<()> {
        let (position, expected) = self.take();
        match expected {
            Some(Exchange::Control(actual)) if actual == bytes => Ok(()),
            Some(Exchange::ControlFailure {
                bytes: actual,
                error,
            }) if actual == bytes => Err(Error::Transfer {
                stage: TransferStage::Control(kind),
                source: error,
                transferred: Some(0),
                partial: vec![],
            }),
            expected => Err(Error::ReplayMismatch {
                position,
                expected: expected.map(Box::new),
                actual: Box::new(Exchange::Control(bytes)),
            }),
        }
    }
    async fn write(&mut self, bytes: &[u8]) -> Result<()> {
        self.expect(Exchange::Write(bytes.to_vec()))
    }
    async fn read(&mut self, maximum: usize) -> Result<Vec<u8>> {
        let (position, expected) = self.take();
        match expected {
            Some(Exchange::Read {
                maximum: expected,
                data,
            }) if expected == maximum => Ok(data),
            Some(Exchange::PendingRead { maximum: expected }) if expected == maximum => {
                std::future::pending().await
            }
            Some(Exchange::DelayedRead {
                maximum: expected,
                data,
                delay,
            }) if expected == maximum => {
                futures_timer::Delay::new(delay).await;
                Ok(data)
            }
            Some(Exchange::ReadFailure {
                maximum: expected,
                error,
                partial,
            }) if expected == maximum => Err(Error::Transfer {
                stage: TransferStage::BulkIn,
                source: error,
                transferred: Some(partial.len()),
                partial,
            }),
            expected => Err(Error::ReplayMismatch {
                position,
                expected: expected.map(Box::new),
                actual: Box::new(Exchange::Read {
                    maximum,
                    data: vec![],
                }),
            }),
        }
    }
}

/// Stateful virtual USB endpoint for emulator integration tests. This is
/// excluded from production builds. Callbacks must not reenter their session.
/// Framing, validation and uncertain-completion fencing still run in the core.
pub trait Peer: Send + Sync + 'static {
    fn control(&mut self, kind: RequestKind, envelope: [u8; 12]) -> Result<()>;
    fn write(&mut self, data: &[u8]) -> Result<()>;
    fn read(&mut self, maximum: usize) -> Result<Vec<u8>>;
    fn endpoint(&self, _stage: TransferStage) -> Option<u8> {
        None
    }
}

/// Open a session against a virtual endpoint without device discovery or nusb.
pub async fn peer_session(
    peer: impl Peer,
    max_reply_bytes: u32,
    trace: Option<crate::trace::Trace>,
) -> Result<Session> {
    Session::from_peer(Model(Box::new(peer)), max_reply_bytes, trace).await
}

pub(crate) struct Model(pub(crate) Box<dyn Peer>);
impl Io for Model {
    fn endpoint(&self, stage: TransferStage) -> Option<u8> {
        self.0.endpoint(stage)
    }
    async fn control(&mut self, kind: RequestKind, envelope: [u8; 12]) -> Result<()> {
        self.0.control(kind, envelope)
    }
    async fn write(&mut self, data: &[u8]) -> Result<()> {
        self.0.write(data)
    }
    async fn read(&mut self, maximum: usize) -> Result<Vec<u8>> {
        self.0.read(maximum)
    }
}
