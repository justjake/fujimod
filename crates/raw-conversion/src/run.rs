//! Runs a job file against a camera in USB RAW CONV. mode and records each
//! result in `manifest.jsonl`, one line per finished job, so an interrupted
//! run resumes where it stopped.
use crate::profile::{Field, Profile, Settings};
use crate::ptp::{self, DeviceInfo, Pipe, Session};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Fujifilm vendor operations and properties (README.md, Protocol).
pub const SEND_OBJECT_INFO: u16 = 0x900c;
pub const SEND_OBJECT: u16 = 0x900d;
pub const START_RAW_CONVERSION: u32 = 0xd183;
pub const RAW_CONVERSION_PROFILE: u32 = 0xd185;
const RAF_FORMAT: u16 = 0xf802;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobFile {
    /// Settings every job starts from.
    pub base: Settings,
    pub jobs: Vec<Job>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Job {
    /// Also the output file name: lowercase letters, digits, `-` and `+`.
    pub id: String,
    /// Which RAF, by the name given with `--raf NAME=PATH`.
    pub raf: String,
    pub settings: Settings,
}

impl JobFile {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let file: JobFile =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut ids = BTreeSet::new();
        for job in &file.jobs {
            if job.id.is_empty()
                || !job
                    .id
                    .chars()
                    .all(|c| matches!(c, 'a'..='z' | '0'..='9' | '-' | '+'))
            {
                return Err(format!("job id {:?} is not a plain file name", job.id));
            }
            if !ids.insert(job.id.as_str()) {
                return Err(format!("job id {} repeats", job.id));
            }
            let merged = file.settings(job);
            if merged.film.is_none() {
                return Err(format!("{}: no film", job.id));
            }
            merged.words().map_err(|e| format!("{}: {e}", job.id))?;
        }
        Ok(file)
    }
    pub fn settings(&self, job: &Job) -> Settings {
        self.base.overlay(&job.settings)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Camera {
    pub manufacturer: String,
    pub model: String,
    pub firmware: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Adjusted {
    pub field: String,
    pub sent: u32,
    pub camera: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Output {
    pub file: String,
    pub sha256: String,
    pub bytes: usize,
    pub width: u16,
    pub height: u16,
    pub maker_note: BTreeMap<String, i64>,
    /// Why the size and MakerNote could not be read; the file is still saved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inspect_error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub raf: String,
    pub raf_file: String,
    pub raf_sha256: String,
    pub settings: serde_json::Value,
    pub camera: Camera,
    /// `converted`, or `unsupported` when the body lacks or refused a setting
    /// the job names; nothing is converted then.
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Fields the body's profile does not have.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing: Vec<String>,
    /// Words the body changed after accepting the profile.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub adjusted: Vec<Adjusted>,
    /// Profile files under `profiles/`: as reported after upload, as sent,
    /// and as read back when that differs from what was sent.
    pub profiles: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<Output>,
    pub finished_unix: u64,
}

pub struct Options {
    pub out: PathBuf,
    pub rafs: BTreeMap<String, PathBuf>,
    /// Upload the RAF again before every job instead of once per RAF.
    pub upload_each: bool,
    pub conversion_timeout: Duration,
}

/// Jobs already finished in `out`, with a converted file that still matches.
pub fn finished(out: &Path) -> BTreeSet<String> {
    let path = out.join("manifest.jsonl");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return BTreeSet::new();
    };
    let mut done = BTreeSet::new();
    for line in text.lines() {
        // A line torn by an interruption names a job that simply runs again.
        let Ok(entry) = serde_json::from_str::<Entry>(line) else {
            continue;
        };
        // Outputs are renamed into place whole, so a matching size means intact.
        let intact = match &entry.output {
            Some(o) => {
                std::fs::metadata(out.join(&o.file)).is_ok_and(|m| m.len() == o.bytes as u64)
            }
            None => true,
        };
        if intact {
            done.insert(entry.id);
        }
    }
    done
}

/// One line per job, in order, with the D185 words it writes.
pub fn describe(
    file: &JobFile,
    options: &Options,
    done: &BTreeSet<String>,
) -> Result<Vec<String>, String> {
    let mut lines = Vec::new();
    for job in &file.jobs {
        let words = file.settings(job).words()?;
        let words: Vec<String> = words
            .iter()
            .map(|(f, v)| format!("{}={:#x}", *f as usize, v))
            .collect();
        let raf = options
            .rafs
            .get(&job.raf)
            .map_or("unassigned".into(), |p| p.display().to_string());
        let state = if done.contains(&job.id) {
            "done"
        } else {
            "pending"
        };
        lines.push(format!(
            "{state:7} {:28} {}={raf} {} [{}]",
            job.id,
            job.raf,
            serde_json::to_string(&job.settings).unwrap(),
            words.join(" ")
        ));
    }
    Ok(lines)
}

pub async fn identify<P: Pipe>(session: &mut Session<P>) -> Result<Camera, String> {
    let info = DeviceInfo::parse(&call(session, ptp::GET_DEVICE_INFO, &[], None).await?.data)
        .map_err(|e| e.to_string())?;
    let camera = Camera {
        manufacturer: info.manufacturer.clone(),
        model: info.model.clone(),
        firmware: info.device_version.clone(),
    };
    for property in [START_RAW_CONVERSION, RAW_CONVERSION_PROFILE] {
        if !info.properties.contains(&(property as u16)) {
            return Err(format!(
                "{} does not offer property {property:#06x}; set CONNECTION SETTING > \
                 CONNECTION MODE to USB RAW CONV./BACKUP RESTORE",
                camera.model
            ));
        }
    }
    Ok(camera)
}

/// Converts every job not yet in the manifest.
pub async fn execute<P: Pipe>(
    session: &mut Session<P>,
    file: &JobFile,
    options: &Options,
    mut log: impl FnMut(&str),
) -> Result<(), String> {
    let done = finished(&options.out);
    for job in &file.jobs {
        if !done.contains(&job.id) && !options.rafs.contains_key(&job.raf) {
            return Err(format!("{}: no --raf {}=PATH", job.id, job.raf));
        }
    }
    std::fs::create_dir_all(options.out.join("profiles")).map_err(|e| e.to_string())?;
    let camera = identify(session).await?;
    log(&format!(
        "{} {} firmware {}",
        camera.manufacturer, camera.model, camera.firmware
    ));

    let mut loaded: Option<(String, Profile, String)> = None; // RAF name, as-shot profile, its file
    let mut hashes = BTreeMap::new();
    for job in file.jobs.iter().filter(|j| !done.contains(&j.id)) {
        let path = &options.rafs[&job.raf];
        if options.upload_each || loaded.as_ref().is_none_or(|(name, ..)| *name != job.raf) {
            let raf = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
            let raf_sha256 = hex(&Sha256::digest(&raf));
            hashes.insert(job.raf.clone(), raf_sha256.clone());
            log(&format!(
                "uploading {} ({} bytes)",
                path.display(),
                raf.len()
            ));
            upload(session, &raf).await?;
            let as_shot = get_profile(session).await?;
            let name = format!("profiles/{}-{}.as-shot.d185", job.raf, &raf_sha256[..12]);
            std::fs::write(options.out.join(&name), &as_shot.0).map_err(|e| e.to_string())?;
            loaded = Some((job.raf.clone(), as_shot, name));
        }
        let (_, as_shot, as_shot_file) = loaded.as_ref().unwrap();
        let raf_sha256 = hashes[&job.raf].clone();
        let settings = file.settings(job);
        let explicit = job_fields(&job.settings, settings.film);
        let (sent, missing) = as_shot.patch(&settings.words()?);
        let sent_file = format!("profiles/{}.sent.d185", job.id);
        let mut entry = Entry {
            id: job.id.clone(),
            raf: job.raf.clone(),
            raf_file: path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into(),
            raf_sha256,
            settings: serde_json::to_value(&settings).unwrap(),
            camera: camera.clone(),
            status: "converted".into(),
            reason: None,
            missing: missing.iter().map(field_name).collect(),
            adjusted: Vec::new(),
            profiles: BTreeMap::from([("as_shot".into(), as_shot_file.clone())]),
            output: None,
            finished_unix: 0,
        };
        let lacking: Vec<_> = missing.iter().filter(|f| explicit.contains(f)).collect();
        if !lacking.is_empty() {
            entry.status = "unsupported".into();
            entry.reason = Some(format!("profile has no {}", names(&lacking)));
            log(&format!(
                "{}: skipped, {}",
                job.id,
                entry.reason.as_ref().unwrap()
            ));
            record(&options.out, entry)?;
            continue;
        }
        std::fs::write(options.out.join(&sent_file), &sent.0).map_err(|e| e.to_string())?;
        entry.profiles.insert("sent".into(), sent_file);
        match session
            .call(
                ptp::SET_DEVICE_PROP_VALUE,
                &[RAW_CONVERSION_PROFILE],
                Some(&sent.0),
            )
            .await
        {
            Err(ptp::Error::Response { code, .. }) => {
                entry.status = "unsupported".into();
                entry.reason = Some(format!("camera refused the profile with {code:#06x}"));
                log(&format!(
                    "{}: skipped, {}",
                    job.id,
                    entry.reason.as_ref().unwrap()
                ));
                record(&options.out, entry)?;
                continue;
            }
            other => other.map_err(|e| e.to_string())?,
        };
        let readback = get_profile(session).await?;
        if readback != sent {
            let name = format!("profiles/{}.camera.d185", job.id);
            std::fs::write(options.out.join(&name), &readback.0).map_err(|e| e.to_string())?;
            entry.profiles.insert("camera".into(), name);
        }
        for (field, value) in settings.words()? {
            match readback.get(field) {
                Some(camera) if camera != value && !missing.contains(&field) => {
                    entry.adjusted.push(Adjusted {
                        field: field_name(&field),
                        sent: value,
                        camera,
                    })
                }
                _ => {}
            }
        }
        let refused: Vec<String> = entry
            .adjusted
            .iter()
            .filter(|a| explicit.iter().any(|f| field_name(f) == a.field))
            .map(|a| a.field.clone())
            .collect();
        if !refused.is_empty() {
            entry.status = "unsupported".into();
            entry.reason = Some(format!("camera changed {}", refused.join(", ")));
            log(&format!(
                "{}: skipped, {}",
                job.id,
                entry.reason.as_ref().unwrap()
            ));
            record(&options.out, entry)?;
            continue;
        }
        let jpeg = convert(session, options.conversion_timeout).await?;
        // The camera no longer holds the image: save it before reading it.
        let name = format!("{}.jpg", job.id);
        let partial = options.out.join(format!("{name}.partial"));
        std::fs::write(&partial, &jpeg).map_err(|e| e.to_string())?;
        std::fs::rename(&partial, options.out.join(&name)).map_err(|e| e.to_string())?;
        let (summary, inspect_error) = match crate::jpeg::inspect(&jpeg) {
            Ok(s) => (s, None),
            Err(e) => (Default::default(), Some(e)),
        };
        log(&format!(
            "{}: {}x{} {} bytes",
            job.id,
            summary.width,
            summary.height,
            jpeg.len()
        ));
        entry.output = Some(Output {
            file: name,
            sha256: hex(&Sha256::digest(&jpeg)),
            bytes: jpeg.len(),
            width: summary.width,
            height: summary.height,
            maker_note: summary.maker_note,
            inspect_error,
        });
        record(&options.out, entry)?;
    }
    Ok(())
}

/// The profile fields the job itself names, as opposed to the base.
fn job_fields(job: &Settings, film: Option<crate::profile::Film>) -> Vec<Field> {
    let named = Settings {
        film,
        ..job.clone()
    };
    let mut fields: Vec<Field> = named
        .words()
        .unwrap_or_default()
        .into_iter()
        .map(|(f, _)| f)
        .collect();
    fields.retain(|f| *f != Field::FileType && (*f != Field::FilmSimulation || job.film.is_some()));
    if job.monochrome_warm_cool.is_none() {
        fields.retain(|f| *f != Field::MonochromeWarmCool);
    }
    if job.monochrome_magenta_green.is_none() {
        fields.retain(|f| *f != Field::MonochromeMagentaGreen);
    }
    fields
}

fn field_name(f: &Field) -> String {
    serde_json::to_value(f)
        .unwrap()
        .as_str()
        .unwrap()
        .to_owned()
}
fn names(fields: &[&Field]) -> String {
    fields
        .iter()
        .map(|f| field_name(f))
        .collect::<Vec<_>>()
        .join(", ")
}

async fn call<P: Pipe>(
    s: &mut Session<P>,
    code: u16,
    params: &[u32],
    send: Option<&[u8]>,
) -> Result<ptp::Reply, String> {
    s.call(code, params, send).await.map_err(|e| e.to_string())
}

/// Sends a RAF the way libfuji's `fuji_send_raf` does: vendor ObjectInfo, then the bytes.
async fn upload<P: Pipe>(session: &mut Session<P>, raf: &[u8]) -> Result<(), String> {
    let size = u32::try_from(raf.len()).map_err(|_| "RAF over 4 GiB")?;
    let mut info = Vec::new();
    info.extend_from_slice(&0u32.to_le_bytes()); // StorageID
    info.extend_from_slice(&RAF_FORMAT.to_le_bytes());
    info.extend_from_slice(&0u16.to_le_bytes()); // ProtectionStatus
    info.extend_from_slice(&size.to_le_bytes());
    info.extend_from_slice(&0u16.to_le_bytes()); // ThumbFormat
    info.extend_from_slice(&[0; 4 * 7]); // thumb size/width/height, image width/height/depth, parent
    info.extend_from_slice(&0u16.to_le_bytes()); // AssociationType
    info.extend_from_slice(&[0; 8]); // AssociationDesc, SequenceNumber
    ptp::write_string(&mut info, "FUP_FILE.dat");
    for _ in 0..3 {
        ptp::write_string(&mut info, ""); // CaptureDate, ModificationDate, Keywords
    }
    call(session, SEND_OBJECT_INFO, &[0, 0, 0], Some(&info)).await?;
    call(session, SEND_OBJECT, &[], Some(raf)).await?;
    Ok(())
}

async fn get_profile<P: Pipe>(session: &mut Session<P>) -> Result<Profile, String> {
    Ok(Profile(
        call(
            session,
            ptp::GET_DEVICE_PROP_VALUE,
            &[RAW_CONVERSION_PROFILE],
            None,
        )
        .await?
        .data,
    ))
}

async fn handles<P: Pipe>(session: &mut Session<P>) -> Result<Vec<u32>, String> {
    let data = call(session, ptp::GET_OBJECT_HANDLES, &[0xffff_ffff, 0, 0], None)
        .await?
        .data;
    ptp::Reader::new(&data)
        .u32_array()
        .map_err(|e| e.to_string())
}

/// Starts the conversion, waits for a new object, downloads and deletes it.
/// Objects that were already present are left alone.
async fn convert<P: Pipe>(session: &mut Session<P>, timeout: Duration) -> Result<Vec<u8>, String> {
    let before: BTreeSet<u32> = handles(session).await?.into_iter().collect();
    call(
        session,
        ptp::SET_DEVICE_PROP_VALUE,
        &[START_RAW_CONVERSION],
        Some(&0u16.to_le_bytes()),
    )
    .await?;
    let poll = Duration::from_millis(500);
    let deadline = std::time::Instant::now() + timeout;
    let handle = loop {
        if let Some(h) = handles(session)
            .await?
            .into_iter()
            .find(|h| !before.contains(h))
        {
            break h;
        }
        if std::time::Instant::now() >= deadline {
            return Err(format!("no converted image after {timeout:?}"));
        }
        futures_timer::Delay::new(poll).await;
    };
    let jpeg = call(session, ptp::GET_OBJECT, &[handle], None).await?.data;
    call(session, ptp::DELETE_OBJECT, &[handle, 0], None).await?;
    Ok(jpeg)
}

fn record(out: &Path, mut entry: Entry) -> Result<(), String> {
    entry.finished_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let path = out.join("manifest.jsonl");
    // End a line torn by an interruption so this entry starts on its own line.
    let torn = std::fs::read(&path).is_ok_and(|t| t.last().is_some_and(|b| *b != b'\n'));
    let mut line = if torn { "\n".to_owned() } else { String::new() };
    line.push_str(&serde_json::to_string(&entry).unwrap());
    line.push('\n');
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| e.to_string())?;
    f.write_all(line.as_bytes())
        .and_then(|_| f.sync_all())
        .map_err(|e| e.to_string())
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_camera::FakeCamera;
    use futures_lite::future::block_on;

    const JOBS: &str = r#"{
      "base": {"white_balance_as_shot": "ON", "dynamic_range": 100, "color_space": "SRGB",
               "image_size": "L_3_2", "image_quality": "FINE", "grain": "OFF", "clarity": 0},
      "jobs": [
        {"id": "provia", "raf": "base", "settings": {"film": "PROVIA"}},
        {"id": "acros-wc+9", "raf": "base", "settings": {"film": "ACROS", "monochrome_warm_cool": 9}},
        {"id": "provia-clarity+5", "raf": "base", "settings": {"film": "PROVIA", "clarity": 5}},
        {"id": "acros-iso3200", "raf": "iso3200", "settings": {"film": "ACROS", "noise_reduction": -4}}
      ]
    }"#;

    struct Setup {
        dir: PathBuf,
        file: JobFile,
        options: Options,
    }
    fn setup(name: &str) -> Setup {
        let dir =
            std::env::temp_dir().join(format!("raw-conversion-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("jobs.json"), JOBS).unwrap();
        let mut rafs = BTreeMap::new();
        for (role, bytes) in [("base", vec![1u8; 5000]), ("iso3200", vec![2u8; 4096])] {
            let path = dir.join(format!("{role}.RAF"));
            std::fs::write(&path, bytes).unwrap();
            rafs.insert(role.to_owned(), path);
        }
        Setup {
            file: JobFile::load(&dir.join("jobs.json")).unwrap(),
            options: Options {
                out: dir.join("out"),
                rafs,
                upload_each: false,
                conversion_timeout: Duration::from_secs(1),
            },
            dir,
        }
    }
    fn run(s: &Setup, camera: FakeCamera) -> (Result<(), String>, FakeCamera) {
        block_on(async {
            let mut session = Session::open(camera).await.unwrap();
            let result = execute(&mut session, &s.file, &s.options, |_| {}).await;
            (
                result,
                session.close().await.unwrap_or_else(|_| panic!("close")),
            )
        })
    }
    fn manifest_lines(s: &Setup) -> usize {
        std::fs::read_to_string(s.options.out.join("manifest.jsonl"))
            .unwrap()
            .lines()
            .count()
    }
    fn manifest(s: &Setup) -> Vec<Entry> {
        std::fs::read_to_string(s.options.out.join("manifest.jsonl"))
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn converts_every_job_once_per_raf_and_records_it() {
        // Four-byte packets put a zero-length packet after every container.
        let s = setup("all");
        let (result, camera) = run(&s, FakeCamera::new(4));
        result.unwrap();
        assert_eq!((camera.uploads, camera.conversions), (2, 4));
        assert!(camera.objects.is_empty());
        let entries = manifest(&s);
        assert_eq!(entries.len(), 4);
        let acros = &entries[1];
        assert_eq!(acros.status, "converted");
        assert_eq!(acros.camera.model, "X-T5");
        assert_eq!(acros.camera.firmware, "1.10");
        let out = acros.output.as_ref().unwrap();
        assert_eq!((out.width, out.height), (6240, 4160));
        assert_eq!(out.maker_note["0x1401"], 12);
        let jpeg = std::fs::read(s.options.out.join(&out.file)).unwrap();
        assert_eq!(hex(&Sha256::digest(&jpeg)), out.sha256);
        let sent = Profile(std::fs::read(s.options.out.join(&acros.profiles["sent"])).unwrap());
        assert_eq!(sent.get(Field::MonochromeWarmCool), Some(90));
        // A color film keeps the camera's own words 22 and 25.
        let provia =
            Profile(std::fs::read(s.options.out.join(&entries[0].profiles["sent"])).unwrap());
        assert_eq!(provia.get(Field::MonochromeWarmCool), Some(0x0902_6000));
        assert_eq!(entries[3].raf_sha256, hex(&Sha256::digest(vec![2u8; 4096])));
        let iso = Profile(std::fs::read(s.options.out.join(&entries[3].profiles["sent"])).unwrap());
        assert_eq!(iso.get(Field::NoiseReduction), Some(0x8000));
        std::fs::remove_dir_all(&s.dir).unwrap();
    }

    #[test]
    fn resumes_after_an_interrupted_conversion() {
        let s = setup("resume");
        let mut camera = FakeCamera::new(512);
        camera.fail_conversion = Some(2);
        let result = block_on(async {
            let mut session = Session::open(camera).await.unwrap();
            execute(&mut session, &s.file, &s.options, |_| {}).await
        });
        assert!(result.unwrap_err().contains("disconnected"));
        assert_eq!(manifest(&s).len(), 1);
        let (result, camera) = run(&s, FakeCamera::new(512));
        result.unwrap();
        assert_eq!(camera.conversions, 3);
        let ids: Vec<_> = manifest(&s).into_iter().map(|e| e.id).collect();
        assert_eq!(
            ids,
            ["provia", "acros-wc+9", "provia-clarity+5", "acros-iso3200"]
        );
        // A converted file that no longer matches its hash is converted again.
        std::fs::write(s.options.out.join("provia.jpg"), b"x").unwrap();
        assert!(!finished(&s.options.out).contains("provia"));
        std::fs::remove_dir_all(&s.dir).unwrap();
    }

    #[test]
    fn resumes_on_an_open_session_and_after_a_torn_manifest_line() {
        let s = setup("torn");
        let mut camera = FakeCamera::new(512);
        camera.fail_conversion = Some(2);
        let result = block_on(async {
            let mut session = Session::open(camera).await.unwrap();
            execute(&mut session, &s.file, &s.options, |_| {}).await
        });
        assert!(result.is_err());
        let path = s.options.out.join("manifest.jsonl");
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str(r#"{"id":"acros-wc+9","raf"#);
        std::fs::write(&path, text).unwrap();
        // The interrupted run never closed its session.
        let mut camera = FakeCamera::new(512);
        camera.session_open = true;
        let (result, _) = run(&s, camera);
        result.unwrap();
        assert_eq!(manifest_lines(&s), 5);
        assert_eq!(finished(&s.options.out).len(), 4);
        std::fs::remove_dir_all(&s.dir).unwrap();
    }

    #[test]
    fn unreadable_output_is_saved_with_its_error() {
        let s = setup("unreadable");
        let mut camera = FakeCamera::new(512);
        camera.unreadable_output = true;
        let (result, _) = run(&s, camera);
        result.unwrap();
        let entry = &manifest(&s)[0];
        let out = entry.output.as_ref().unwrap();
        assert!(out.inspect_error.is_some());
        assert_eq!(
            std::fs::read(s.options.out.join(&out.file)).unwrap().len(),
            out.bytes
        );
        std::fs::remove_dir_all(&s.dir).unwrap();
    }

    #[test]
    fn bodies_without_a_setting_skip_only_jobs_that_name_it() {
        let s = setup("missing");
        let mut camera = FakeCamera::new(512);
        camera.count = 26; // no CLARITY word
        let (result, camera) = run(&s, camera);
        result.unwrap();
        assert_eq!(camera.conversions, 3);
        let entries = manifest(&s);
        let clarity = entries.iter().find(|e| e.id == "provia-clarity+5").unwrap();
        assert_eq!(clarity.status, "unsupported");
        assert_eq!(clarity.missing, ["clarity"]);
        assert!(clarity.output.is_none());
        // The base's CLARITY 0 is missing too, but recorded, not fatal.
        assert_eq!(entries[0].status, "converted");
        assert_eq!(entries[0].missing, ["clarity"]);
        std::fs::remove_dir_all(&s.dir).unwrap();
    }

    #[test]
    fn refused_or_changed_settings_are_recorded_without_converting() {
        let s = setup("clamp");
        let mut camera = FakeCamera::new(512);
        camera.clamp = Some(Field::NoiseReduction);
        let (result, camera) = run(&s, camera);
        result.unwrap();
        assert_eq!(camera.conversions, 3);
        let iso = manifest(&s)
            .into_iter()
            .find(|e| e.id == "acros-iso3200")
            .unwrap();
        assert_eq!(iso.status, "unsupported");
        assert_eq!(iso.adjusted[0].field, "noise_reduction");
        assert!(iso.profiles.contains_key("camera"));

        std::fs::remove_dir_all(&s.dir).unwrap();
        let s = setup("refuse");
        let mut camera = FakeCamera::new(512);
        camera.refuse_profile = true;
        let (result, camera) = run(&s, camera);
        result.unwrap();
        assert_eq!(camera.conversions, 0);
        assert!(manifest(&s).iter().all(|e| e.status == "unsupported"));
        std::fs::remove_dir_all(&s.dir).unwrap();
    }

    #[test]
    fn objects_already_on_the_camera_are_left_alone() {
        let s = setup("existing");
        let mut camera = FakeCamera::new(512);
        camera.objects.insert(1, vec![0xff, 0xd8]);
        let (result, camera) = run(&s, camera);
        result.unwrap();
        assert_eq!(camera.objects.keys().copied().collect::<Vec<_>>(), [1]);
        std::fs::remove_dir_all(&s.dir).unwrap();
    }

    #[test]
    fn dry_run_lists_jobs_and_their_words() {
        let s = setup("dry");
        let lines = describe(&s.file, &s.options, &BTreeSet::from(["provia".to_owned()])).unwrap();
        assert_eq!(lines.len(), 4);
        assert!(lines[0].starts_with("done"));
        assert!(lines[1].starts_with("pending acros-wc+9"));
        assert!(lines[1].contains("22=0x5a"));
        std::fs::remove_dir_all(&s.dir).unwrap();
    }

    #[test]
    fn job_files_are_checked_before_any_camera_work() {
        let dir = std::env::temp_dir().join(format!("raw-conversion-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (bad, why) in [
            (
                r#"{"base":{},"jobs":[{"id":"a","raf":"r","settings":{}}]}"#,
                "no film",
            ),
            (
                r#"{"base":{},"jobs":[{"id":"A","raf":"r","settings":{"film":"PROVIA"}}]}"#,
                "plain file name",
            ),
            (
                r#"{"base":{},"jobs":[{"id":"a","raf":"r","settings":{"film":"PROVIA","clarity":7}}]}"#,
                "clarity",
            ),
            (
                r#"{"base":{},"jobs":[{"id":"a","raf":"r","settings":{"film":"PROVIA","grainy":1}}]}"#,
                "unknown field",
            ),
        ] {
            std::fs::write(dir.join("j.json"), bad).unwrap();
            let error = JobFile::load(&dir.join("j.json")).unwrap_err();
            assert!(error.contains(why), "{error}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
