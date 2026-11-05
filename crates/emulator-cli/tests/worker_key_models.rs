//! The worker's declared key model reaches the board: a ladder key edge sent
//! over the JSON-lines protocol changes what the guest reads from the ADC.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
};

const RAM: u64 = 0x10000;
const ADC: u64 = 0xff90_6000;
const RESULT: u64 = 0x11000;
const RELEASED: u16 = 0x456;

/// AArch64 loop: read ADC channel 1 (`+0x14`) and store it at `RESULT`.
fn program() -> Vec<u8> {
    [0xb940_1420u32, 0xb900_0040, 0x17ff_fffe]
        .iter()
        .flat_map(|word| word.to_le_bytes())
        .collect()
}

/// Synthetic ADC ladder: bands and samples are test inputs, not body facts.
fn ladder_keys() -> Value {
    json!({"kind":"ladder-keys","ladders":[{"wire":{"kind":"adc","channel":1},"levels":4096,"rungs":[
        {"key":"Menu","band_low":100,"band_high":200,"sample":150,"basis":"test fixture"},
        {"key":"StickPush","band_low":300,"band_high":400,"sample":350,"basis":"test fixture"}]}]})
}

fn config(directory: &Path, controls: Value) -> Value {
    let program = program();
    std::fs::write(directory.join("program.bin"), &program).unwrap();
    json!({
        "schema": 1, "engine": "unicorn", "mode": "strict",
        "instructions_per_tick": [1, 1], "quantum": 16,
        "ram": [{"base": RAM, "size": 0x4000}],
        "processors": [{"model": "armv8a", "released": true, "mprofile": null, "rprofile": null, "aprofile": null,
            "registers": [{"name":"pc","value":RAM},{"name":"pstate","value":0x3c5},
                {"name":"x1","value":ADC},{"name":"x2","value":RESULT}]}],
        "mappings": [{"core":0,"address":RAM,"physical":RAM,"size":0x4000,"permissions":7}],
        "peripherals": [{"core":0,"address":ADC,"physical":ADC,"size":0x1000}],
        "interrupts": null,
        "devices": {"kind":"xp5","companion":null,"storage":null,"displays":[],
            "windows":[{"kind":"adc","base":ADC,"size":0x1000,"options":{"samples":vec![RELEASED; 15]}}],
            "controls": controls},
        "images": [{"address": RAM, "asset": "program"}],
        "assets": [{"id":"program","content":{"size":program.len(),
            "sha256":format!("{:x}", Sha256::digest(&program))}}],
        "inputs": {"root": directory, "files": {"program": {"kind":"file","path":"program.bin"}},
            "max_asset_bytes": program.len()},
        "output_directory": directory.join("frames"),
    })
}

struct Worker {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}
impl Worker {
    fn spawn() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_fujimod-emulator-worker"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Self { child, stdin, stdout }
    }
    fn call(&mut self, request: Value) -> Result<Value, String> {
        writeln!(self.stdin, "{request}").unwrap();
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        let reply: Value = serde_json::from_str(&line).unwrap();
        if reply["ok"] == true {
            Ok(reply["result"].clone())
        } else {
            Err(reply["error"].as_str().unwrap().to_owned())
        }
    }
    fn read_sample(&mut self) -> u16 {
        let result = self
            .call(json!({"command":"run-instructions","count":64,"until":null}))
            .unwrap();
        assert!(result["failure"].is_null(), "{result}");
        let hex = self
            .call(json!({"command":"read-memory","core":0,"address":RESULT,"size":2}))
            .unwrap();
        let hex = hex["hex"].as_str().unwrap();
        u16::from_le_bytes([
            u8::from_str_radix(&hex[0..2], 16).unwrap(),
            u8::from_str_radix(&hex[2..4], 16).unwrap(),
        ])
    }
    /// Close the process; a worker that never created a board refuses the
    /// command but still exits.
    fn close(mut self) {
        let _ = self.call(json!({"command":"close"}));
        assert!(self.child.wait().unwrap().success());
    }
}

fn directory(name: &str) -> std::path::PathBuf {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    directory
}

#[test]
fn ladder_key_edges_reach_the_adc_the_guest_reads_and_undeclared_models_are_refused() {
    let directory = directory("worker-ladder-keys");
    let mut worker = Worker::spawn();
    worker
        .call(json!({"command":"create","config":config(&directory, ladder_keys())}))
        .unwrap();
    assert_eq!(worker.read_sample(), RELEASED);

    worker
        .call(json!({"command":"input","key":"STICK_PUSH","pressed":true,"at":null}))
        .unwrap();
    assert_eq!(worker.read_sample(), 350);
    worker
        .call(json!({"command":"input","key":"STICK_PUSH","pressed":false,"at":null}))
        .unwrap();
    assert_eq!(worker.read_sample(), RELEASED);
    worker
        .call(json!({"command":"input","key":"MENU","pressed":true,"at":null}))
        .unwrap();
    assert_eq!(worker.read_sample(), 150);

    // A key without a rung has no electrical description in this model.
    let error = worker
        .call(json!({"command":"input","key":"DISP/BACK","pressed":true,"at":null}))
        .unwrap_err();
    assert!(error.contains("no declared ladder rung"), "{error}");
    worker.close();

    // Key models are named by mechanism; a camera name is not one. One worker
    // at a time, so this shares the test above.
    let mut worker = Worker::spawn();
    let mut controls = ladder_keys();
    controls["kind"] = json!("x100vi");
    let error = worker
        .call(json!({"command":"create","config":config(&directory, controls)}))
        .unwrap_err();
    assert!(error.contains("unknown variant"), "{error}");
    worker.close();
}
