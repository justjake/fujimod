//! FFF24000 extractor, result curve at FFF25004. The U16 paired SAD and
//! packed12 interleaved-aperture SAD are declared numerical laws, not optical
//! calibration. Only register-addressed rows are read, and only IRQ33 rises.
use fujimod_emulator_imaging::exr2_isp::{dma, due};
use fujimod_emulator_devices::exr2::irq_routes::{Irq, IrqRoutes};
use fujimod_emulator_image_processing::analysis::sad_curve;
use fujimod_emulator_image_processing::raw::{Packing, RawView};
use fujimod_emulator_devices::{Cx, Device};
use fujimod_emulator_imaging::exr2_isp::options::phase_detection::{Options, Arithmetic};

const BASE: u64 = 0xfff24000;
const RESULT: u64 = 0xfff25004;
const MAX_SAMPLES: usize = 65536;
pub struct Wiring {
    pub routes: IrqRoutes,
}
#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum State {
    Idle,
    Pending,
    Delivered,
    Stopping,
    Ready,
    Observed,
}
#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
struct Measurement {
    due: u64,
    route: Irq,
    radius: usize,
    #[serde(with = "fujimod_emulator_devices::checkpoint::array")]
    curve: [u32; 41],
    scalars: [u32; 2],
    totals: [u32; 2],
}
pub struct PhaseDetection {
    options: Options,
    wiring: Wiring,
    registers: [u32; 52],
    state: State,
    pending: Option<Measurement>,
    result: Option<Measurement>,
    stop_due: Option<u64>,
    planes: [Vec<u16>; 2],
    row: Vec<u8>,
}
impl PhaseDetection {
    pub fn new(options: Options, wiring: Wiring) -> Result<Self, String> {
        if options.data_ticks == 0 || options.stop_ticks == 0 {
            return Err("phase detection clocks must be positive".into());
        }
        Ok(Self {
            options,
            wiring,
            registers: [0; 52],
            state: State::Idle,
            pending: None,
            result: None,
            stop_due: None,
            planes: [vec![0; MAX_SAMPLES], vec![0; MAX_SAMPLES]],
            row: vec![0; 32768],
        })
    }
    fn mask(o: usize) -> Option<u32> {
        Some(match o {
            4 => 0x30000080,
            8 | 0xc | 0x10 => 0x3fffffff,
            0x14 => 0x1fff,
            0x18 | 0x1c => 0x1fff1fff,
            0x28 => 0x3ff,
            0x2c => 0x1f310fff,
            0x40 | 0xa8 => 0x3fff,
            0xa0 | 0xa4 => u32::MAX,
            0xb0 => 0xf,
            0xc0 => 0x71ff71ff,
            _ => return None,
        })
    }
    fn offset(address: u64, size: u8) -> Result<usize, String> {
        let o = address.wrapping_sub(BASE);
        if size != 4 || address & 3 != 0 || o >= 0xd0 {
            return Err("phase detection requires an aligned register word".into());
        }
        Ok(o as usize)
    }
    fn sample(&mut self, cx: &Cx<'_>) -> Result<Measurement, String> {
        let r = &self.registers;
        let word = r[0x2c / 4];
        let aperture = self.options.arithmetic == Arithmetic::ApertureSad;
        if self.options.arithmetic == Arithmetic::Disabled {
            return Err("PDAF measurement needs a declared arithmetic law".into());
        }
        let stride = r[0x14 / 4] as usize;
        let x0 = (r[0x18 / 4] & 0x1fff) as usize;
        let y0 = (r[0x18 / 4] >> 16) as usize;
        let x1 = (r[0x1c / 4] & 0x1fff) as usize;
        let y1 = (r[0x1c / 4] >> 16) as usize;
        let left_p = ((word >> 16) & 1) as usize;
        let right_p = ((word >> 20) & 1) as usize;
        let xstep = ((word & 15) as usize + 1) << if aperture { left_p } else { 0 };
        let ystep = (((word >> 4) & 15) as usize + ((word >> 8) & 15) as usize + 2)
            >> if aperture { left_p } else { 0 };
        let radius_word = (word >> 24) as usize & 31;
        let radius = if aperture {
            radius_word
        } else {
            radius_word / 2
        };
        if stride == 0
            || stride & 15 != 0
            || x1 < x0
            || y1 < y0
            || x1 + usize::from(aperture) >= stride
            || radius > 20
            || aperture && word & 0x200000 != 0
            || !aperture
                && (word & 0x310000 != 0
                    || radius_word & 1 != 0
                    || r[1] & 0x30000080 != 0x80
                    || r[8 / 4] == r[0xc / 4])
        {
            return Err("unsupported phase detection format, selectors or ROI".into());
        }
        let width = (x1 - x0) / xstep + 1;
        let height = (y1 - y0) / ystep + 1;
        let samples = width * height;
        if width <= 2 * radius || samples > MAX_SAMPLES || aperture && r[0x28 / 4] as usize != width
        {
            return Err("phase ROI exceeds its sample or comparison bound".into());
        }
        let packed = aperture && r[1] & 0x80 != 0;
        let packing = if packed {
            Packing::Packed12
        } else {
            Packing::U16
        };
        let pitch = packing.row_bytes(stride);
        if 2 * pitch * height > dma::MAX_BYTES {
            return Err("phase source rows exceed DMA bound".into());
        }
        let phase = ((x0 / 3) ^ y0) & 1;
        let parity = if left_p != right_p {
            [phase, 1 - phase]
        } else {
            [left_p ^ phase, right_p ^ phase]
        };
        let mut totals = [0u64; 2];
        let mut scalars = [0u32; 2];
        for source in 0..2 {
            let pointer = r[2 + source] as u64;
            if pointer & 7 != 0 {
                return Err("unaligned phase source pointer".into());
            }
            let mut lo = u16::MAX;
            let mut hi = 0u16;
            let mut saturated = 0u32;
            for row in 0..height {
                let y = y0 + row * ystep;
                let address = pointer + (y * pitch) as u64;
                dma::physical(address, pitch)?;
                cx.ram_read(address, &mut self.row[..pitch])?;
                let decoded = RawView {
                    bytes: &self.row[..pitch],
                    width: stride,
                    height: 1,
                    pitch,
                    packing,
                };
                for col in 0..width {
                    let x = x0 + col * xstep;
                    let value = if aperture {
                        let c = (x & !1) | parity[source];
                        let (a, b) = if source == 1 && left_p == 1 {
                            (
                                (c as isize - 4).clamp(
                                    parity[source] as isize,
                                    (stride - 2 + parity[source]) as isize,
                                ) as usize,
                                (c as isize - 2).clamp(
                                    parity[source] as isize,
                                    (stride - 2 + parity[source]) as isize,
                                ) as usize,
                            )
                        } else {
                            (c, c)
                        };
                        let av = decoded.phase_sample(a, 0) as u32;
                        let bv = decoded.phase_sample(b, 0) as u32;
                        let sum = av + bv;
                        let half = sum / 2;
                        // NumPy rint's ties-to-even, expressed in integers.
                        (half + u32::from(sum & 1 != 0 && half & 1 != 0)) as u16
                    } else {
                        decoded.phase_sample(x, 0)
                    };
                    if value > 0x3fff {
                        return Err("phase sample exceeds U14".into());
                    }
                    self.planes[source][row * width + col] = value;
                    lo = lo.min(value);
                    hi = hi.max(value);
                    totals[source] += value as u64;
                    saturated += u32::from(value >= 0x3ff0);
                }
            }
            scalars[source] = if aperture {
                saturated
            } else {
                (hi - lo) as u32
            };
            if scalars[source] > 0x3fff || totals[source] > u32::MAX as u64 {
                return Err("phase scalar or total exceeds register width".into());
            }
        }
        let curve = sad_curve(&self.planes[0], &self.planes[1], width, height, radius)?;
        Ok(Measurement {
            due: due(cx.now(), self.options.data_ticks)?,
            route: self.wiring.routes.latch(0x33)?,
            radius,
            curve,
            scalars,
            totals: [totals[0] as u32, totals[1] as u32],
        })
    }
    fn run(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        if let Some(p) = self.pending.filter(|p| cx.now() >= p.due) {
            self.wiring.routes.validate(p.route)?;
            self.registers[0xac / 4] = p.scalars[0] | p.scalars[1] << 16;
            self.registers[0xb4 / 4] = p.totals[0];
            self.registers[0xb8 / 4] = p.totals[1];
            self.registers[0] = 1;
            self.result = Some(p);
            self.pending = None;
            self.state = State::Delivered;
            p.route.raise(cx)?;
        }
        if self.stop_due.is_some_and(|t| cx.now() >= t) {
            self.registers[0] = 1;
            self.state = State::Ready;
            self.stop_due = None;
        }
        Ok(())
    }
}
impl Device for PhaseDetection {
    fujimod_emulator_devices::checkpoint_fields!(#[serde(with="fujimod_emulator_devices::checkpoint::array")] registers: [u32;52], state: State, pending: Option<Measurement>, result: Option<Measurement>, stop_due: Option<u64>, planes: [Vec<u16>;2], #[serde(with = "fujimod_emulator_devices::checkpoint::bytes")] row: Vec<u8>);
    fn fingerprint(&self) -> Result<serde_json::Value, String> {
        let measurement = |p: Measurement| {
            serde_json::json!({"due":p.due,"route":p.route,"radius":p.radius,
            "curve":p.curve.as_slice(),"scalars":p.scalars,"totals":p.totals})
        };
        Ok(
            serde_json::json!({"registers":self.registers.as_slice(),"state":self.state,"pending":self.pending.map(measurement),
            "result":self.result.map(measurement),"stop_due":self.stop_due}),
        )
    }
    fn save(&self) -> serde_json::Value {
        serde_json::json!({"registers":self.registers.as_slice(),
            "pending_due":self.pending.map(|p|p.due),"stop_due":self.stop_due,"has_result":self.result.is_some()})
    }
    fn name(&self) -> &'static str {
        "phase-detection"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let result: Result<u64, String> = (|| {
            if size == 4 && address & 3 == 0 && (RESULT..RESULT + 41 * 4).contains(&address) {
                let p = self.result.ok_or("phase curve read precedes completion")?;
                let index = ((address - RESULT) / 4) as usize;
                if index.abs_diff(20) > p.radius {
                    return Err("phase curve read exceeds configured radius".into());
                }
                return Ok(p.curve[index] as u64);
            }
            let o = Self::offset(address, size)?;
            if o == 0 && self.state == State::Ready {
                self.state = State::Observed;
            }
            if o == 0 || Self::mask(o).is_some() || matches!(o, 0xac | 0xb4 | 0xb8 | 0xc8 | 0xcc) {
                return Ok(self.registers[o / 4] as u64);
            }
            Err("unknown phase result register".into())
        })();
        match result {
            Ok(v) => v,
            Err(e) => {
                cx.fail(e);
                0
            }
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let result: Result<(), String> = (|| {
            let o = Self::offset(address, size)?;
            let v = u32::try_from(value).map_err(|_| "phase word exceeds 32 bits")?;
            if o == 0 && v == 0 {
                if !matches!(self.state, State::Delivered | State::Observed) {
                    return Err("phase cleanup requires a completed result".into());
                }
                self.stop_due = Some(due(cx.now(), self.options.stop_ticks)?);
                self.state = State::Stopping;
            } else if o == 4 && v & 7 == 1 {
                if v & !0x30000081 != 0 {
                    return Err("unsupported phase start bits".into());
                }
                if matches!(self.state, State::Stopping | State::Ready) {
                    return Ok(());
                }
                if !matches!(self.state, State::Idle | State::Observed) {
                    return Err("previous phase result has not drained".into());
                }
                let old = self.registers[1];
                self.registers[1] = v & 0x30000080;
                let measurement = self.sample(cx);
                self.registers[1] = old;
                self.pending = Some(measurement?);
                self.registers[1] = v & 0x30000080;
                self.result = None;
                self.state = State::Pending;
                return Ok(());
            } else if self.state != State::Pending
                && (matches!(o, 0xc8 | 0xcc) && v == 1 || matches!(o, 0xac | 0xb4 | 0xb8) && v == 0)
            {
            } else if Self::mask(o).is_some_and(|m| v & !m == 0) {
                if matches!(
                    self.state,
                    State::Pending | State::Delivered | State::Stopping
                ) {
                    return Err("phase configuration changed before cleanup".into());
                }
                if o == 4 {
                    self.result = None;
                    self.state = State::Idle;
                }
            } else {
                return Err("unsupported phase register or mask".into());
            }
            self.registers[o / 4] = v;
            Ok(())
        })();
        if let Err(e) = result {
            cx.fail(e);
        }
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.pending
            .map(|p| p.due)
            .into_iter()
            .chain(self.stop_due)
            .min()
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if let Err(e) = self.run(cx) {
            cx.fail(e);
        }
    }
    fn event(&mut self, cx: &mut Cx<'_>, _: u64) {
        self.advance(cx);
    }
    fn pollable(&self, address: u64, size: u8) -> bool {
        address == BASE && size == 4 && self.state != State::Ready
    }
}
