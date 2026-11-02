//! FIL1 descriptor ABI: the same file service used by camera reports.
use super::*;

struct Mailbox<'a> {
    host: Host<'a>,
    start: u32,
    size: u32,
    alias: u32,
    address: u32,
    descriptor: Vec<u8>,
    p: Vec<u32>,
    capacity: u32,
    operations: u32,
    source: Option<u32>,
}
impl Mailbox<'_> {
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
        require(
            self.read(self.address, 64).await? == self.descriptor,
            "File-service descriptor changed",
        )?;
        require(
            self.word(self.p[7]).await? == self.operations
                && self.word(self.p[1]).await? == 0
                && self.word(self.p[2]).await? != 1,
            "File service busy or another operation intervened",
        )
    }
    async fn command(&mut self, op: u32, size: u32) -> Result<u32> {
        require(size <= 0xffffff, "File-service argument too large")?;
        self.idle().await?;
        self.write(self.p[1], &words(&[op | size << 8])).await?;
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            require(
                self.read(self.address, 64).await? == self.descriptor,
                "File-service descriptor changed",
            )?;
            let request = self.word(self.p[1]).await?;
            let state = self.word(self.p[2]).await?;
            if request == 0 && state != 1 {
                require(
                    self.word(self.p[7]).await? == self.operations.wrapping_add(1)
                        && self.word(self.p[8]).await? == 0,
                    "File-service operation ownership changed",
                )?;
                self.operations = self.operations.wrapping_add(1);
                let status = self.word(self.p[3]).await?;
                require(
                    state == 2 && status == 0,
                    &format!("File-service operation {op} failed: {status:#x}"),
                )?;
                return self.word(self.p[4]).await;
            }
            if Instant::now() >= deadline {
                self.host.session.invalidate_completion();
                return Err(fail("File-service completion unknown; buffer retained"));
            }
            futures_timer::Delay::new(Duration::from_millis(10)).await;
        }
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
    async fn transfer(&mut self, request: Request) -> Result<Response> {
        match request {
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
                self.command(5, 0).await?;
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

pub(super) async fn execute(
    host: Host<'_>,
    start: u32,
    size: u32,
    alias: u32,
    build: Option<u32>,
    request: Request,
) -> Result<Response> {
    require(
        start % 32 == 0
            && size >= 64
            && size <= 16 * 1024 * 1024
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
    };
    let raw = service.read(start, size).await?;
    let found: Vec<_> = (0..raw.len() - 63)
        .step_by(32)
        .filter(|&i| {
            u32_at(&raw, i) == 0x314c4946
                && (1..=2).contains(&u32_at(&raw, i + 4))
                && build.is_none_or(|v| u32_at(&raw, i + 8) == v)
        })
        .collect();
    require(
        found.len() == 1,
        "No unique matching installed file service",
    )?;
    service.address = start + found[0] as u32;
    service.descriptor = raw[found[0]..found[0] + 64].to_vec();
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
    let result = service.transfer(request).await;
    if result.is_err() && service.host.session.state() == crate::ff80::SessionState::Ready {
        service.idle().await?;
        if service.word(service.p[9]).await? != u32::MAX {
            service.command(4, 0).await?;
        }
    }
    if service.host.session.state() == crate::ff80::SessionState::Ready {
        if let Some(source) = service.source {
            service.idle().await?;
            require(
                service.word(service.p[10]).await? == 0,
                "File-service source pointer changed",
            )?;
            service.write(service.p[10], &source.to_le_bytes()).await?;
            require(
                service.word(service.p[10]).await? == source,
                "File-service source pointer was not restored",
            )?;
        }
    }
    result
}
