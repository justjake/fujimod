//! Controller-side observations for a proxy device. The physical owner stays
//! at the exporting browser; this tracks this client's session and wire results.
use crate::{
    owner::{OwnerEvent, SessionOwner},
    owner_log::{LogEvents, Recorder},
};
use fujimod_usb_proxy::{Completion, Operation};
use sha2::{Digest, Sha256};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
struct State {
    event: OwnerEvent,
    watchers: Vec<async_channel::Sender<OwnerEvent>>,
    log: Recorder,
    closed: bool,
}
pub struct Observation {
    state: Mutex<State>,
    monitoring: AtomicBool,
}
impl Observation {
    pub fn new(source: &str, token: &str, route: &str, present: bool) -> Arc<Self> {
        let mut hash = Sha256::new();
        for value in [source, token, route] {
            hash.update((value.len() as u64).to_le_bytes());
            hash.update(value.as_bytes());
        }
        let id = format!("remote-{:x}", hash.finalize());
        Arc::new(Self {
            state: Mutex::new(State {
                event: OwnerEvent {
                    id,
                    present,
                    open: true,
                    sharing: false,
                    session_owner: None,
                    fenced: false,
                },
                watchers: Vec::new(),
                log: Recorder::default(),
                closed: false,
            }),
            monitoring: AtomicBool::new(false),
        })
    }
    fn publish(state: &mut State) {
        let event = state.event.clone();
        state
            .watchers
            .retain(|sender| sender.try_send(event.clone()).is_ok());
    }
    pub fn status(&self) -> OwnerEvent {
        self.state.lock().unwrap().event.clone()
    }
    pub fn watch(&self) -> async_channel::Receiver<OwnerEvent> {
        let (sender, receiver) = async_channel::unbounded();
        let mut state = self.state.lock().unwrap();
        sender.try_send(state.event.clone()).unwrap();
        state.watchers.push(sender);
        receiver
    }
    pub fn watch_log(&self) -> Arc<LogEvents> {
        self.state.lock().unwrap().log.watch()
    }
    pub fn start_monitor(&self) -> bool {
        !self.monitoring.swap(true, Ordering::Relaxed)
    }
    pub fn monitoring_ended(&self) {
        self.monitoring.store(false, Ordering::Relaxed);
    }
    pub fn resume(&self) {
        let mut state = self.state.lock().unwrap();
        state.closed = false;
        state.event.open = true;
        Self::publish(&mut state);
    }
    pub fn closed(&self) -> bool {
        self.state.lock().unwrap().closed
    }
    pub fn presence(&self, present: bool) {
        let mut state = self.state.lock().unwrap();
        if state.event.present == present {
            return;
        }
        state.event.present = present;
        if !present && state.event.session_owner.is_some() {
            state.event.fenced = true;
            state
                .log
                .fail("Remote device disconnected; completion unknown");
        }
        Self::publish(&mut state);
    }
    pub fn open(&self) -> fujimod_usb_proxy::Result<()> {
        let mut state = self.state.lock().unwrap();
        if !state.event.present {
            return Err(fujimod_usb_proxy::Error(
                "Remote device is disconnected".into(),
            ));
        }
        if state.event.session_owner.is_some() {
            return Err(fujimod_usb_proxy::Error(
                "Remote device session is held".into(),
            ));
        }
        state.closed = false;
        state.event.open = true;
        state.event.fenced = false;
        Self::publish(&mut state);
        Ok(())
    }
    pub fn close(&self) {
        let mut state = self.state.lock().unwrap();
        state.closed = true;
        state.event.open = false;
        Self::publish(&mut state);
    }
    pub fn acquire(self: &Arc<Self>, tool: Option<String>) -> fujimod_usb_proxy::Result<Lease> {
        let mut state = self.state.lock().unwrap();
        if !state.event.present
            || !state.event.open
            || state.event.fenced
            || state.event.session_owner.is_some()
        {
            return Err(fujimod_usb_proxy::Error(
                "Remote device is gone, uncertain, or held".into(),
            ));
        }
        state.event.session_owner = Some(SessionOwner::Local { tool });
        Self::publish(&mut state);
        Ok(Lease {
            observation: self.clone(),
            clean: false,
        })
    }
}
pub struct Lease {
    observation: Arc<Observation>,
    clean: bool,
}
impl Lease {
    pub fn release(&mut self) {
        self.clean = true;
    }
    pub fn failed(&self, message: &str) {
        let mut state = self.observation.state.lock().unwrap();
        state.event.fenced = true;
        state.log.fail(message);
        Observation::publish(&mut state);
    }
    pub fn record(&self, operation: &Operation, completion: &Completion) {
        let mut state = self.observation.state.lock().unwrap();
        match operation {
            Operation::ControlOut { data, .. } => {
                let event = state.event.clone();
                if let Some(holder) = event.session_owner {
                    state.log.start(&event.id, holder, data);
                }
                if let Some(error) = &completion.error {
                    state.log.fail(error);
                } else {
                    state.log.control_complete();
                }
            }
            Operation::TransferOut { data, .. } => state.log.payload(
                false,
                data,
                data.len(),
                completion.transferred,
                completion.error.clone(),
            ),
            Operation::TransferIn { length, .. } => state.log.payload(
                true,
                &completion.data,
                *length,
                completion.transferred,
                completion.error.clone(),
            ),
            _ => (),
        }
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        let mut state = self.observation.state.lock().unwrap();
        state
            .log
            .fail("Proxy session ended before its FF80 exchange completed");
        state.event.session_owner = None;
        if !self.clean {
            state.event.fenced = true;
        }
        Observation::publish(&mut state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ids_are_stable_and_scoped_to_host_token_and_route() {
        let id = |source, token, route| Observation::new(source, token, route, true).status().id;
        assert_eq!(
            id("host", "secret", "camera"),
            id("host", "secret", "camera")
        );
        for other in [
            id("other", "secret", "camera"),
            id("host", "other", "camera"),
            id("host", "secret", "other"),
        ] {
            assert_ne!(id("host", "secret", "camera"), other);
        }
    }
    #[test]
    fn a_named_local_client_has_one_lease_and_unknown_completion_fences_it() {
        let owner = Observation::new("host", "token", "camera", true);
        let events = owner.watch();
        assert!(events.try_recv().unwrap().present);
        let mut lease = owner.acquire(Some("identity".into())).unwrap();
        assert!(
            matches!(owner.status().session_owner, Some(SessionOwner::Local { tool: Some(tool) }) if tool == "identity")
        );
        assert!(owner.acquire(None).is_err());
        lease.release();
        drop(lease);
        assert!(!owner.status().fenced);
        assert!(owner.status().session_owner.is_none());
        drop(owner.acquire(None).unwrap());
        assert!(owner.status().fenced);
        assert!(owner.acquire(None).is_err());
        owner.presence(false);
        assert!(owner.open().is_err());
        owner.presence(true);
        owner.open().unwrap();
        assert!(!owner.status().fenced);
        owner.close();
        assert!(owner.closed());
        owner.resume();
        assert!(!owner.closed());
    }
    #[test]
    fn proxy_completions_feed_one_rust_decoded_exchange() {
        futures_lite::future::block_on(async {
            let owner = Observation::new("host", "token", "camera", true);
            let log = owner.watch_log();
            let mut lease = owner.acquire(Some("read".into())).unwrap();
            let envelope =
                crate::ff80::framing::envelope(crate::ff80::framing::RequestKind::Open, 8, 0);
            lease.record(
                &Operation::ControlOut {
                    setup: fujimod_usb_proxy::Setup {
                        request_type: fujimod_usb_proxy::RequestType::Vendor,
                        recipient: fujimod_usb_proxy::Recipient::Other,
                        request: 0,
                        value: 0,
                        index: 0,
                    },
                    data: envelope.to_vec(),
                },
                &Completion {
                    transferred: 12,
                    ..Default::default()
                },
            );
            lease.record(
                &Operation::TransferIn {
                    endpoint: 0x82,
                    length: 64,
                },
                &Completion {
                    transferred: 8,
                    data: vec![0; 8],
                    ..Default::default()
                },
            );
            lease.release();
            drop(lease);
            let event = log.next().await.unwrap().unwrap();
            assert_eq!(event.device_id, owner.status().id);
            assert_eq!(event.decoded.operation, "Open session");
            assert_eq!(event.data, [0; 8]);
            log.close();
            assert!(log.next().await.unwrap().is_none());
        });
    }
}
