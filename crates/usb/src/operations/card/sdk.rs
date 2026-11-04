//! Existing code only: borrow a stock call request, allocate data RAM, and
//! wait for the stock SDK's own callbacks. No executable bytes are uploaded.
use super::*;
use crate::ff80::{SessionState, commands::diagnostic::DiagnosticTag};
use crate::operations::borrowed::BorrowedQueries;
use fujimod_catalog_card::StockSdk;

const SENTINEL: u32 = 0x713ef11e;
const REQUEST_BYTES: u32 = 36;
const DATA_OFFSET: u32 = 512;
const DATA_RESERVATION: u32 = DATA_OFFSET + 32;

pub(super) struct Sdk<'a, 'p> {
    pub(super) host: Host<'a>,
    p: &'p StockSdk,
    slot: u32,
    table: u32,
    originals: Vec<(u32, Vec<u8>)>,
    base: Option<u32>,
    pool: u32,
    handle: Option<u32>,
    active: Option<(String, u32)>,
    position: Option<u32>,
    pending: bool,
    cursor: bool,
}
impl Sdk<'_, '_> {
    async fn call(&mut self, target: u32, args: &[u32]) -> Result<Vec<u8>> {
        require(args.len() <= 6, "Too many stock call arguments")?;
        let before = self.originals[0].1.clone();
        self.host.check(self.p.request, &before).await?;
        let mut w = vec![0; 9];
        w[0] = target;
        w[1..1 + args.len()].copy_from_slice(args);
        let request = words(&w);
        let outputs = self.host.read(self.p.query_output, 8).await?;
        let mut call = BorrowedQueries::new(
            Address(self.slot),
            Address(self.p.stock_callback),
            Address(self.p.query_output),
            outputs.clone(),
        )?;
        call.call(
            Address(self.p.dispatcher),
            DiagnosticTag::Research(0),
            vec![(0, 0)],
            vec![0; 8],
        )?;
        self.host.write(self.p.request, &request).await?;
        let result = call.invoke(self.host.session, self.host.limits).await;
        if self.host.session.state() == SessionState::Ready {
            let current = self.host.read(self.p.request, REQUEST_BYTES).await?;
            let mut consumed = request.clone();
            consumed[..4].fill(0);
            require(
                current == request || current == consumed,
                "Stock request ownership changed; preserve RAM",
            )?;
            self.host.write(self.p.request, &before).await?;
            self.host.check(self.p.request, &before).await?;
            self.host
                .check(self.slot, &self.p.stock_callback.to_le_bytes())
                .await?;
            self.host.check(self.p.query_output, &outputs).await?;
        }
        Ok(result?.remove(0).1)
    }
    fn base(&self) -> Result<u32> {
        self.base.ok_or_else(|| fail("No allocated card buffer"))
    }
    fn buffer(&self) -> Result<u32> {
        Ok((self.base()? + DATA_OFFSET + 31) & !31)
    }

    async fn read_buffer(&mut self, address: u32, size: u32) -> Result<Vec<u8>> {
        // Borrowing E1's publication pair costs eight control exchanges. A
        // small CPU read is cheaper and also avoids padding a tiny transfer.
        let cpu = self
            .host
            .limits
            .read
            .get()
            .min(self.host.session.info().advertised_packet_bytes.get());
        if size <= cpu.saturating_mul(8) {
            return self.host.read(address, size).await;
        }
        let Some(source) = self.p.bulk_source else {
            return self.host.read(address, size).await;
        };
        let padded = size.checked_add(31).ok_or(Error::AddressOverflow)? & !31;
        require(
            address % 32 == 0 && padded <= self.p.chunk_bytes,
            "Invalid owned E1 range",
        )?;
        let original = self.host.read(source, 8).await?;
        let control = self.host.read(address, size.min(256)).await?;
        let selected = words(&[address, padded]);
        self.host.write(source, &selected).await?;
        self.host.check(source, &selected).await?;
        let result = self
            .host
            .session
            .read_data(0, NonZeroU32::new(padded).ok_or(Error::InvalidLength)?)
            .await;
        if self.host.session.state() == SessionState::Ready {
            self.host.check(source, &selected).await?;
            self.host.write(source, &original).await?;
            self.host.check(source, &original).await?;
        }
        let mut raw = result?;
        require(
            raw.starts_with(&control),
            "E1 file bytes differ from the CPU control read",
        )?;
        raw.truncate(size as usize);
        Ok(raw)
    }

    async fn write_buffer(&mut self, address: u32, raw: &[u8]) -> Result<()> {
        if self
            .host
            .io
            .as_ref()
            .is_some_and(|io| io.admits_write(address, raw.len() as u32, self.p.chunk_bytes))
        {
            // Discard dirty cached aliases before DMA. Afterward no CPU access
            // touches the payload until the existing post-transfer sync.
            self.call(self.p.cache, &[]).await?;
        }
        self.host
            .write_owned(address, raw, self.p.chunk_bytes)
            .await
    }

    async fn allocate(&mut self) -> Result<()> {
        self.host.put(self.p.error, SENTINEL).await?;
        let result = self
            .call(
                self.p.allocate,
                &[
                    self.pool,
                    self.p.chunk_bytes + DATA_RESERVATION,
                    self.p.query_output,
                    0,
                ],
            )
            .await?;
        let base = u32_at(&result, 0);
        require(
            self.host.word(self.p.error).await? == SENTINEL,
            "RAM allocation failed",
        )?;
        require(
            base >= self.p.pool_begin + 4
                && u64::from(base) + u64::from(self.p.chunk_bytes) + u64::from(DATA_RESERVATION)
                    <= u64::from(self.p.pool_begin) + u64::from(self.p.pool_bytes)
                && base % 4 == 0,
            "Allocation outside the located RAM pool",
        )?;
        self.base = Some(base);
        Ok(())
    }
    async fn start(&mut self, target: u32, args: &[u32]) -> Result<()> {
        require(!self.pending, "A card callback is outstanding")?;
        self.host.put(self.p.status, SENTINEL).await?;
        self.host.put(self.p.value, SENTINEL).await?;
        self.pending = true;
        self.call(target, args).await?;
        Ok(())
    }
    async fn completion(&mut self, value_required: bool) -> Result<(u32, u32)> {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let status = self.host.word(self.p.status).await?;
            if status != SENTINEL {
                let value = self.host.word(self.p.value).await?;
                if !value_required || status != 0 || value != SENTINEL {
                    self.pending = false;
                    return Ok((status, value));
                }
            }
            if Instant::now() >= deadline {
                self.host.session.invalidate_completion();
                return Err(fail("SDK callback completion unknown; retain RAM"));
            }
            futures_timer::Delay::new(Duration::from_millis(10)).await;
        }
    }
    async fn io(&mut self, target: u32, args: &[u32], value_required: bool) -> Result<(u32, u32)> {
        let mut arguments = vec![self.p.completion];
        arguments.extend(args);
        self.start(target, &arguments).await?;
        self.completion(value_required).await
    }
    async fn path(&mut self, path: &str) -> Result<()> {
        self.host
            .write(self.base()?, &[path.as_bytes(), b"\0"].concat())
            .await
    }
    async fn open(&mut self, path: &str, mode: u32) -> Result<()> {
        if self
            .active
            .as_ref()
            .is_some_and(|(p, m)| p == path && *m == mode)
            && self.handle.is_some()
        {
            return Ok(());
        }
        self.close().await?;
        self.path(path).await?;
        let (status, handle) = self.io(self.p.open, &[self.base()?, mode], true).await?;
        require(status == 0, &format!("Card open failed: {status:#x}"))?;
        require(
            handle > 0 && handle <= 0x0fffffff,
            "Invalid SD0 file handle",
        )?;
        self.handle = Some(handle);
        self.active = Some((path.to_owned(), mode));
        self.position = if mode == 2 { None } else { Some(0) };
        Ok(())
    }
    async fn seek(&mut self, offset: u32) -> Result<()> {
        if self.position == Some(offset) {
            return Ok(());
        }
        let (status, _) = self
            .io(
                self.p.seek,
                &[self.handle.expect("opened"), offset, 0, 0],
                false,
            )
            .await?;
        require(status == 0, "SDK seek failed")?;
        self.position = Some(offset);
        Ok(())
    }
    pub(super) fn idle(&self) -> bool {
        !self.pending && !self.cursor
    }

    pub(super) async fn close(&mut self) -> Result<()> {
        if let Some(handle) = self.handle {
            let (status, _) = self.io(self.p.close, &[handle, 0], false).await?;
            require(status == 0, &format!("Card close failed: {status:#x}"))?;
            self.handle = None;
            self.active = None;
            self.position = None;
        }
        Ok(())
    }
    async fn scan(&mut self, pattern: &str, directories: bool) -> Result<Vec<DirectoryEntry>> {
        self.close().await?;
        require(
            !self.cursor && self.handle.is_none(),
            "SDK directory cursor already held",
        )?;
        let (folder, pattern) = pattern.rsplit_once('\\').ok_or(Error::InvalidToken)?;
        require(
            folder.len() < 32 && pattern.len() <= 12,
            "SDK directory pattern too long",
        )?;
        let base = self.base()?;
        let mut payload = vec![0; 60];
        payload[..8].copy_from_slice(&words(&[self.p.completion, base + 256]));
        payload[8..8 + folder.len()].copy_from_slice(folder.as_bytes());
        payload[40..40 + pattern.len()].copy_from_slice(pattern.as_bytes());
        payload[56..60].copy_from_slice(&0u32.to_le_bytes());
        self.host.write(base + 64, &payload).await?;
        let mut entries = Vec::new();
        let mut code = if directories {
            self.p.directory_first_code
        } else {
            self.p.list_first_code
        };
        let mut size = 60;
        self.cursor = true;
        loop {
            // Persistent command and payload survive the asynchronous workers.
            let receiver = self.host.read(self.p.receiver_cell, 1).await?[0];
            require(receiver != 0, "SDK task is unavailable")?;
            let command = words(&[u32::from(code) | u32::from(receiver) << 24, size, base + 64]);
            self.host.write(base + 128, &command).await?;
            self.start(self.p.send, &[base + 128]).await?;
            let (status, _) = self.completion(false).await?;
            if status != 0 {
                require(
                    status == self.p.end_of_directory_status,
                    &format!("Directory scan failed: {status:#x}"),
                )?;
                self.cursor = false;
                return Ok(entries);
            }
            let raw = self
                .host
                .read(base + 256, self.p.directory_context_bytes)
                .await?;
            let attributes = raw[self.p.directory_attribute_offset as usize];
            if attributes & 8 == 0 && (directories || attributes & 0x10 == 0) {
                let end = raw[..48].iter().position(|&v| v == 0).unwrap_or(48);
                let tail = raw[..end]
                    .rsplit(|&v| v == b'\\' || v == b':')
                    .next()
                    .unwrap_or(&[]);
                let mut name = Vec::new();
                let dotted = tail.contains(&b'.');
                for (i, &byte) in tail.iter().enumerate() {
                    if !dotted && i == 8 {
                        name.push(b'.')
                    }
                    if byte != b' ' {
                        name.push(byte)
                    }
                }
                if name.last() == Some(&b'.') {
                    name.pop();
                }
                require(
                    !name.is_empty()
                        && name.len() <= 12
                        && name
                            .iter()
                            .all(|b| b.is_ascii_graphic() && !b"\\/:".contains(b)),
                    "Invalid SDK directory name",
                )?;
                let bytes = directory_size(
                    &raw,
                    self.p.directory_size_offset as usize,
                    self.p.directory_size_bytes,
                )?;
                entries.push(DirectoryEntry {
                    directory: attributes & 0x10 != 0,
                    read_only: attributes & 1 != 0,
                    name: String::from_utf8(name).map_err(|_| Error::InvalidToken)?,
                    bytes,
                });
            }
            self.host
                .write(base + 64, &words(&[self.p.completion, base + 256]))
                .await?;
            code = if directories {
                self.p.directory_next_code
            } else {
                self.p.list_next_code
            };
            size = 8;
            require(entries.len() < 1_000_000, "Directory scan limit exceeded")?;
        }
    }
    pub(super) async fn transfer(&mut self, request: Request) -> Result<Response> {
        match request {
            Request::List { pattern } => Ok(Response::Entries(
                self.scan(&pattern, false)
                    .await?
                    .into_iter()
                    .map(|e| Entry {
                        name: e.name,
                        bytes: e.bytes,
                    })
                    .collect(),
            )),
            Request::ReadDirectory { path } => {
                let pattern = format!("{}\\*.*", path.trim_end_matches('\\'));
                let mut entries = self.scan(&pattern, false).await?;
                entries.extend(self.scan(&pattern, true).await?);
                Ok(Response::Directory(entries))
            }
            Request::Shrink { path, size } => {
                self.open(&path, 2).await?;
                let (status, _) = self
                    .io(
                        self.p.close,
                        &[self.handle.expect("opened"), size, 0],
                        false,
                    )
                    .await?;
                require(status == 0, "SDK truncate failed")?;
                self.handle = None;
                self.active = None;
                self.position = None;
                Ok(Response::Done)
            }
            Request::Create { path } => {
                self.close().await?;
                self.open(&path, 1).await?;
                self.close().await?;
                Ok(Response::Done)
            }
            Request::ReadAt { path, offset, size } => {
                if size == 0 {
                    return Ok(Response::Bytes(Vec::new()));
                }
                self.open(&path, 0).await?;
                self.seek(offset).await?;
                let mut data = Vec::with_capacity(size as usize);
                while data.len() < size as usize {
                    let count = self.p.chunk_bytes.min(size - data.len() as u32);
                    let buffer = self.buffer()?;
                    let (status, n) = self
                        .io(
                            self.p.read,
                            &[self.handle.expect("opened"), count, buffer],
                            true,
                        )
                        .await?;
                    require(status == 0 && n <= count, "SDK ranged read failed")?;
                    if n > 0 {
                        self.call(self.p.cache, &[]).await?;
                        data.extend(self.read_buffer(buffer, n).await?);
                    }
                    self.position = self.position.and_then(|p| p.checked_add(n));
                    if n < count {
                        break;
                    }
                }
                Ok(Response::Bytes(data))
            }
            Request::WriteAt { path, offset, data } => {
                self.open(&path, 2).await?;
                self.seek(offset).await?;
                for raw in data.chunks(self.p.chunk_bytes as usize) {
                    let buffer = self.buffer()?;
                    self.write_buffer(buffer, raw).await?;
                    self.call(self.p.cache, &[]).await?;
                    let (status, n) = self
                        .io(
                            self.p.write,
                            &[self.handle.expect("opened"), raw.len() as u32, buffer],
                            true,
                        )
                        .await?;
                    require(
                        status == 0 && n as usize == raw.len(),
                        "SDK ranged write failed",
                    )?;
                    self.position = self.position.and_then(|p| p.checked_add(n));
                }
                Ok(Response::Done)
            }
            Request::Delete { path } => {
                if !self.scan(&path, false).await?.is_empty() {
                    self.path(&path).await?;
                    let (status, _) = self.io(self.p.unlink, &[self.base()?], false).await?;
                    require(status == 0, &format!("Card unlink failed: {status:#x}"))?;
                }
                require(
                    self.scan(&path, false).await?.is_empty(),
                    "Deleted card file remains visible",
                )?;
                Ok(Response::Done)
            }
            Request::Read { path, max_bytes } => {
                self.close().await?;
                self.open(&path, 0).await?;
                let mut data = Vec::new();
                loop {
                    let size = self.p.chunk_bytes.min(
                        max_bytes
                            .saturating_sub(data.len() as u32)
                            .saturating_add(1),
                    );
                    let buffer = self.buffer()?;
                    let (status, n) = self
                        .io(
                            self.p.read,
                            &[self.handle.expect("opened"), size, buffer],
                            true,
                        )
                        .await?;
                    require(
                        status == 0 && n <= size,
                        "SDK read failed or returned an invalid count",
                    )?;
                    require(
                        data.len() as u64 + u64::from(n) <= u64::from(max_bytes),
                        "File exceeds requested limit",
                    )?;
                    self.call(self.p.cache, &[]).await?;
                    if n > 0 {
                        data.extend(self.read_buffer(buffer, n).await?);
                    }
                    if n < size {
                        break;
                    }
                }
                self.close().await?;
                Ok(Response::Bytes(data))
            }
            Request::Write { path, data } => {
                self.open(&path, 1).await?;
                for raw in data.chunks(self.p.chunk_bytes as usize) {
                    let buffer = self.buffer()?;
                    self.write_buffer(buffer, raw).await?;
                    self.call(self.p.cache, &[]).await?;
                    let (status, n) = self
                        .io(
                            self.p.write,
                            &[self.handle.expect("opened"), raw.len() as u32, buffer],
                            true,
                        )
                        .await?;
                    require(
                        status == 0 && n as usize == raw.len(),
                        "SDK short/error write",
                    )?;
                }
                self.close().await?;
                Ok(Response::Done)
            }
        }
    }
    pub(super) async fn cleanup(&mut self) -> Result<()> {
        require(
            self.host.session.state() == SessionState::Ready && !self.pending && !self.cursor,
            "Uncertain card operation; retain RAM",
        )?;
        self.close().await?;
        if let Some(base) = self.base {
            self.host.put(self.p.error, SENTINEL).await?;
            self.call(self.p.release, &[self.pool, base]).await?;
            require(
                self.host.word(self.p.error).await? == SENTINEL,
                "RAM pool release failed",
            )?;
            self.base = None;
        }
        for (address, raw) in self.originals.clone() {
            self.host.write(address, &raw).await?;
            self.host.check(address, &raw).await?;
        }
        require(
            self.host.word(self.p.callback_table_pointer).await? == self.table,
            "Service table pointer changed",
        )?;
        require(
            format!(
                "{:x}",
                Sha256::digest(self.host.read(self.table, self.p.table_bytes).await?)
            ) == self.p.table_sha256,
            "Service callbacks were not restored",
        )
    }
}

pub(super) async fn prepare<'a, 'p>(mut host: Host<'a>, p: &'p StockSdk) -> Result<Sdk<'a, 'p>> {
    require(
        p.chunk_bytes > 0
            && p.chunk_bytes <= 1024 * 1024
            && p.pool_bytes >= p.chunk_bytes + DATA_RESERVATION
            && p.table_bytes >= p.callback_offset + 4,
        "Invalid stock SDK geometry",
    )?;
    require(
        p.directory_context_bytes >= 48
            && p.directory_context_bytes <= 256
            && p.directory_attribute_offset < p.directory_context_bytes
            && matches!(p.directory_size_bytes, 4 | 8)
            && u64::from(p.directory_size_offset) + u64::from(p.directory_size_bytes)
                <= u64::from(p.directory_context_bytes),
        "Invalid directory layout",
    )?;
    require(
        host.word(p.ui_address).await? == p.normal_ui,
        "Card access requires normal REC",
    )?;
    require(
        host.word(p.sdk_table_cell).await? == p.sdk_table && host.word(p.drive_map).await? == 0,
        "SD0 SDK unavailable",
    )?;
    let table = host.word(p.callback_table_pointer).await?;
    require(
        table % 4 == 0
            && format!(
                "{:x}",
                Sha256::digest(host.read(table, p.table_bytes).await?)
            ) == p.table_sha256,
        "Service callback table differs",
    )?;
    let slot = table
        .checked_add(p.callback_offset)
        .ok_or(Error::AddressOverflow)?;
    require(
        host.word(slot).await? == p.stock_callback,
        "Query callback differs",
    )?;
    let mut originals = Vec::new();
    for (address, size) in [
        (p.request, REQUEST_BYTES),
        (p.status, 4),
        (p.value, 4),
        (p.error, 4),
    ] {
        originals.push((address, host.read(address, size).await?));
    }
    require(
        u32_at(&originals[0].1, 0) == 0 && u32_at(&originals[0].1, 32) == 0,
        "Stock dispatcher has outstanding callbacks",
    )?;
    let pool = host.word(p.pool_id).await?;
    require(pool > 0 && pool < 0x8000, "Stock pool unavailable")?;
    let mut sdk = Sdk {
        host,
        p,
        slot,
        table,
        originals,
        base: None,
        pool,
        handle: None,
        active: None,
        position: None,
        pending: false,
        cursor: false,
    };
    if let Err(error) = sdk.allocate().await {
        return match sdk.cleanup().await {
            Ok(()) => Err(error),
            Err(cleanup) => {
                sdk.host.session.invalidate_completion();
                Err(fail(format!("{error}; cleanup: {cleanup}")))
            }
        };
    }
    Ok(sdk)
}

pub(super) async fn execute(host: Host<'_>, p: &StockSdk, request: Request) -> Result<Response> {
    let mut sdk = prepare(host, p).await?;
    let result = sdk.transfer(request).await;
    let cleanup = sdk.cleanup().await;
    match (result, cleanup) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) => Err(error),
        (result, Err(cleanup)) => Err(fail(format!(
            "{}; cleanup: {cleanup}",
            result
                .err()
                .map(|e| e.to_string())
                .unwrap_or_else(|| "Card transfer completed".into())
        ))),
    }
}

fn directory_size(raw: &[u8], offset: usize, width: u32) -> Result<u32> {
    require(matches!(width, 4 | 8), "Invalid directory size width")?;
    require(
        offset
            .checked_add(width as usize)
            .is_some_and(|end| end <= raw.len()),
        "Truncated directory size field",
    )?;
    if width == 8 {
        require(
            u32_at(raw, offset + 4) == 0,
            "Directory file size exceeds 32 bits",
        )?;
    }
    Ok(u32_at(raw, offset))
}

#[cfg(test)]
mod directory_size_tests {
    use super::directory_size;

    #[test]
    fn allocation_word_is_not_part_of_a_four_byte_size() {
        let raw = [42, 0, 0, 0, 0, 16, 0, 0];
        assert_eq!(directory_size(&raw, 0, 4).unwrap(), 42);
        assert!(directory_size(&raw, 0, 8).is_err());
        assert_eq!(directory_size(&raw[..4], 0, 4).unwrap(), 42);
    }

    #[test]
    fn eight_byte_sizes_and_bounds_remain_checked() {
        assert_eq!(
            directory_size(&[42, 0, 0, 0, 0, 0, 0, 0], 0, 8).unwrap(),
            42
        );
        assert!(directory_size(&[0; 4], 0, 8).is_err());
        assert!(directory_size(&[0; 8], 0, 3).is_err());
        assert!(directory_size(&[0; 8], usize::MAX, 4).is_err());
    }
}
