//! Fixed-point film processing over caller-supplied tables and coefficients.
use pyo3::prelude::*;
use pyo3::exceptions::PyValueError;

#[pyclass(frozen, module = "fujimod._native.emulator")]
pub(crate) struct FilmStage {
    linear: Vec<u16>,
    gain: Vec<i64>,
    matrix: [i64; 9],
    curve: Vec<i64>,
    chroma: [i64; 4],
    bounds: [i64; 4],
    bands: [(i64, i64); 5],
}

#[pymethods]
impl FilmStage {
    #[new]
    fn new(linear: Vec<u16>, gain: Vec<i64>, matrix: [i64; 9], curve: Vec<i64>,
           chroma: [i64; 4], bounds: [i64; 4], bands: [(i64, i64); 5]) -> PyResult<Self> {
        if linear.len() != 768 || linear.iter().any(|&x| x > 16383)
            || gain.len() != 4096 || gain.iter().any(|&x| !(0..=8191).contains(&x))
            || curve.len() != 4096 || curve.iter().any(|&x| !(0..=16383).contains(&x))
            || matrix.iter().chain(chroma.iter()).any(|&x| !(-4096..=4095).contains(&x))
            || bounds.iter().any(|&x| !(0..=255).contains(&x))
            || bounds.windows(2).any(|p| p[0] > p[1])
            || bands.iter().any(|&(shift,gain)| !(-1024..=1023).contains(&shift) || !(0..=1023).contains(&gain)) {
            return Err(PyValueError::new_err("Invalid film tables or coefficients"));
        }
        Ok(Self {linear,gain,matrix,curve,chroma,bounds,bands})
    }
}

impl FilmStage {
    fn pixel(&self, rgb: [u8;3]) -> [i64;3] {
        let w = std::array::from_fn::<_,3,_>(|i| self.linear[i*256+rgb[i] as usize] as i64);
        let brightness = (3*(w[0]>>2)+7*(w[1]>>2)+6*(w[2]>>2))>>4;
        let gain = self.gain[brightness as usize];
        let x = w.map(|v| ((v*gain)>>11).min(4095));
        let color = std::array::from_fn::<_,3,_>(|i| {
            let value = (((self.matrix[i*3]*x[0])>>7)+((self.matrix[i*3+1]*x[1])>>7)
                +((self.matrix[i*3+2]*x[2])>>7)+4)>>3;
            self.curve[value.clamp(0,4095) as usize]>>6
        });
        let [r,g,b] = color;
        let y = (((306*r)>>7)+((601*g)>>7)+((117*b)>>7)+4)>>3;
        let [c0,c1,c2,c3] = self.chroma;
        let cb = (((((c2*(b-y))>>8)+((c3*(r-y))>>8))*578)>>10).clamp(-512,511);
        let cr = (((((c0*(r-y))>>8)+((c1*(b-y))>>8))*730)>>10).clamp(-512,511);
        let band = self.bounds.iter().take_while(|&&v| v<=y).count();
        let (shift,gain) = self.bands[band];
        let factor = gain+((shift*y)>>8);
        [y,(128+((cb*factor+512)>>10)).clamp(0,255),(128+((cr*factor+512)>>10)).clamp(0,255)]
    }

    pub(crate) fn color(&self, rgb: [u8;3]) -> [u8;3] { self.pixel(rgb).map(|v|v as u8) }

    pub(crate) fn pack(a: [u8;3], b: [u8;3]) -> [u8;4] {
        let a=a.map(i64::from); let b=b.map(i64::from);
        // numpy.round's ties-to-even for the shared 4:2:2 chroma sample.
        let mean=|a,b| {let sum=a+b; let half=sum/2; (half+((sum&1)&(half&1))) as u8};
        [a[0] as u8,b[0] as u8,mean(a[1],b[1]),mean(a[2],b[2])]
    }
}
