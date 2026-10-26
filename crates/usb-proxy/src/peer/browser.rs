use crate::{Error, Result};
use wasm_bindgen::prelude::*;
#[wasm_bindgen(module = "/web/build/peer.js")]
extern "C" {
    #[wasm_bindgen(catch, js_name=connectPeer)]
    async fn connect_peer(url: &str) -> std::result::Result<JsValue, JsValue>;
    type Channel;
    #[wasm_bindgen(method, catch)]
    async fn send(
        this: &Channel,
        bytes: &js_sys::Uint8Array,
    ) -> std::result::Result<JsValue, JsValue>;
    #[wasm_bindgen(method, catch)]
    async fn receive(this: &Channel) -> std::result::Result<JsValue, JsValue>;
    #[wasm_bindgen(method)]
    fn close(this: &Channel);
}
fn error(value: JsValue) -> Error {
    Error(value.as_string().unwrap_or_else(|| format!("{value:?}")))
}
pub(crate) struct PeerSocket(Channel);
impl PeerSocket {
    pub async fn connect(url: &str, progress: &crate::Progress) -> Result<Self> {
        let channel = connect_peer(url).await.map_err(error)?.unchecked_into();
        progress("WebRTC data channel open");
        Ok(Self(channel))
    }
    pub async fn send(&mut self, bytes: Vec<u8>) -> Result<()> {
        // Own the JS buffer across backpressure awaits; Wasm memory may grow.
        let data = js_sys::Uint8Array::from(bytes.as_slice());
        self.0.send(&data).await.map_err(error)?;
        Ok(())
    }
    pub async fn receive(&mut self) -> Result<Vec<u8>> {
        let value = self.0.receive().await.map_err(error)?;
        if !value.is_instance_of::<js_sys::Uint8Array>() {
            return Err(Error("invalid binary peer message".into()));
        }
        Ok(js_sys::Uint8Array::new(&value).to_vec())
    }
}
impl Drop for PeerSocket {
    fn drop(&mut self) {
        self.0.close();
    }
}
