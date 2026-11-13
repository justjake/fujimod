//! Path-addressed SD card file commands over [`CameraFs`], for hosts that
//! cannot mount it.
use crate::{CameraFs, Device, IO_BYTES, Node, Result, SD};
use sha2::{Digest, Sha256};

/// `DCIM/100_FUJI/DSCF*.JPG`, with `/` or `\` separators and an optional `A:\`
/// prefix. Only the last component may hold `*` or `?`.
fn components(path: &str) -> Vec<&str> {
    let path = path.strip_prefix("A:").unwrap_or(path);
    path.split(['/', '\\']).filter(|c| !c.is_empty()).collect()
}

fn matches(pattern: &[u8], name: &[u8]) -> bool {
    match (pattern.split_first(), name.split_first()) {
        (None, None) => true,
        (Some((b'*', rest)), _) => {
            matches(rest, name) || !name.is_empty() && matches(pattern, &name[1..])
        }
        (Some((p, rest)), Some((n, tail))) => {
            (*p == b'?' || p.eq_ignore_ascii_case(n)) && matches(rest, tail)
        }
        _ => false,
    }
}

fn folder<D: Device>(fs: &mut CameraFs<D>, parts: &[&str]) -> Result<u64> {
    let mut id = SD;
    for part in parts {
        let node = fs.lookup(id, part)?;
        if !node.directory {
            return Err(libc::ENOTDIR);
        }
        id = node.id;
    }
    Ok(id)
}

/// Files and folders matching `path`, in the card's directory order.
pub fn list<D: Device>(fs: &mut CameraFs<D>, path: &str) -> Result<Vec<Node>> {
    let parts = components(path);
    let (last, parents) = parts.split_last().ok_or(libc::EINVAL)?;
    let parent = folder(fs, parents)?;
    if !last.contains(['*', '?']) {
        return Ok(vec![fs.lookup(parent, last)?]);
    }
    Ok(fs
        .directory(parent, false)?
        .1
        .into_iter()
        .filter(|n| matches(last.as_bytes(), n.name.as_bytes()))
        .collect())
}

/// The first `limit` bytes of a file (all of it without one).
pub fn get<D: Device>(fs: &mut CameraFs<D>, node: &Node, limit: Option<u64>) -> Result<Vec<u8>> {
    let size = limit.map_or(node.size, |l| l.min(node.size));
    let mut data = Vec::with_capacity(size as usize);
    while (data.len() as u64) < size {
        let chunk = fs.read(
            node.id,
            data.len() as u64,
            IO_BYTES.min((size - data.len() as u64) as usize),
        )?;
        if chunk.is_empty() {
            return Err(libc::EIO);
        }
        data.extend(chunk);
    }
    Ok(data)
}

/// Writes `data` as the whole file at `path`, creating it when missing and
/// shrinking it when it was longer, then reads it back from the card and
/// returns its SHA-256. An existing file is rewritten in place.
pub fn put<D: Device>(fs: &mut CameraFs<D>, path: &str, data: &[u8]) -> Result<String> {
    let parts = components(path);
    let (name, parents) = parts.split_last().ok_or(libc::EINVAL)?;
    if name.contains(['*', '?']) {
        return Err(libc::EINVAL);
    }
    let parent = folder(fs, parents)?;
    let node = match fs.lookup(parent, name) {
        Ok(node) => node,
        Err(libc::ENOENT) => fs.create(parent, name, false)?,
        Err(error) => return Err(error),
    };
    if node.directory {
        return Err(libc::EISDIR);
    }
    for (index, chunk) in data.chunks(IO_BYTES).enumerate() {
        fs.write(node.id, (index * IO_BYTES) as u64, chunk)?;
    }
    fs.truncate(node.id, data.len() as u64)?;
    fs.listings.remove(&parent);
    let written = fs.lookup(parent, name)?;
    let readback = get(fs, &written, None)?;
    let expected = format!("{:x}", Sha256::digest(data));
    if written.size != data.len() as u64 || format!("{:x}", Sha256::digest(&readback)) != expected {
        return Err(libc::EIO);
    }
    Ok(expected)
}
