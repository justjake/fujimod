//! Shared native frame publication, status and screenshots.
use super::png_writer;
use fujimod_emulator_devices::graphics::scanout::Frame;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

#[derive(Clone, Default)]
pub struct Outputs(Arc<Mutex<BTreeMap<usize, Output>>>);
struct Output {
    directory: PathBuf,
    writer: png_writer::Writer,
    next_archive: u64,
    status: String,
}
impl Outputs {
    /// Publish display bank `bank` into `directory`.
    pub fn add(&self, bank: usize, directory: PathBuf) -> Result<(), String> {
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
            },
        );
        drop(outputs);
        self.status(
            bank,
            serde_json::json!({"bank":bank,"enabled":false,"sequence":0}),
        )?;
        Ok(())
    }

    /// Flush all frames and copy the latest capture of the first active output.
    pub fn screenshot(&self, path: &Path) -> Result<(), String> {
        self.finish()?;
        for (&bank, output) in self.0.lock().unwrap().iter() {
            let frame = output.directory.join(format!("latest-bank{bank}.png"));
            if output.next_archive > 0 && frame.is_file() {
                std::fs::copy(&frame, path)
                    .map_err(|error| format!("{}: {error}", path.display()))?;
                return Ok(());
            }
        }
        Err("no active scanout published a frame".into())
    }

    /// Publish the EXR II LCD source captured from its own scanout registers.
    pub fn capture_rec(
        &self,
        session: &fujimod_emulator_session::Session,
        instance: usize,
        sequence: u64,
    ) -> Result<(), String> {
        let Some(frame) = session.scanout("scanout", instance, true, false)? else {
            return Ok(());
        };
        let Some(rgb) = frame.rgb else {
            return Ok(());
        };
        self.frame(Frame {
            output: 0,
            sequence,
            width: u32::try_from(frame.width).map_err(|_| "frame width exceeds u32")?,
            height: u32::try_from(frame.height).map_err(|_| "frame height exceeds u32")?,
            rgba: rgb
                .as_chunks::<3>()
                .0
                .iter()
                .flat_map(|rgb| [rgb[0], rgb[1], rgb[2], 255])
                .collect(),
        })?;
        self.status(
            0,
            serde_json::json!({"bank":0,"enabled":true,"sequence":sequence}),
        )
    }

    /// Publish each newly observed output sequence through the shared frame writer.
    pub fn capture(
        &self,
        directory: &Path,
        frames: impl IntoIterator<Item = Frame>,
        sequences: &mut BTreeMap<usize, u64>,
    ) -> Result<(), String> {
        for frame in frames {
            let (bank, sequence) = (frame.output, frame.sequence);
            if sequences.get(&bank) == Some(&sequence) {
                continue;
            }
            if !self.0.lock().unwrap().contains_key(&bank) {
                self.add(bank, directory.to_owned())?;
            }
            self.frame(frame)?;
            self.status(
                bank,
                serde_json::json!({"bank":bank,"enabled":true,"sequence":sequence}),
            )?;
            sequences.insert(bank, sequence);
        }
        Ok(())
    }

    pub fn write_screenshot(path: &Path, frame: &Frame) -> Result<(), String> {
        let bytes = png_writer::rgba_png(frame.width, frame.height, &frame.rgba)?;
        std::fs::write(path, bytes).map_err(|error| format!("{}: {error}", path.display()))
    }

    pub fn frame(&self, frame: Frame) -> Result<u64, String> {
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
        })?;
        Ok(output.next_archive)
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
        outputs.add(0, directory.clone()).unwrap();
        let rgba = vec![127; 130 * 130 * 4];
        let archive_sequences = (0..2)
            .map(|_| {
                outputs
                    .frame(Frame {
                        output: 0,
                        sequence: 1,
                        width: 130,
                        height: 130,
                        rgba: rgba.clone(),
                    })
                    .unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(archive_sequences, [2, 3]);
        outputs.finish().unwrap();
        assert_eq!(
            std::fs::read(directory.join("bank0-000001.png")).unwrap(),
            b"preserved"
        );
        let expected = png_writer::rgba_png(130, 130, &rgba).unwrap();
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
