//! Frame viewer (loopback by default). Frames are published by display_output.rs;
//! key requests call the physical input device API, never guest UI globals.
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub type KeyHandler = Arc<dyn Fn(&str, bool) -> Result<(), String> + Send + Sync>;
pub struct Viewer {
    address: SocketAddr,
    url: String,
    stopped: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}
impl Viewer {
    pub fn with_bind(
        frames: PathBuf,
        bind: IpAddr,
        port: u16,
        hostname: Option<&str>,
        keys: Option<KeyHandler>,
    ) -> Result<Self, String> {
        if hostname.is_some_and(|s| {
            s.is_empty()
                || !s
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
        }) {
            return Err("serve hostname must be a DNS name without scheme or port".into());
        }
        let listener = TcpListener::bind((bind, port)).map_err(|e| e.to_string())?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let address = listener.local_addr().map_err(|e| e.to_string())?;
        let origin = format!("http://{address}");
        let url = hostname.map_or_else(
            || origin.clone(),
            |name| format!("http://{name}:{}", address.port()),
        );
        let origins = vec![origin, url.clone()];
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        let thread = thread::Builder::new()
            .name("frame-viewer".into())
            .spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            // A phone's PNG download must not delay key edges.
                            let frames = frames.clone();
                            let keys = keys.clone();
                            let origins = origins.clone();
                            let _ = thread::Builder::new().name("viewer-client".into()).spawn(
                                move || {
                                    let _ = serve(&mut stream, &frames, keys.as_ref(), &origins);
                                },
                            );
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(20))
                        }
                        Err(_) => break,
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            address,
            url,
            stopped,
            thread: Some(thread),
        })
    }
    pub fn address(&self) -> SocketAddr {
        self.address
    }
    pub fn url(&self) -> &str {
        &self.url
    }
}
impl Drop for Viewer {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
fn reply(stream: &mut TcpStream, status: &str, kind: &str, body: &[u8]) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\nX-Content-Type-Options: nosniff\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)
}
fn serve(
    stream: &mut TcpStream,
    frames: &Path,
    keys: Option<&KeyHandler>,
    origins: &[String],
) -> std::io::Result<()> {
    // macOS accepted sockets inherit the listener's nonblocking flag. Leave
    // accept nonblocking, but complete each bounded request/PNG before close.
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(1)))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let mut request = Vec::new();
    let end = loop {
        if std::time::Instant::now() >= deadline {
            return reply(
                stream,
                "408 Request Timeout",
                "text/plain",
                b"request timed out",
            );
        }
        if request.len() >= 8192 {
            return reply(
                stream,
                "413 Content Too Large",
                "text/plain",
                b"request header too large",
            );
        }
        let mut b = [0; 1024];
        let n = stream.read(&mut b)?;
        if n == 0 {
            return Ok(());
        }
        request.extend_from_slice(&b[..n]);
        if let Some(e) = request.windows(4).position(|w| w == b"\r\n\r\n") {
            break e + 4;
        }
    };
    let Ok(header) = std::str::from_utf8(&request[..end]) else {
        return reply(stream, "400 Bad Request", "text/plain", b"invalid header");
    };
    let mut lines = header.split("\r\n");
    let first = lines.next().unwrap_or("");
    let mut fields = first.split_whitespace();
    let method = fields.next().unwrap_or("");
    let path = fields.next().unwrap_or("").split('?').next().unwrap_or("");
    let mut length = 0usize;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                let Ok(n) = value.trim().parse::<usize>() else {
                    return reply(
                        stream,
                        "400 Bad Request",
                        "text/plain",
                        b"invalid body length",
                    );
                };
                length = n;
            }
            if name.eq_ignore_ascii_case("transfer-encoding") {
                return reply(
                    stream,
                    "400 Bad Request",
                    "text/plain",
                    b"chunked requests unsupported",
                );
            }
            // Browser requests from another page must not inject camera keys.
            if name.eq_ignore_ascii_case("origin") {
                if !origins.iter().any(|origin| origin == value.trim()) {
                    return reply(stream, "403 Forbidden", "text/plain", b"foreign origin");
                }
            }
        }
    }
    match (method, path) {
        ("GET", "/") => reply(
            stream,
            "200 OK",
            "text/html; charset=utf-8",
            PAGE.as_bytes(),
        ),
        ("GET", "/bank0.png" | "/bank1.png") => {
            let file = frames.join(format!("latest-{}", &path[1..]));
            match std::fs::read(file) {
                Ok(png) => reply(stream, "200 OK", "image/png", &png),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => reply(
                    stream,
                    "404 Not Found",
                    "text/plain",
                    b"waiting for guest frame",
                ),
                Err(_) => reply(
                    stream,
                    "500 Internal Server Error",
                    "text/plain",
                    b"frame read failed",
                ),
            }
        }
        ("GET", "/status") => {
            let banks: Vec<serde_json::Value> = (0..2)
                .map(|bank| {
                    let mut status = std::fs::read(frames.join(format!("bank{bank}-status.json")))
                        .ok()
                        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                        .unwrap_or(serde_json::json!({"bank": bank, "enabled": false}));
                    if let Ok(bytes) = std::fs::read(frames.join(format!("bank{bank}-publication.json"))) {
                        if let Ok(publication) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                            status["frames"] = publication["frames"].clone();
                        }
                    }
                    status["has_frame"] = frames
                        .join(format!("latest-bank{bank}.png"))
                        .is_file()
                        .into();
                    status
                })
                .collect();
            let body =
                serde_json::json!({"banks": banks, "keys_available": keys.is_some()}).to_string();
            reply(stream, "200 OK", "application/json", body.as_bytes())
        }
        ("POST", "/key") => {
            if length == 0 || length > 1024 {
                return reply(
                    stream,
                    "400 Bad Request",
                    "text/plain",
                    b"expected JSON key and pressed",
                );
            }
            while request.len() < end + length {
                if std::time::Instant::now() >= deadline {
                    return reply(
                        stream,
                        "408 Request Timeout",
                        "text/plain",
                        b"request timed out",
                    );
                }
                let mut b = [0; 1024];
                let n = stream.read(&mut b[..(end + length - request.len()).min(1024)])?;
                if n == 0 {
                    return reply(stream, "400 Bad Request", "text/plain", b"short body");
                }
                request.extend_from_slice(&b[..n]);
            }
            let value: serde_json::Value = match serde_json::from_slice(&request[end..end + length])
            {
                Ok(v) => v,
                Err(_) => return reply(stream, "400 Bad Request", "text/plain", b"invalid JSON"),
            };
            let (Some(key), Some(pressed)) = (value["key"].as_str(), value["pressed"].as_bool())
            else {
                return reply(
                    stream,
                    "400 Bad Request",
                    "text/plain",
                    b"expected key:string and pressed:boolean",
                );
            };
            let Some(handler) = keys else {
                return reply(
                    stream,
                    "501 Not Implemented",
                    "text/plain",
                    b"physical key injection unavailable",
                );
            };
            match handler(key, pressed) {
                Ok(()) => reply(stream, "200 OK", "application/json", b"{\"ok\":true}"),
                Err(e) => reply(
                    stream,
                    "422 Unprocessable Content",
                    "text/plain",
                    e.as_bytes(),
                ),
            }
        }
        _ => reply(stream, "404 Not Found", "text/plain", b"unknown endpoint"),
    }
}
const PAGE: &str = r#"<!doctype html><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>X100VI display</title>
<style>body{font:16px system-ui;background:#16181b;color:#eee;margin:24px}main{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:24px}img{display:block;width:100%;background:#000;min-height:200px}figure{margin:0;min-width:0}figcaption{margin-bottom:8px}.state{color:#aaa}.state[data-enabled=true]{color:#7fe5a2}.detail{font-size:13px;color:#aaa;min-height:20px;margin-top:8px}button{font:inherit;padding:12px;margin:4px;touch-action:none}#status{min-height:24px}h1{font-size:24px}@media(max-width:650px){main{grid-template-columns:1fr}}.controls{display:flex;flex-wrap:wrap;align-items:center;gap:16px;margin-top:20px}.primary,.shutter{display:grid;gap:8px}.controls button{min-height:56px;min-width:56px;margin:0;font-size:18px;cursor:pointer;user-select:none}.dpad{display:grid;grid-template-columns:repeat(3,64px);grid-template-rows:repeat(2,56px);gap:8px}.up{grid-column:2}.left{grid-column:1;grid-row:2}.down{grid-column:2;grid-row:2}.right{grid-column:3;grid-row:2}.dials{display:grid;grid-template-columns:auto 56px 56px;gap:8px;align-items:center}@media(max-width:650px){body{margin:12px}.controls{display:grid;grid-template-columns:1fr;justify-items:stretch}.primary,.shutter{grid-template-columns:1fr 1fr}.dpad{justify-self:center}.dials{grid-template-columns:1fr 64px 64px}.keyboard{display:none}img{min-height:0}}</style>
<h1>X100VI display</h1><main><figure><figcaption>Bank 0 · <span class="state" id="s0">waiting</span></figcaption><img id="b0" alt="Waiting for guest frame"><div class="detail" id="d0"></div></figure><figure><figcaption>Bank 1 · <span class="state" id="s1">waiting</span></figcaption><img id="b1" alt="Waiting for guest frame"><div class="detail" id="d1"></div></figure></main>
<section class="controls" aria-label="Camera controls"><div class="primary"><button data-key="MENU">MENU / OK</button><button data-key="BACK">DISP / BACK</button></div>
<div class="dpad"><button class="up" data-key="UP" aria-label="Up">↑</button><button class="left" data-key="LEFT" aria-label="Left">←</button><button class="down" data-key="DOWN" aria-label="Down">↓</button><button class="right" data-key="RIGHT" aria-label="Right">→</button></div>
<div class="shutter"><button data-key="SHUTTER_HALF">Shutter half</button><button data-key="SHUTTER_FULL">Shutter full</button></div>
<div class="dials"><span>Rear dial</span><button data-detent="REAR-" aria-label="Rear dial minus">−</button><button data-detent="REAR+" aria-label="Rear dial plus">+</button><span>Front dial</span><button data-detent="FRONT-" aria-label="Front dial minus">−</button><button data-detent="FRONT+" aria-label="Front dial plus">+</button></div></section>
<p class="detail keyboard">Keyboard: arrows · Enter = MENU/OK · Esc = BACK · H/F = shutter half/full · [ / ] = rear dial · , / . = front dial</p><p id="status" role="status"></p>
<script>
const status=document.querySelector('#status'),held=new Set(),pressedAt=new Map();
async function send(k,p){try{const r=await fetch('/key',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({key:k,pressed:p})});status.textContent=r.ok?'':await r.text()}catch(e){status.textContent=e.message}}
async function key(k,p){if(p===held.has(k))return;if(p){pressedAt.set(k,performance.now());held.add(k)}else{held.delete(k);await new Promise(r=>setTimeout(r,Math.max(0,150-(performance.now()-pressedAt.get(k)))));if(held.has(k))return;}await send(k,p)}
for(const b of document.querySelectorAll('[data-key]')){b.onpointerdown=e=>{e.preventDefault();b.setPointerCapture(e.pointerId);key(b.dataset.key,true)};b.onpointerup=b.onpointercancel=b.onlostpointercapture=()=>key(b.dataset.key,false);b.onclick=e=>{if(e.detail===0){key(b.dataset.key,true);key(b.dataset.key,false)}}}
for(const b of document.querySelectorAll('[data-detent]'))b.onclick=()=>send(b.dataset.detent,true);
const mapping={ArrowUp:'UP',ArrowDown:'DOWN',ArrowLeft:'LEFT',ArrowRight:'RIGHT',Enter:'MENU',Escape:'BACK',h:'SHUTTER_HALF',f:'SHUTTER_FULL'},dials={'[':'REAR-',']':'REAR+',',':'FRONT-','.':'FRONT+'};
onkeydown=e=>{const k=mapping[e.key]||mapping[e.key.toLowerCase()],d=dials[e.key];if(k){e.preventDefault();key(k,true)}else if(d){e.preventDefault();if(!e.repeat)send(d,true)}};onkeyup=e=>{const k=mapping[e.key]||mapping[e.key.toLowerCase()];if(k){e.preventDefault();key(k,false)}};onblur=()=>{for(const k of [...held])key(k,false)};document.addEventListener("visibilitychange",()=>{if(document.hidden)onblur()});
async function refresh(){try{const r=await fetch('/status'),s=await r.json();for(const b of s.banks){const n=b.bank,im=document.querySelector('#b'+n),label=document.querySelector('#s'+n),detail=document.querySelector('#d'+n);label.textContent=b.enabled?'enabled':'disabled';label.dataset.enabled=b.enabled;const layers=[...(b.picture||[]).flatMap((on,i)=>on?['picture '+i]:[]),...(b.osd||[]).flatMap((on,i)=>on?['OSD '+i]:[])];detail.textContent=[b.width&&b.height?b.width+'×'+b.height+' (total '+b.total_width+'×'+b.total_height+')':'timing not programmed',layers.length?layers.join(', '):'no planes enabled',b.error||'',!b.enabled&&b.has_frame?'showing last frame':''].filter(Boolean).join(' · ');if(b.has_frame&&im.dataset.frame!==String(b.frames)){const frame=await fetch('/bank'+n+'.png');if(frame.ok){const u=URL.createObjectURL(await frame.blob()),old=im.dataset.url;im.src=u;im.dataset.url=u;im.dataset.frame=b.frames;if(old)URL.revokeObjectURL(old)}}}for(const b of document.querySelectorAll('button'))b.disabled=!s.keys_available}catch(e){status.textContent=e.message}setTimeout(refresh,200)}refresh();
</script>"#;

/// PNG RGBA8 with a zlib stream of stored DEFLATE blocks; no codec dependency.
pub fn rgba_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    if width == 0
        || height == 0
        || width > 8192
        || height > 8192
        || rgba.len() != width as usize * height as usize * 4
    {
        return Err("invalid RGBA frame dimensions".into());
    }
    let mut raw = Vec::with_capacity(rgba.len() + height as usize);
    for row in rgba.chunks_exact(width as usize * 4) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut z = vec![0x78, 0x01];
    let count = raw.len().div_ceil(65535);
    for (n, b) in raw.chunks(65535).enumerate() {
        z.push(u8::from(n + 1 == count));
        let len = b.len() as u16;
        z.extend_from_slice(&len.to_le_bytes());
        z.extend_from_slice(&(!len).to_le_bytes());
        z.extend_from_slice(b);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &v in &raw {
        a = (a + v as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut png, b"IHDR", &ihdr);
    chunk(&mut png, b"IDAT", &z);
    chunk(&mut png, b"IEND", &[]);
    Ok(png)
}
fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(bytes);
    let mut crc = !0u32;
    for &b in kind.iter().chain(bytes) {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ if crc & 1 != 0 { 0xedb88320 } else { 0 };
        }
    }
    out.extend_from_slice(&(!crc).to_be_bytes());
}
