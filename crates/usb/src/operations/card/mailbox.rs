//! FIL1 descriptor ABI: the same file service used by camera reports.
use super::*;

pub(super) struct Mailbox<'a> {
    pub(super) host: Host<'a>,
    start: u32,
    size: u32,
    alias: u32,
    address: u32,
    descriptor: Vec<u8>,
    p: Vec<u32>,
    capacity: u32,
    operations: u32,
    source: Option<u32>,
    pub(super) trigger: Option<super::QueryTrigger>,
}
impl Mailbox<'_> {
    pub(super) fn location(&self) -> u32 {
        self.address
    }

    fn address(&self, address: u32, size: u32) -> Result<u32> {
        let physical = address & !self.alias;
        require(
            physical >= self.start
                && u64::from(physical) + u64::from(size)
                    <= u64::from(self.start) + u64::from(self.size),
            "File-service pointer outside its reservation",
        )?;
        Ok(physical | self.alias)
    }
    async fn read(&mut self, address: u32, size: u32) -> Result<Vec<u8>> {
        let at = self.address(address, size)?;
        self.host.read(at, size).await
    }
    async fn word(&mut self, address: u32) -> Result<u32> {
        Ok(u32_at(&self.read(address, 4).await?, 0))
    }
    async fn write(&mut self, address: u32, raw: &[u8]) -> Result<()> {
        let at = self.address(
            address,
            raw.len().try_into().map_err(|_| Error::InvalidLength)?,
        )?;
        self.host.write(at, raw).await
    }
    async fn idle(&mut self) -> Result<()> {
        let state = self.snapshot().await?;
        require(
            state[0] == self.operations && state[1] == 0 && state[2] != 1,
            "File service busy or another operation intervened",
        )
    }
    async fn snapshot(&mut self) -> Result<[u32; 6]> {
        let mut fields = [
            self.p[7], self.p[1], self.p[2], self.p[8], self.p[3], self.p[4],
        ];
        for field in &mut fields {
            *field = self.address(*field, 4)?;
        }
        let descriptor_address = self.address(self.address, 64)?;
        let start = fields
            .iter()
            .copied()
            .chain([descriptor_address])
            .min()
            .unwrap();
        let end = fields
            .iter()
            .map(|&a| u64::from(a) + 4)
            .chain([u64::from(descriptor_address) + 64])
            .max()
            .unwrap();
        let size = u32::try_from(end - u64::from(start)).map_err(|_| Error::InvalidLength)?;
        // These are already checked RAM pointers in the service reservation.
        // The owned helper puts them in one small span, so descriptor and
        // completion guards cost one bounded command instead of seven waits.
        let (descriptor, values) = if size <= 1024 {
            let raw = self.read(start, size).await?;
            let d = (descriptor_address - start) as usize;
            (
                raw[d..d + 64].to_vec(),
                fields.map(|at| u32_at(&raw, (at - start) as usize)),
            )
        } else {
            let mut ranges = vec![range(descriptor_address, 64)?];
            ranges.extend(
                fields
                    .iter()
                    .map(|&at| range(at, 4))
                    .collect::<Result<Vec<_>>>()?,
            );
            let raw =
                super::super::memory::read_ranges(self.host.session, &ranges, self.host.limits)
                    .await?;
            (
                raw[0].clone(),
                std::array::from_fn(|i| u32_at(&raw[i + 1], 0)),
            )
        };
        require(
            descriptor == self.descriptor,
            "File-service descriptor changed",
        )?;
        Ok(values)
    }
    async fn command(&mut self, op: u32, size: u32) -> Result<u32> {
        let (status, count) = self.command_status(op, size).await?;
        require(
            status == 0,
            &format!("File-service operation {op} failed: {status:#x}"),
        )?;
        Ok(count)
    }
    async fn command_status(&mut self, op: u32, size: u32) -> Result<(u32, u32)> {
        require(size <= 0xffffff, "File-service argument too large")?;
        self.idle().await?;
        self.write(self.p[1], &words(&[op | size << 8])).await?;
        if let Some(trigger) = self.trigger {
            use crate::ff80::commands::diagnostic::DiagnosticTag;
            use crate::operations::borrowed::BorrowedQueries;
            let outputs = self.host.read(trigger.outputs, 8).await?;
            let mut call = BorrowedQueries::new(
                Address(trigger.slot),
                Address(trigger.stock),
                Address(trigger.outputs),
                outputs,
            )?;
            call.call(
                Address(trigger.entry),
                DiagnosticTag::Research(trigger.argument),
                vec![(0, 0)],
                vec![0; 8],
            )?;
            call.invoke(self.host.session, self.host.limits).await?;
        }
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let snapshot = self.snapshot().await?;
            let request = snapshot[1];
            let state = snapshot[2];
            if request == 0 && state != 1 {
                require(
                    snapshot[0] == self.operations.wrapping_add(1) && snapshot[3] == 0,
                    "File-service operation ownership changed",
                )?;
                self.operations = self.operations.wrapping_add(1);
                let status = snapshot[4];
                require(
                    state == 2 || state == 3,
                    "Invalid file-service completion state",
                )?;
                return Ok((status, snapshot[5]));
            }
            if Instant::now() >= deadline {
                self.host.session.invalidate_completion();
                return Err(fail("File-service completion unknown; buffer retained"));
            }
            futures_timer::Delay::new(Duration::from_millis(10)).await;
        }
    }
    async fn open_update(&mut self, path: &str) -> Result<()> {
        // Append-open on a zero-length FAT file can accept a missing tail and
        // link the first allocated cluster through reserved FAT entry zero.
        // Create mode gives the first write a fresh writable stream instead.
        let entries = self.list(path).await?;
        if entries.len() == 1
            && entries[0].bytes == 0
            && path
                .rsplit('\\')
                .next()
                .is_some_and(|name| entries[0].name.eq_ignore_ascii_case(name))
        {
            self.path(path).await?;
            self.command(9, 0).await?;
            return Ok(());
        }
        self.path(path).await?;
        let (status, _) = self.command_status(5, 0).await?;
        if status == 0 {
            return Ok(());
        }
        // Some append-mode implementations refuse an empty file with no tail
        // cluster. A completed refusal leaves no handle. Verify its live size
        // before using create mode, which must never erase existing contents.
        if status == 5 && self.word(self.p[9]).await? == u32::MAX {
            let entries = self.list(path).await?;
            if entries.len() == 1 && entries[0].bytes == 0 {
                self.path(path).await?;
                self.command(9, 0).await?;
                return Ok(());
            }
        }
        Err(fail(format!(
            "File-service operation 5 failed: {status:#x}"
        )))
    }
    async fn path(&mut self, p: &str) -> Result<()> {
        self.idle().await?;
        require(
            self.word(self.p[9]).await? == u32::MAX,
            "Another file is open",
        )?;
        self.write(self.p[5], &[p.as_bytes(), b"\0"].concat()).await
    }
    async fn list(&mut self, pattern: &str) -> Result<Vec<Entry>> {
        require(
            u32_at(&self.descriptor, 4) >= 2,
            "Listing requires file-service ABI 2",
        )?;
        let mut entries = Vec::new();
        loop {
            self.path(pattern).await?;
            let count = self
                .command(
                    8,
                    entries.len().try_into().map_err(|_| Error::InvalidLength)?,
                )
                .await?;
            require(count <= self.capacity, "Directory page exceeds buffer")?;
            if count == 0 {
                return Ok(entries);
            }
            entries.extend(decode_entries(&self.read(self.p[6], count).await?)?);
        }
    }
    fn require_mount_abi(&self) -> Result<()> {
        require(
            u32_at(&self.descriptor, 4) >= 3,
            "Mounting requires installed file-service ABI 3",
        )
    }
    async fn seek(&mut self, offset: u32) -> Result<()> {
        self.write(self.p[6], &offset.to_le_bytes()).await?;
        self.command(11, 0).await?;
        Ok(())
    }
    pub(super) async fn finish(&mut self) -> Result<()> {
        self.idle().await?;
        if self.word(self.p[9]).await? != u32::MAX {
            self.command(4, 0).await?;
        }
        if let Some(source) = self.source {
            self.idle().await?;
            require(
                self.word(self.p[10]).await? == 0,
                "File-service source pointer changed",
            )?;
            self.write(self.p[10], &source.to_le_bytes()).await?;
            require(
                self.word(self.p[10]).await? == source,
                "File-service source pointer was not restored",
            )?;
            self.source = None;
        }
        Ok(())
    }

    pub(super) async fn transfer(&mut self, request: Request) -> Result<Response> {
        match request {
            Request::ReadDirectory { path } => {
                self.require_mount_abi()?;
                let pattern = format!("{}\\*.*", path.trim_end_matches('\\'));
                let mut entries = Vec::new();
                loop {
                    self.path(&pattern).await?;
                    let count = self
                        .command(
                            10,
                            entries.len().try_into().map_err(|_| Error::InvalidLength)?,
                        )
                        .await?;
                    require(
                        count <= self.capacity && count % 24 == 0,
                        "Invalid directory page",
                    )?;
                    if count == 0 {
                        break;
                    }
                    let raw = self.read(self.p[6], count).await?;
                    for row in raw.as_chunks::<24>().0 {
                        let mut legacy = row.to_vec();
                        legacy[23] = 0;
                        let entry = decode_entries(&legacy)?.remove(0);
                        entries.push(DirectoryEntry {
                            name: entry.name,
                            bytes: entry.bytes,
                            directory: row[23] & 0x10 != 0,
                            read_only: row[23] & 1 != 0,
                        });
                    }
                    require(entries.len() < 1_000_000, "Directory scan limit exceeded")?;
                }
                Ok(Response::Directory(entries))
            }
            Request::Shrink { path, size } => {
                self.require_mount_abi()?;
                self.path(&path).await?;
                self.open_update(&path).await?;
                self.write(self.p[6], &size.to_le_bytes()).await?;
                self.command(12, 0).await?;
                Ok(Response::Done)
            }
            Request::Create { path } => {
                self.require_mount_abi()?;
                self.path(&path).await?;
                self.command(9, 0).await?;
                self.command(3, 0).await?;
                Ok(Response::Done)
            }
            Request::ReadAt { path, offset, size } => {
                self.require_mount_abi()?;
                self.path(&path).await?;
                if u32_at(&self.descriptor, 56) & 1 != 0 && size <= self.capacity {
                    self.write(self.p[6], &offset.to_le_bytes()).await?;
                    let count = self.command(14, size).await?;
                    require(count <= size, "Invalid file range read count")?;
                    return Ok(Response::Bytes(if count == 0 {
                        Vec::new()
                    } else {
                        self.read(self.p[6], count).await?
                    }));
                }
                self.command(1, 0).await?;
                self.seek(offset).await?;
                let mut data = Vec::with_capacity(size as usize);
                while data.len() < size as usize {
                    let count = self.capacity.min(size - data.len() as u32);
                    let n = self.command(2, count).await?;
                    require(n <= count, "Invalid file read count")?;
                    if n > 0 {
                        data.extend(self.read(self.p[6], n).await?);
                    }
                    if n < count {
                        break;
                    }
                }
                self.command(3, 0).await?;
                Ok(Response::Bytes(data))
            }
            Request::WriteAt { path, offset, data } => {
                self.require_mount_abi()?;
                self.path(&path).await?;
                self.open_update(&path).await?;
                self.seek(offset).await?;
                self.source = Some(self.word(self.p[10]).await?);
                self.host.put(self.address(self.p[10], 4)?, 0).await?;
                for raw in data.chunks(self.capacity as usize) {
                    self.host
                        .write_owned(self.address(self.p[6], self.capacity)?, raw, self.capacity)
                        .await?;
                    let n = self.command(6, raw.len() as u32).await?;
                    require(n as usize == raw.len(), "Short file write")?;
                }
                self.command(3, 0).await?;
                Ok(Response::Done)
            }
            Request::List { pattern } => Ok(Response::Entries(self.list(&pattern).await?)),
            Request::Delete { path } => {
                self.path(&path).await?;
                self.command(7, 0).await?;
                Ok(Response::Done)
            }
            Request::Read { path, max_bytes } => {
                self.path(&path).await?;
                self.command(1, 0).await?;
                let mut data = Vec::new();
                loop {
                    let size = self.capacity.min(
                        max_bytes
                            .saturating_sub(data.len() as u32)
                            .saturating_add(1),
                    );
                    let n = self.command(2, size).await?;
                    require(
                        n <= size && data.len() as u64 + u64::from(n) <= u64::from(max_bytes),
                        "File exceeds requested limit",
                    )?;
                    if n > 0 {
                        data.extend(self.read(self.p[6], n).await?);
                    }
                    if n < size {
                        break;
                    }
                }
                self.command(3, 0).await?;
                Ok(Response::Bytes(data))
            }
            Request::Write { path, data } => {
                // ABI 2 exposes in-place rewrite. Require an exact existing
                // length before opening so stale trailing bytes cannot survive.
                let entry = self.list(&path).await?;
                require(
                    entry.len() == 1 && entry[0].bytes as usize == data.len(),
                    "This installed file service rewrites existing files of equal length",
                )?;
                self.path(&path).await?;
                self.open_update(&path).await?;
                self.source = Some(self.word(self.p[10]).await?);
                self.host.put(self.address(self.p[10], 4)?, 0).await?;
                for chunk in data.chunks(self.capacity as usize) {
                    self.write(self.p[6], chunk).await?;
                    let n = self.command(6, chunk.len() as u32).await?;
                    require(n as usize == chunk.len(), "Short file write")?;
                }
                self.command(3, 0).await?;
                Ok(Response::Done)
            }
        }
    }
}

pub(super) async fn prepare<'a>(
    host: Host<'a>,
    start: u32,
    size: u32,
    alias: u32,
    build: Option<u32>,
    hint: Option<u32>,
) -> Result<Mailbox<'a>> {
    require(
        start.is_multiple_of(32)
            && (64..=16 * 1024 * 1024).contains(&size)
            && (alias == 0 || alias.is_power_of_two())
            && start & alias == 0,
        "Invalid file-service reservation",
    )?;
    range(start | alias, size)?;
    let mut service = Mailbox {
        host,
        start,
        size,
        alias,
        address: start,
        descriptor: Vec::new(),
        p: Vec::new(),
        capacity: 0,
        operations: 0,
        source: None,
        trigger: None,
    };
    let matches = |raw: &[u8]| {
        u32_at(raw, 0) == 0x314c4946
            && (1..=3).contains(&u32_at(raw, 4))
            && build.is_none_or(|v| u32_at(raw, 8) == v)
    };
    let mut descriptor = None;
    if let Some(hint) = hint.filter(|h| {
        *h % 32 == 0 && *h >= start && u64::from(*h) + 64 <= u64::from(start) + u64::from(size)
    }) {
        let raw = service.read(hint, 64).await?;
        if matches(&raw) {
            descriptor = Some((hint, raw));
        }
    }
    let (location, descriptor) = if let Some(found) = descriptor {
        found
    } else {
        let raw = service.read(start, size).await?;
        let found: Vec<_> = (0..raw.len() - 63)
            .step_by(32)
            .filter(|&i| matches(&raw[i..i + 64]))
            .collect();
        require(found.len() < 2, "Several matching installed file services")?;
        if found.is_empty() {
            return Err(Error::NoFileService);
        }
        (
            start + found[0] as u32,
            raw[found[0]..found[0] + 64].to_vec(),
        )
    };
    service.address = location;
    service.descriptor = descriptor;
    service.p = (0..11)
        .map(|i| u32_at(&service.descriptor, 12 + i * 4))
        .collect();
    for &pointer in &service.p {
        service.address(pointer, 4)?;
    }
    service.address(service.p[5], 64)?;
    service.capacity = service.word(service.p[0]).await?;
    require(
        service.capacity > 0 && service.capacity <= 0xffffff,
        "Invalid file buffer capacity",
    )?;
    service.address(service.p[6], service.capacity)?;
    service.operations = service.word(service.p[7]).await?;
    Ok(service)
}

pub(super) async fn execute(
    host: Host<'_>,
    start: u32,
    size: u32,
    alias: u32,
    build: Option<u32>,
    request: Request,
) -> Result<Response> {
    let mut service = prepare(host, start, size, alias, build, None).await?;
    let result = service.transfer(request).await;
    if service.host.session.state() == crate::ff80::SessionState::Ready {
        service.finish().await?;
    }
    result
}
