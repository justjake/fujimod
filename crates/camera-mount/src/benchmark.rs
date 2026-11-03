//! End-to-end POSIX reads of the mounted files, with cache suppression.
use fujimod_camera_mount::Region;
use serde_json::{Value, json};
use std::{
    fs::File,
    io,
    os::fd::AsRawFd,
    os::unix::fs::FileExt,
    path::{Path, PathBuf},
    time::Instant,
};

fn read(file: &Path, offset: u64, bytes: u64, piece: usize) -> io::Result<Value> {
    let file = File::open(file)?;
    // F_NOCACHE is per descriptor; it leaves the volume's global policy alone.
    // SAFETY: the descriptor is live and this fcntl takes an integer argument.
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_NOCACHE, 1) } == -1 {
        return Err(io::Error::last_os_error());
    }
    let mut buffer = vec![0u8; piece];
    let mut done = 0;
    let start = Instant::now();
    while done < bytes {
        let n = buffer.len().min((bytes - done) as usize);
        let count = file.read_at(&mut buffer[..n], offset + done)?;
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Short benchmark read",
            ));
        }
        done += count as u64;
        std::hint::black_box(&buffer[..count]);
    }
    let seconds = start.elapsed().as_secs_f64();
    Ok(
        json!({"offset":offset,"bytes":done,"request_bytes":piece,"seconds":seconds,
        "megabytes_per_second":done as f64/seconds/1_000_000.0}),
    )
}
fn candidate(folder: &Path, depth: usize) -> io::Result<Option<PathBuf>> {
    if depth == 0 || !folder.exists() {
        return Ok(None);
    }
    let mut directories = Vec::new();
    for entry in std::fs::read_dir(folder)?.take(10000) {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if metadata.is_file() && metadata.len() >= 8 << 20 {
            return Ok(Some(entry.path()));
        }
        if metadata.is_dir() {
            directories.push(entry.path());
        }
    }
    for path in directories {
        if let Some(path) = candidate(&path, depth - 1)? {
            return Ok(Some(path));
        }
    }
    Ok(None)
}
pub fn run(mount: &Path, regions: &[Region]) -> io::Result<Value> {
    let region = regions
        .iter()
        .filter(|r| r.size >= 128 << 20)
        .max_by_key(|r| r.size)
        .ok_or_else(|| io::Error::other("No sufficiently large readable memory region"))?;
    let mut memory = Vec::new();
    for size in [128 << 10, 1 << 20, 8 << 20, 64 << 20] {
        let result = read(
            &mount.join("memory"),
            u64::from(region.address) + (16 << 20),
            64 << 20,
            size,
        )?;
        eprintln!("{}", json!({"benchmark_layer":"posix","memory":result}));
        memory.push(result);
    }
    let sd = if let Some(path) = candidate(&mount.join("sd"), 5)? {
        let n = std::fs::metadata(&path)?.len().min(64 << 20);
        let mut runs = Vec::new();
        for size in [128 << 10, 1 << 20, 8 << 20] {
            let result = read(&path, 0, n, size)?;
            eprintln!("{}", json!({"benchmark_layer":"posix","sd":result}));
            runs.push(result);
        }
        json!({"path":path.strip_prefix(mount).unwrap_or(&path),"runs":runs})
    } else {
        Value::Null
    };
    Ok(json!({"memory":memory,"sd":sd}))
}

/// Measure the same bounded backend transactions without the OS adapter.
/// Results explicitly identify their layer so they cannot be mistaken for
/// mounted-file throughput.
pub fn usb(
    device: &mut impl fujimod_camera_mount::Device,
    regions: &[Region],
) -> io::Result<Value> {
    fn check<T>(result: Result<T, i32>) -> io::Result<T> {
        result.map_err(io::Error::from_raw_os_error)
    }
    fn measure(
        bytes: u32,
        piece: u32,
        mut read: impl FnMut(u32, u32) -> io::Result<Vec<u8>>,
    ) -> io::Result<Value> {
        let start = Instant::now();
        let mut done = 0;
        while done < bytes {
            let count = piece.min(bytes - done);
            let raw = read(done, count)?;
            if raw.len() != count as usize {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Short USB benchmark read",
                ));
            }
            std::hint::black_box(raw);
            done += count;
        }
        let seconds = start.elapsed().as_secs_f64();
        Ok(json!({"bytes":done,"request_bytes":piece,"seconds":seconds,
            "megabytes_per_second":f64::from(done)/seconds/1_000_000.0}))
    }
    fn candidate(
        device: &mut impl fujimod_camera_mount::Device,
        path: &str,
        depth: usize,
    ) -> io::Result<Option<(String, u32)>> {
        if depth == 0 {
            return Ok(None);
        }
        let entries = check(device.list(path))?;
        let mut directories = Vec::new();
        for entry in entries {
            let child = format!(
                "{}{}{}",
                path,
                if path.ends_with('\\') { "" } else { "\\" },
                entry.name
            );
            if entry.directory {
                if !matches!(entry.name.as_str(), "." | "..") {
                    directories.push(child);
                }
            } else if entry.bytes >= 8 << 20 {
                return Ok(Some((child, entry.bytes)));
            }
        }
        for child in directories {
            if let Some(value) = candidate(device, &child, depth - 1)? {
                return Ok(Some(value));
            }
        }
        Ok(None)
    }
    let region = regions
        .iter()
        .filter(|r| r.size >= 128 << 20)
        .max_by_key(|r| r.size)
        .ok_or_else(|| io::Error::other("No sufficiently large readable memory region"))?;
    let address = u32::try_from(region.address + (16 << 20))
        .map_err(|_| io::Error::other("Memory address exceeds 32 bits"))?;
    let mut memory = Vec::new();
    for size in [128 << 10, 1 << 20, 8 << 20, 64 << 20] {
        let result = measure(64 << 20, size, |offset, count| {
            check(device.memory(address + offset, count))
        })?;
        eprintln!(
            "{}",
            json!({"benchmark_layer":"usb-backend","memory":result})
        );
        memory.push(result);
    }
    let sd = if device.has_card() {
        if let Some((path, bytes)) = candidate(device, "A:\\", 5)? {
            let mut runs = Vec::new();
            for size in [128 << 10, 1 << 20, 8 << 20] {
                let result = measure(bytes.min(64 << 20), size, |offset, count| {
                    check(device.read(&path, offset, count))
                })?;
                eprintln!("{}", json!({"benchmark_layer":"usb-backend","sd":result}));
                runs.push(result);
            }
            json!({"path":path,"runs":runs})
        } else {
            Value::Null
        }
    } else {
        Value::Null
    };
    Ok(json!({"benchmark_layer":"usb-backend","memory":memory,"sd":sd}))
}
