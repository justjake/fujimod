//! Decoder for Fujifilm's lossless compressed X-Trans RAF planes.
//!
//! # Stream layout
//!
//! The compressed plane starts with a 16-byte big-endian header: signature
//! `0x4953`, format version (1 is lossless), CFA kind (16 is X-Trans), sample
//! depth, plane height, plane width rounded up to whole strips, plane width,
//! strip width, strip count and the number of six-row groups. One big-endian
//! 32-bit byte length per strip follows, padded to a multiple of 16 bytes,
//! then the strips back to back. A strip is a column of the plane
//! `strip_width` sites wide and the full plane high, coded independently of
//! the others; the last strip may cover fewer real sites than it codes.
//!
//! # Lines
//!
//! Each six-row group of a strip is rearranged into color lines: one green
//! line per raw row and one red and one blue line per pair of raw rows. Of
//! every three sites in a raw row, the first lands on an even line position
//! and the other two on the following odd position, so each line holds two
//! thirds of the strip width. The six lines' positions that no site of the
//! CFA tile maps to are not coded; even ones are interpolated from the lines
//! above. Each color keeps the last two lines of the previous group as
//! history, and every line borrows one border value on each side from the
//! line above it.
//!
//! # Coding
//!
//! The group is coded as six line pairs: (red 0, green 0), (green 1, blue 0),
//! (red 1, green 2), (green 3, blue 1), (red 2, green 4), (green 5, blue 2),
//! using context sets 0, 1, 2, 0, 1, 2. Within a pair the even positions of
//! both lines advance together and the odd positions follow eight sites
//! behind, because an odd prediction uses its decoded even neighbors. Even
//! positions are predicted from the two lines above, odd positions from the
//! neighbors on either side and the line above. The prediction error is
//! coded with an adaptive Rice code whose parameter is tracked for each of
//! 41 quantized gradient contexts, separately for even and odd positions.
//!
//! The format has no checksum. A misread or desynchronized stream is
//! refused: every code must stay within the sample range and every strip
//! must end within the zero padding its recorded length leaves. A flipped
//! bit inside a residual's low bits keeps the code in step and decodes with
//! changed values.

/// Fields of the compressed-plane header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamHeader {
    pub bits: u32,
    pub height: usize,
    pub width: usize,
    pub rounded_width: usize,
    pub strip_width: usize,
    pub strips: usize,
    pub groups: usize,
}

/// Why a compressed plane is not decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The header names a format variant this decoder does not implement.
    Unsupported(String),
    /// The stream is inconsistent with its header or with itself.
    Corrupt(String),
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::Unsupported(m) => write!(f, "unsupported compressed RAF: {m}"),
            Refusal::Corrupt(m) => write!(f, "corrupt compressed RAF plane: {m}"),
        }
    }
}

fn corrupt(msg: impl Into<String>) -> Refusal {
    Refusal::Corrupt(msg.into())
}

const SIGNATURE: u16 = 0x4953;
const VERSION_LOSSLESS: u8 = 1;
const CFA_XTRANS: u8 = 16;
const HEADER_BYTES: usize = 16;

fn be16(b: &[u8], at: usize) -> usize {
    u16::from_be_bytes([b[at], b[at + 1]]) as usize
}

/// Parses and checks the header. Lossy and non-X-Trans streams are refused
/// as unsupported.
pub fn read_header(stream: &[u8]) -> Result<StreamHeader, Refusal> {
    if stream.len() < HEADER_BYTES {
        return Err(corrupt("header is truncated"));
    }
    let signature = be16(stream, 0) as u16;
    if signature != SIGNATURE {
        return Err(corrupt(format!(
            "header signature is {signature:#06x}, expected {SIGNATURE:#06x}"
        )));
    }
    let (version, kind) = (stream[2], stream[3]);
    if version != VERSION_LOSSLESS {
        return Err(Refusal::Unsupported(format!(
            "format version {version} is lossy compression, which is not decoded. \
             Shoot with RAW RECORDING set to LOSSLESS COMPRESSED or UNCOMPRESSED."
        )));
    }
    if kind != CFA_XTRANS {
        return Err(Refusal::Unsupported(format!(
            "CFA kind {kind} is not X-Trans"
        )));
    }
    let h = StreamHeader {
        bits: stream[4] as u32,
        height: be16(stream, 5),
        rounded_width: be16(stream, 7),
        width: be16(stream, 9),
        strip_width: be16(stream, 11),
        strips: stream[13] as usize,
        groups: be16(stream, 14),
    };
    if h.bits != 14 {
        return Err(Refusal::Unsupported(format!(
            "{}-bit samples (only 14-bit streams are verified)",
            h.bits
        )));
    }
    let consistent = h.height >= 6
        && h.height.is_multiple_of(6)
        && h.groups == h.height / 6
        && h.strip_width >= 6
        && h.strip_width.is_multiple_of(6)
        && h.width > 0
        && h.width.is_multiple_of(6)
        && h.rounded_width.is_multiple_of(h.strip_width)
        && h.rounded_width >= h.width
        && h.rounded_width - h.width < h.strip_width
        && h.strips == h.rounded_width / h.strip_width;
    if !consistent {
        return Err(corrupt(format!("inconsistent header {h:?}")));
    }
    Ok(h)
}

/// Returns each strip's bytes, checking that they lie within the stream.
fn strips<'a>(stream: &'a [u8], h: &StreamHeader) -> Result<Vec<&'a [u8]>, Refusal> {
    let mut at = (HEADER_BYTES + 4 * h.strips).next_multiple_of(16);
    if stream.len() < at {
        return Err(corrupt("strip length table is truncated"));
    }
    (0..h.strips)
        .map(|i| {
            let o = HEADER_BYTES + 4 * i;
            let len = u32::from_be_bytes([stream[o], stream[o + 1], stream[o + 2], stream[o + 3]])
                as usize;
            let s = stream
                .get(at..at + len)
                .ok_or_else(|| corrupt(format!("strip {i} runs past the end of the plane")))?;
            at += len;
            Ok(s)
        })
        .collect()
}

/// Fixed coding parameters for a sample depth.
struct Params {
    bits: u32,
    max: i32,
    total: i32,
    /// A run of this many zero bits announces a raw `bits`-bit code.
    escape_zeros: u32,
    /// Quantized gradient (-4..=4) for each difference, indexed by
    /// `difference + max`.
    quant: Vec<i8>,
    initial_magnitude: u32,
}

/// Gradient quantization thresholds: differences of 1-17, 18-66, 67-275 and
/// 276 or more fall into steps 1 to 4.
const GRADIENT_STEPS: [i32; 3] = [18, 67, 276];
/// A context's statistics are halved when its count reaches this value.
const RESET_COUNT: u32 = 64;
/// 41 contexts: `|9 * q(first difference) + q(second difference)|`.
const CONTEXTS: usize = 41;

impl Params {
    fn new(bits: u32) -> Self {
        let max = (1i32 << bits) - 1;
        let quant = (-max..=max)
            .map(|d: i32| {
                let m = d.abs();
                let step = 1 + GRADIENT_STEPS.iter().filter(|&&t| m >= t).count() as i8;
                let q = if m == 0 { 0 } else { step };
                if d < 0 { -q } else { q }
            })
            .collect();
        Self {
            bits,
            max,
            total: max + 1,
            escape_zeros: 3 * bits - 1,
            quant,
            initial_magnitude: ((max + 1) >> 6) as u32,
        }
    }

    fn q(&self, d: i32) -> i32 {
        self.quant[(d + self.max) as usize] as i32
    }
}

/// Running statistics of one gradient context.
#[derive(Clone, Copy)]
struct Context {
    magnitude: u32,
    count: u32,
}

impl Context {
    /// Rice parameter: the smallest k (at most 15) with count << k >= magnitude.
    fn k(&self) -> u32 {
        let mut k = 0;
        while k < 15 && (self.count << k) < self.magnitude {
            k += 1;
        }
        k
    }

    fn update(&mut self, residual: i32) {
        self.magnitude += residual.unsigned_abs();
        if self.count == RESET_COUNT {
            self.magnitude >>= 1;
            self.count >>= 1;
        }
        self.count += 1;
    }
}

/// Source or sink of prediction residuals.
trait Coder {
    /// Returns the residual of a coded position. `wanted` is the residual
    /// an encoder writes; a decoder reads it from the stream instead.
    fn residual(&mut self, p: &Params, c: &mut Context, wanted: i32) -> Result<i32, Refusal>;
}

/// MSB-first bit reader that refuses to read past its slice.
struct BitReader<'a> {
    data: &'a [u8],
    next: usize,
    cache: u64,
    have: u32,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            next: 0,
            cache: 0,
            have: 0,
        }
    }

    fn refill(&mut self) {
        while self.have <= 56 && self.next < self.data.len() {
            self.cache |= (self.data[self.next] as u64) << (56 - self.have);
            self.next += 1;
            self.have += 8;
        }
    }

    fn consumed_bits(&self) -> usize {
        self.next * 8 - self.have as usize
    }

    fn read(&mut self, n: u32) -> Result<u32, Refusal> {
        if n == 0 {
            return Ok(0);
        }
        if self.have < n {
            self.refill();
            if self.have < n {
                return Err(corrupt("strip ends inside a code"));
            }
        }
        let v = (self.cache >> (64 - n)) as u32;
        self.cache <<= n;
        self.have -= n;
        Ok(v)
    }

    /// Counts zero bits up to the next one bit and consumes both.
    fn zeros(&mut self, limit: u32) -> Result<u32, Refusal> {
        let mut count = 0;
        loop {
            if self.have == 0 {
                self.refill();
                if self.have == 0 {
                    return Err(corrupt("strip ends inside a code"));
                }
            }
            let lead = self.cache.leading_zeros().min(self.have);
            count += lead;
            if count > limit {
                return Err(corrupt(format!("run of {count} zero bits in a code")));
            }
            if lead < self.have {
                // `lead + 1` reaches 64 when the one bit is the cache's last.
                self.cache = self.cache.checked_shl(lead + 1).unwrap_or(0);
                self.have -= lead + 1;
                return Ok(count);
            }
            self.cache = 0;
            self.have = 0;
        }
    }

    /// Bits after the last code: fewer than one 16-byte padding block, all
    /// zero.
    fn check_padding(&mut self) -> Result<(), Refusal> {
        let left = self.data.len() * 8 - self.consumed_bits();
        if left >= 128 {
            return Err(corrupt(format!("{left} bits remain after the last code")));
        }
        let mut rest = left as u32;
        while rest > 0 {
            let n = rest.min(16);
            if self.read(n)? != 0 {
                return Err(corrupt("padding after the last code is not zero"));
            }
            rest -= n;
        }
        Ok(())
    }
}

impl Coder for BitReader<'_> {
    fn residual(&mut self, p: &Params, c: &mut Context, _wanted: i32) -> Result<i32, Refusal> {
        let zeros = self.zeros(p.escape_zeros)?;
        let code = if zeros < p.escape_zeros {
            let k = c.k();
            ((zeros as i32) << k) + self.read(k)? as i32
        } else {
            self.read(p.bits)? as i32 + 1
        };
        if code >= p.total {
            return Err(corrupt(format!(
                "residual code {code} exceeds the sample range"
            )));
        }
        let value = if code & 1 == 1 {
            -1 - code / 2
        } else {
            code / 2
        };
        c.update(value);
        Ok(value)
    }
}

/// Line slots: two history lines precede each color's current lines.
const R: usize = 0;
const G: usize = 5;
const B: usize = 13;
const LINES: usize = 18;

/// One coded line pair of a six-row group.
#[derive(Clone, Copy)]
struct Pair {
    /// Lines in coding order within each step.
    lines: [usize; 2],
    context: usize,
}

const PAIRS: [Pair; 6] = [
    Pair {
        lines: [R + 2, G + 2],
        context: 0,
    },
    Pair {
        lines: [G + 3, B + 2],
        context: 1,
    },
    Pair {
        lines: [R + 3, G + 4],
        context: 2,
    },
    Pair {
        lines: [G + 5, B + 3],
        context: 0,
    },
    Pair {
        lines: [R + 4, G + 6],
        context: 1,
    },
    Pair {
        lines: [G + 7, B + 4],
        context: 2,
    },
];

/// Odd positions trail even positions by this many sites.
const ODD_LAG: isize = 8;

/// Line slot and line position, modulo 4, of each site of the CFA tile.
type TileSlots = [[(usize, usize); 6]; 6];

fn tile_slots(cfa: &[[u8; 6]; 6]) -> TileSlots {
    let mut out = [[(0, 0); 6]; 6];
    for (r, row) in cfa.iter().enumerate() {
        for (x, &c) in row.iter().enumerate() {
            let line = match c {
                0 => R + 2 + r / 2,
                2 => B + 2 + r / 2,
                _ => G + 2 + r,
            };
            out[r][x] = (line, 2 * (x / 3) + [0, 1, 1][x % 3]);
        }
    }
    out
}

/// Line position of strip column `x` given its tile position's slot.
fn line_position(x: usize, slot: usize) -> usize {
    4 * (x / 6) + slot
}

/// Context-modeled state of one strip.
struct Strip<'a, C: Coder> {
    p: &'a Params,
    coder: C,
    width: usize,
    lines: Vec<i32>,
    /// Values an encoder must reproduce, laid out like `lines`.
    wanted: Vec<i32>,
    /// Whether line position `x % 4` of each slot carries a site.
    coded: [[bool; 4]; LINES],
    even: [[Context; CONTEXTS]; 3],
    odd: [[Context; CONTEXTS]; 3],
}

impl<'a, C: Coder> Strip<'a, C> {
    fn new(p: &'a Params, coder: C, width: usize, slots: &TileSlots) -> Self {
        let mut coded = [[false; 4]; LINES];
        for row in slots {
            for &(l, i) in row {
                coded[l][i] = true;
            }
        }
        let c = Context {
            magnitude: p.initial_magnitude,
            count: 1,
        };
        Self {
            p,
            coder,
            width,
            lines: vec![0; LINES * (width + 2)],
            wanted: Vec::new(),
            coded,
            even: [[c; CONTEXTS]; 3],
            odd: [[c; CONTEXTS]; 3],
        }
    }

    /// Index of position `x` (from -1 to `width`) of line `l`.
    fn at(&self, l: usize, x: isize) -> usize {
        l * (self.width + 2) + (x + 1) as usize
    }

    fn get(&self, l: usize, x: isize) -> i32 {
        self.lines[self.at(l, x)]
    }

    /// Codes the residual of position `x` of line `l` against `prediction`
    /// and stores the result. A negative gradient flips the residual's sign.
    fn code(
        &mut self,
        l: usize,
        x: isize,
        odd: bool,
        set: usize,
        prediction: i32,
        gradient: i32,
    ) -> Result<(), Refusal> {
        let p = self.p;
        let sign = if gradient < 0 { -1 } else { 1 };
        let at = self.at(l, x);
        let wanted = match self.wanted.get(at) {
            Some(&v) => {
                let half = p.total / 2;
                let mut r = sign * (v - prediction);
                if r >= half {
                    r -= p.total;
                } else if r < -half {
                    r += p.total;
                }
                r
            }
            None => 0,
        };
        let ctx = gradient.unsigned_abs() as usize;
        let c = if odd {
            &mut self.odd[set][ctx]
        } else {
            &mut self.even[set][ctx]
        };
        let r = self.coder.residual(p, c, wanted).map_err(|e| match e {
            Refusal::Corrupt(m) => corrupt(format!("line {l} position {x}: {m}")),
            other => other,
        })?;
        let mut v = prediction + sign * r;
        if v < 0 {
            v += p.total;
        } else if v > p.max {
            v -= p.total;
        }
        self.lines[at] = v.clamp(0, p.max);
        Ok(())
    }

    /// Prediction and gradient context of an even position.
    fn even_prediction(&self, l: usize, x: isize) -> (i32, i32) {
        let n = self.get(l - 1, x);
        let nw = self.get(l - 1, x - 1);
        let ne = self.get(l - 1, x + 1);
        let nn = self.get(l - 2, x);
        let (dw, dn, de) = ((nw - n).abs(), (nn - n).abs(), (ne - n).abs());
        let sum = if dw > dn && dw > de {
            nn + ne
        } else if de > dw && de > dn {
            nn + nw
        } else {
            ne + nw
        } + 2 * n;
        (sum >> 2, self.p.q(n - nn) * 9 + self.p.q(nw - n))
    }

    fn even(&mut self, l: usize, x: isize, set: usize) -> Result<(), Refusal> {
        let (prediction, gradient) = self.even_prediction(l, x);
        if self.coded[l][(x % 4) as usize] {
            self.code(l, x, false, set, prediction, gradient)
        } else {
            let at = self.at(l, x);
            self.lines[at] = prediction;
            Ok(())
        }
    }

    fn odd(&mut self, l: usize, x: isize, set: usize) -> Result<(), Refusal> {
        let w = self.get(l, x - 1);
        let e = self.get(l, x + 1);
        let n = self.get(l - 1, x);
        let nw = self.get(l - 1, x - 1);
        let ne = self.get(l - 1, x + 1);
        let gradient = self.p.q(n - nw) * 9 + self.p.q(nw - w);
        let prediction = if (n > nw && n > ne) || (n < nw && n < ne) {
            (w + e + 2 * n) >> 2
        } else {
            (w + e) >> 1
        };
        self.code(l, x, true, set, prediction, gradient)
    }

    /// Sets the border positions of lines `first..=last` from the line above.
    fn extend(&mut self, first: usize, last: usize) {
        let w = self.width as isize;
        for l in first..=last {
            let (left, right) = (self.get(l - 1, 0), self.get(l - 1, w - 1));
            let (a, b) = (self.at(l, -1), self.at(l, w));
            self.lines[a] = left;
            self.lines[b] = right;
        }
    }

    fn extend_color(&mut self, l: usize) {
        match l {
            _ if l < G => self.extend(R + 2, R + 4),
            _ if l < B => self.extend(G + 2, G + 7),
            _ => self.extend(B + 2, B + 4),
        }
    }

    fn pair(&mut self, pair: Pair) -> Result<(), Refusal> {
        let w = self.width as isize;
        let (mut even_x, mut odd_x) = (0isize, 1isize);
        while even_x < w || odd_x < w {
            if even_x < w {
                for l in pair.lines {
                    self.even(l, even_x, pair.context)?;
                }
                even_x += 2;
            }
            if even_x > ODD_LAG {
                for l in pair.lines {
                    self.odd(l, odd_x, pair.context)?;
                }
                odd_x += 2;
            }
        }
        for l in pair.lines {
            self.extend_color(l);
        }
        Ok(())
    }

    fn group(&mut self) -> Result<(), Refusal> {
        self.extend(R + 2, R + 4);
        self.extend(G + 2, G + 7);
        self.extend(B + 2, B + 4);
        for pair in PAIRS {
            self.pair(pair)?;
        }
        Ok(())
    }

    /// Moves each color's last two lines, borders included, into its history.
    fn rotate(&mut self) {
        let stride = self.width + 2;
        for (first, count) in [(R, 5), (G, 8), (B, 5)] {
            for k in 0..2 {
                let src = (first + count - 2 + k) * stride;
                self.lines
                    .copy_within(src..src + stride, (first + k) * stride);
            }
        }
    }
}

/// Mutable view of one strip's columns of the plane.
struct Columns {
    ptr: *mut u16,
    stride: usize,
    x0: usize,
    width: usize,
    height: usize,
}

// SAFETY: each `Columns` covers a disjoint column range of one buffer that
// outlives the scoped threads using it.
unsafe impl Send for Columns {}

impl Columns {
    fn row(&mut self, y: usize) -> &mut [u16] {
        assert!(y < self.height);
        // SAFETY: rows of disjoint column ranges never overlap and stay
        // within the buffer.
        unsafe {
            std::slice::from_raw_parts_mut(self.ptr.add(y * self.stride + self.x0), self.width)
        }
    }
}

fn split_columns(out: &mut [u16], width: usize, strip: usize) -> Vec<Columns> {
    let height = out.len() / width;
    let ptr = out.as_mut_ptr();
    (0..width.div_ceil(strip))
        .map(|i| Columns {
            ptr,
            stride: width,
            x0: i * strip,
            width: strip.min(width - i * strip),
            height,
        })
        .collect()
}

fn decode_strip(
    p: &Params,
    h: &StreamHeader,
    index: usize,
    data: &[u8],
    slots: &TileSlots,
    cols: &mut Columns,
) -> Result<(), Refusal> {
    let mut s = Strip::new(p, BitReader::new(data), h.strip_width * 2 / 3, slots);
    let where_ = |g: usize, e: Refusal| match e {
        Refusal::Corrupt(m) => corrupt(format!("strip {index}, rows {}-{}: {m}", 6 * g, 6 * g + 5)),
        other => other,
    };
    for g in 0..h.groups {
        s.group().map_err(|e| where_(g, e))?;
        for (r, tile_row) in slots.iter().enumerate() {
            let row = cols.row(6 * g + r);
            for (x, o) in row.iter_mut().enumerate() {
                let (l, slot) = tile_row[x % 6];
                *o = s.get(l, line_position(x, slot) as isize) as u16;
            }
        }
        s.rotate();
    }
    s.coder.check_padding().map_err(|e| where_(h.groups - 1, e))
}

/// Decodes a lossless compressed X-Trans plane into `out` (`width * height`
/// samples, row-major). `cfa` gives the color (0 red, 1 green, 2 blue) of
/// each site relative to the plane's origin. Strips decode in parallel.
pub fn decode(stream: &[u8], cfa: &[[u8; 6]; 6], out: &mut [u16]) -> Result<StreamHeader, Refusal> {
    let h = read_header(stream)?;
    if out.len() != h.width * h.height {
        return Err(corrupt(format!(
            "plane is {}x{} but {} samples were provided",
            h.width,
            h.height,
            out.len()
        )));
    }
    let parts = strips(stream, &h)?;
    let p = Params::new(h.bits);
    let slots = tile_slots(cfa);
    let columns = split_columns(out, h.width, h.strip_width);
    let results: Vec<Result<(), Refusal>> = std::thread::scope(|scope| {
        let handles: Vec<_> = parts
            .iter()
            .zip(columns)
            .enumerate()
            .map(|(i, (data, mut cols))| {
                let (p, h, slots) = (&p, &h, &slots);
                scope.spawn(move || decode_strip(p, h, i, data, slots, &mut cols))
            })
            .collect();
        handles
            .into_iter()
            .map(|t| t.join().expect("strip decoder panicked"))
            .collect()
    });
    results.into_iter().collect::<Result<Vec<()>, _>>()?;
    Ok(h)
}

/// MSB-first bit writer for the test encoder.
#[cfg(test)]
#[derive(Default)]
struct BitWriter {
    out: Vec<u8>,
    acc: u64,
    have: u32,
}

#[cfg(test)]
impl BitWriter {
    fn put(&mut self, n: u32, v: u32) {
        for i in (0..n).rev() {
            self.acc = (self.acc << 1) | (v.checked_shr(i).unwrap_or(0) & 1) as u64;
            self.have += 1;
            if self.have == 8 {
                self.out.push(self.acc as u8);
                self.acc = 0;
                self.have = 0;
            }
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.have > 0 {
            let pad = 8 - self.have;
            self.put(pad, 0);
        }
        let len = self.out.len().next_multiple_of(16);
        self.out.resize(len, 0);
        self.out
    }
}

#[cfg(test)]
impl Coder for BitWriter {
    fn residual(&mut self, p: &Params, c: &mut Context, wanted: i32) -> Result<i32, Refusal> {
        let code = if wanted >= 0 {
            2 * wanted
        } else {
            -2 * wanted - 1
        };
        let k = c.k();
        let zeros = (code >> k) as u32;
        if zeros < p.escape_zeros {
            self.put(zeros, 0);
            self.put(1, 1);
            self.put(k, (code & ((1 << k) - 1)) as u32);
        } else {
            self.put(p.escape_zeros, 0);
            self.put(1, 1);
            self.put(p.bits, (code - 1) as u32);
        }
        c.update(wanted);
        Ok(wanted)
    }
}

/// Test encoder: codes `plane` (`width * height`) the way `decode`
/// reads it.
#[cfg(test)]
pub(crate) fn encode(
    plane: &[u16],
    width: usize,
    height: usize,
    strip_width: usize,
    cfa: &[[u8; 6]; 6],
    bits: u32,
) -> Vec<u8> {
    let p = Params::new(bits);
    let slots = tile_slots(cfa);
    let strip_count = width.div_ceil(strip_width);
    let line_width = strip_width * 2 / 3;
    let mut coded_strips = Vec::new();
    for i in 0..strip_count {
        let x0 = i * strip_width;
        let real = strip_width.min(width - x0);
        let mut s = Strip::new(&p, BitWriter::default(), line_width, &slots);
        s.wanted = vec![0; s.lines.len()];
        for g in 0..height / 6 {
            for (r, tile_row) in slots.iter().enumerate() {
                for x in 0..real {
                    let (l, slot) = tile_row[x % 6];
                    let at = s.at(l, line_position(x, slot) as isize);
                    s.wanted[at] = plane[(6 * g + r) * width + x0 + x] as i32;
                }
            }
            s.group().unwrap();
            for (r, tile_row) in slots.iter().enumerate() {
                for x in 0..real {
                    let (l, slot) = tile_row[x % 6];
                    assert_eq!(
                        s.get(l, line_position(x, slot) as isize),
                        plane[(6 * g + r) * width + x0 + x] as i32,
                        "encoder failed to reproduce a site"
                    );
                }
            }
            s.rotate();
        }
        coded_strips.push(s.coder.finish());
    }
    let rounded = strip_count * strip_width;
    let mut out = Vec::new();
    out.extend_from_slice(&SIGNATURE.to_be_bytes());
    out.extend_from_slice(&[VERSION_LOSSLESS, CFA_XTRANS, bits as u8]);
    for v in [height, rounded, width, strip_width] {
        out.extend_from_slice(&(v as u16).to_be_bytes());
    }
    out.push(strip_count as u8);
    out.extend_from_slice(&((height / 6) as u16).to_be_bytes());
    for s in &coded_strips {
        out.extend_from_slice(&(s.len() as u32).to_be_bytes());
    }
    out.resize(out.len().next_multiple_of(16), 0);
    for s in coded_strips {
        out.extend_from_slice(&s);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const CFA: [[u8; 6]; 6] = [
        [1, 1, 0, 1, 1, 2],
        [1, 1, 2, 1, 1, 0],
        [2, 0, 1, 0, 2, 1],
        [1, 1, 2, 1, 1, 0],
        [1, 1, 0, 1, 1, 2],
        [0, 2, 1, 2, 0, 1],
    ];

    /// Deterministic test plane: smooth gradients, edges, noise, clipped and
    /// black regions.
    fn plane(width: usize, height: usize, bits: u32) -> Vec<u16> {
        let max = (1u32 << bits) - 1;
        let mut state = 0x2545_f491_4f6c_dd1du64;
        (0..width * height)
            .map(|i| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                let (x, y) = ((i % width) as f64, (i / width) as f64);
                let base = 1024.0 + 3000.0 * (x / width as f64) + 800.0 * (y / 7.0).sin();
                let edge = if (x + 0.3 * y) % 97.0 < 40.0 {
                    2500.0
                } else {
                    0.0
                };
                let noise = (state % 200) as f64 - 100.0;
                let v = if y < 6.0 {
                    0.0
                } else if (x - 50.0).abs() < 8.0 && (y - 40.0).abs() < 8.0 {
                    max as f64
                } else {
                    base + edge + noise
                };
                v.clamp(0.0, max as f64) as u16
            })
            .collect()
    }

    #[test]
    fn synthetic_planes_round_trip() {
        for (width, height, strip, bits) in [(96, 48, 48, 14), (120, 36, 48, 14), (72, 30, 18, 14)]
        {
            let src = plane(width, height, bits);
            let stream = encode(&src, width, height, strip, &CFA, bits);
            let mut out = vec![0u16; width * height];
            let h = decode(&stream, &CFA, &mut out).unwrap();
            assert_eq!((h.width, h.height, h.bits), (width, height, bits));
            assert_eq!(h.strips, width.div_ceil(strip));
            assert!(
                out == src,
                "{width}x{height} strip {strip}: decoded plane differs"
            );
        }
    }

    #[test]
    fn escapes_and_full_range_round_trip() {
        // Alternating black and white sites force raw escape codes.
        let (width, height) = (48, 24);
        let src: Vec<u16> = (0..width * height)
            .map(|i| {
                if (i / 5 + i / width) % 2 == 0 {
                    0
                } else {
                    16383
                }
            })
            .collect();
        let stream = encode(&src, width, height, 24, &CFA, 14);
        let mut out = vec![0u16; width * height];
        decode(&stream, &CFA, &mut out).unwrap();
        assert!(out == src);
    }

    #[test]
    fn corrupted_streams_are_refused() {
        let (width, height) = (96, 48);
        let src = plane(width, height, 14);
        let stream = encode(&src, width, height, 48, &CFA, 14);
        let first = (HEADER_BYTES + 4 * 2).next_multiple_of(16);
        let strip0 = u32::from_be_bytes(stream[16..20].try_into().unwrap()) as usize;
        // The format carries no checksum. A flipped bit that desynchronizes
        // the code is refused; one inside a residual's low bits decodes with
        // changed values, which must never reproduce the original.
        let (mut refused, mut trials) = (0, 0);
        for at in (first..first + strip0 - 16).step_by(7) {
            for bit in [0u8, 3, 6] {
                let mut bad = stream.clone();
                bad[at] ^= 1 << bit;
                let mut out = vec![0u16; width * height];
                trials += 1;
                match decode(&bad, &CFA, &mut out) {
                    Err(Refusal::Corrupt(_)) => refused += 1,
                    Err(e) => panic!("unexpected refusal {e}"),
                    Ok(_) => assert!(out != src, "a flipped bit decoded to the original"),
                }
            }
        }
        assert!(
            refused * 100 >= trials * 60,
            "only {refused} of {trials} corruptions were refused"
        );

        let mut truncated = stream.clone();
        truncated.truncate(stream.len() - 40);
        let mut out = vec![0u16; width * height];
        assert!(matches!(
            decode(&truncated, &CFA, &mut out),
            Err(Refusal::Corrupt(_))
        ));

        let mut long = stream.clone();
        let len = u32::from_be_bytes(long[16..20].try_into().unwrap()) + 32;
        long[16..20].copy_from_slice(&len.to_be_bytes());
        long.extend_from_slice(&[0; 32]);
        assert!(matches!(
            decode(&long, &CFA, &mut out),
            Err(Refusal::Corrupt(_))
        ));
    }

    #[test]
    fn header_variants_are_refused() {
        let stream = encode(&plane(48, 12, 14), 48, 12, 24, &CFA, 14);
        let mut out = vec![0u16; 48 * 12];
        let mut lossy = stream.clone();
        lossy[2] = 0;
        let err = decode(&lossy, &CFA, &mut out).unwrap_err();
        assert!(matches!(err, Refusal::Unsupported(_)));
        assert!(err.to_string().contains("lossy"), "{err}");
        let mut bayer = stream.clone();
        bayer[3] = 0;
        assert!(matches!(
            decode(&bayer, &CFA, &mut out),
            Err(Refusal::Unsupported(_))
        ));
        let mut signature = stream.clone();
        signature[0] = 0;
        assert!(matches!(
            decode(&signature, &CFA, &mut out),
            Err(Refusal::Corrupt(_))
        ));
        let mut groups = stream;
        groups[15] ^= 1;
        assert!(matches!(
            decode(&groups, &CFA, &mut out),
            Err(Refusal::Corrupt(_))
        ));
    }
}
