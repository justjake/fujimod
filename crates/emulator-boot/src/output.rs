//! Shared native frame publication, status and screenshots.
use super::{frame_viewer::Viewer, png_writer};
use fujimod_emulator_devices::graphics::scanout::Frame;
use serde_json::Value;
use std::{collections::BTreeMap, path::{Path, PathBuf}, sync::{Arc, Mutex}};

#[derive(Clone, Default)]
pub struct Outputs(Arc<Mutex<BTreeMap<usize, Output>>>);
struct Output {
    directory: PathBuf,
    writer: png_writer::Writer,
    next_archive: u64,
    status: String,
    _viewer: Option<Viewer>,
}
impl Outputs {
    pub(crate) fn add(&self, bank: usize, directory: PathBuf, viewer: Option<Viewer>) -> Result<(), String> {
        let mut outputs = self.0.lock().unwrap();
        if outputs.contains_key(&bank) {
            return Err(format!("display output {bank} is already configured"));
        }
        outputs.insert(
            bank,
            Output {
                writer: png_writer::Writer::new(directory.clone(), bank)?,
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
        Ok(())
    }
    /// Register a host output whose model has no device-side output settings.
    pub fn attach(&self, bank: usize, directory: PathBuf, viewer_port: Option<u16>) -> Result<(), String> {
        let viewer = viewer_port.map(|port| {
            Viewer::with_bind(directory.clone(), std::net::Ipv4Addr::LOCALHOST.into(), port, None, None)
        }).transpose()?;
        if let Some(viewer) = &viewer {
            eprintln!("display viewer: {}", viewer.url());
        }
        self.add(bank, directory, viewer)
    }

    /// Flush all frames and copy the latest capture of the first active output.
    pub fn screenshot(&self, path: &Path) -> Result<(), String> {
        self.finish()?;
        for (&bank, output) in self.0.lock().unwrap().iter() {
            let frame = output.directory.join(format!("latest-bank{bank}.png"));
            if frame.is_file() {
                std::fs::copy(&frame, path).map_err(|error| format!("{}: {error}", path.display()))?;
                return Ok(());
            }
        }
        Err("no active scanout published a frame".into())
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
        output.writer.submit(png_writer::Frame {
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
        outputs.add(0, directory.clone(), None).unwrap();
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
        let expected = crate::frame_viewer::rgba_png(130, 130, &rgba).unwrap();
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
