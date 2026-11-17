//! Host transport for captured scanout. Files, PNG workers and viewers never
//! participate in modeled display state or guest timing.
use crate::frontend::{KeyHandler, Viewer};
use fujimod_framebuffer::raster::PixelOrder;
use fujimod_emulator_devices::graphics::scanout::Frame;
use fujimod_emulator_xp5_display::display::Config;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct Outputs(Arc<Mutex<BTreeMap<usize, Output>>>);
struct Output {
    directory: PathBuf,
    writer: crate::png_writer::Writer,
    next_archive: u64,
    status: String,
    _viewer: Option<Viewer>,
}
impl Outputs {
    pub fn configure(
        &self,
        base: u64,
        options: &toml::Table,
        keys: Option<KeyHandler>,
    ) -> Result<Config, String> {
        let valid = [
            "frame_dir",
            "frame_period_ns",
            "raw_format0",
            "raw_format1",
            "raw_format2",
            "raw_format3",
            "viewer_port",
            "serve_bind",
            "serve_hostname",
        ];
        if let Some(key) = options.keys().find(|k| !valid.contains(&k.as_str())) {
            return Err(format!("unknown display option {key}"));
        }
        let bank = match base {
            0xff990000 => 0,
            0xff9a0000 => 1,
            _ => return Err("invalid display output base".into()),
        };
        let number = |key: &str| {
            options
                .get(key)
                .map(|v| {
                    v.as_integer()
                        .and_then(|n| u64::try_from(n).ok())
                        .ok_or_else(|| format!("{key} must be a nonnegative integer"))
                })
                .transpose()
        };
        let directory = PathBuf::from(
            options
                .get("frame_dir")
                .and_then(toml::Value::as_str)
                .ok_or("display requires frame_dir")?,
        );
        let mut config = Config {
            period_ns: number("frame_period_ns")?,
            ..Default::default()
        };
        for (n, format) in config.raw_formats.iter_mut().enumerate() {
            if let Some(v) = options.get(&format!("raw_format{n}")) {
                *format = Some(PixelOrder::parse(
                    v.as_str().ok_or("raw format must be a string")?,
                )?);
            }
        }
        let viewer = if let Some(port) = number("viewer_port")? {
            if bank != 0 {
                return Err("start viewer only on display bank0".into());
            }
            let bind = options
                .get("serve_bind")
                .map(|v| {
                    v.as_str()
                        .ok_or("serve_bind must be an IP address")?
                        .parse::<std::net::IpAddr>()
                        .map_err(|e| e.to_string())
                })
                .transpose()?
                .unwrap_or(std::net::Ipv4Addr::LOCALHOST.into());
            let hostname = options
                .get("serve_hostname")
                .map(|v| v.as_str().ok_or("serve_hostname must be a DNS name"))
                .transpose()?;
            let viewer = Viewer::with_bind(
                directory.clone(),
                bind,
                u16::try_from(port).map_err(|e| e.to_string())?,
                hostname,
                keys,
            )?;
            eprintln!(
                "display viewer: {} (bind {})",
                viewer.url(),
                viewer.address()
            );
            Some(viewer)
        } else {
            None
        };
        let mut outputs = self.0.lock().unwrap();
        if outputs.contains_key(&bank) {
            return Err(format!("display output {bank} is already configured"));
        }
        outputs.insert(
            bank,
            Output {
                writer: crate::png_writer::Writer::new(directory.clone(), bank)?,
                directory,
                next_archive: 0,
                status: String::new(),
                _viewer: viewer,
            },
        );
        drop(outputs);
        self.status(
            bank,
            serde_json::json!({"bank":bank,"enabled":false,"sequence":0}),
        )?;
        Ok(config)
    }
    pub fn frame(&self, frame: Frame) -> Result<(), String> {
        let mut outputs = self.0.lock().unwrap();
        let output = outputs
            .get_mut(&frame.output)
            .ok_or("scanout output is not configured")?;
        // Artifact names stay unique across checkpoint restores and old files.
        // This number never enters the model or its checkpoint.
        loop {
            output.next_archive = output
                .next_archive
                .checked_add(1)
                .ok_or("frame archive number overflow")?;
            if !output
                .directory
                .join(format!(
                    "bank{}-{:06}.png",
                    frame.output, output.next_archive
                ))
                .exists()
            {
                break;
            }
        }
        output.writer.submit(crate::png_writer::Frame {
            number: output.next_archive,
            sequence: frame.sequence,
            width: frame.width,
            height: frame.height,
            rgba: frame.rgba,
        })
    }
    pub fn status(&self, bank: usize, value: Value) -> Result<(), String> {
        let mut outputs = self.0.lock().unwrap();
        let output = outputs
            .get_mut(&bank)
            .ok_or("display status output is not configured")?;
        let value = value.to_string();
        if value == output.status {
            return Ok(());
        }
        std::fs::create_dir_all(&output.directory).map_err(|e| e.to_string())?;
        let temporary = output.directory.join(format!(".bank{bank}-status.tmp"));
        std::fs::write(&temporary, &value).map_err(|e| e.to_string())?;
        std::fs::rename(
            temporary,
            output.directory.join(format!("bank{bank}-status.json")),
        )
        .map_err(|e| e.to_string())?;
        output.status = value;
        Ok(())
    }
    pub fn finish(&self) -> Result<(), String> {
        for output in self.0.lock().unwrap().values() {
            output.writer.flush()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn archival_collisions_and_restored_sequences_are_host_only() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/test-output");
        let directory = root.join(format!("display-publication-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("bank0-000001.png"), b"preserved").unwrap();
        let outputs = Outputs::default();
        let options = toml::Table::from_iter([(
            "frame_dir".into(),
            directory.to_string_lossy().to_string().into(),
        )]);
        outputs.configure(0xff990000, &options, None).unwrap();
        let rgba = vec![127; 130 * 130 * 4];
        for _ in 0..2 {
            outputs
                .frame(Frame {
                    output: 0,
                    sequence: 1,
                    width: 130,
                    height: 130,
                    rgba: rgba.clone(),
                })
                .unwrap();
        }
        outputs.finish().unwrap();
        assert_eq!(
            std::fs::read(directory.join("bank0-000001.png")).unwrap(),
            b"preserved"
        );
        let expected = crate::frontend::rgba_png(130, 130, &rgba).unwrap();
        let publication: Value = serde_json::from_slice(
            &std::fs::read(directory.join("bank0-publication.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(publication["sequence"], 1);
        assert!(publication["frames"].as_u64().unwrap() >= 3);
        assert_eq!(
            std::fs::read(directory.join("latest-bank0.png")).unwrap(),
            expected
        );
        std::fs::remove_dir_all(directory).unwrap();
    }
}
