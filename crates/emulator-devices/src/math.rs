//! CPython 3.12+ float semantics the reference Python models rely on.
//!
//! `math.hypot` and `math.dist` do not call the C library's `hypot`: they use
//! CPython's correctly rounded `vector_norm` (Modules/mathmodule.c). The
//! builtin `sum` of floats uses Neumaier compensated summation. Ports of
//! Python models must use these to produce the same bits.

#[derive(Clone, Copy)]
struct DoubleLength {
    hi: f64,
    lo: f64,
}

fn dl_mul(x: f64, y: f64) -> DoubleLength {
    let z = x * y;
    DoubleLength { hi: z, lo: x.mul_add(y, -z) }
}

fn dl_fast_sum(a: f64, b: f64) -> DoubleLength {
    let x = a + b;
    let z = x - a;
    DoubleLength { hi: x, lo: b - z }
}

fn frexp_exponent(x: f64) -> i32 {
    // C frexp: x = m * 2^e with 0.5 <= |m| < 1 (x finite, nonzero, normal).
    let bits = x.to_bits();
    let exponent = ((bits >> 52) & 0x7ff) as i32;
    if exponent == 0 {
        // Subnormal: normalize first.
        let normalized = x * f64::from_bits(0x4350000000000000); // 2^54
        return frexp_exponent(normalized) - 54;
    }
    exponent - 1022
}

fn ldexp(x: f64, e: i32) -> f64 {
    let mut value = x;
    let mut e = e;
    while e > 1000 {
        value *= f64::from_bits(((1000 + 1023) as u64) << 52);
        e -= 1000;
    }
    while e < -1000 {
        value *= f64::from_bits(((-1000 + 1023) as u64) << 52);
        e += 1000;
    }
    value * f64::from_bits(((e + 1023) as u64) << 52)
}

/// CPython's `vector_norm(n, vec, max, found_nan)`; `vec` holds |x|.
fn vector_norm(vec: &mut [f64], max: f64, found_nan: bool) -> f64 {
    if max.is_infinite() {
        return max;
    }
    if found_nan {
        return f64::NAN;
    }
    if max == 0.0 || vec.len() <= 1 {
        return max;
    }
    let max_e = frexp_exponent(max);
    if max_e < -1023 {
        // ldexp(1.0, -max_e) would overflow: scale subnormals up first.
        for x in vec.iter_mut() {
            *x /= f64::MIN_POSITIVE;
        }
        return f64::MIN_POSITIVE * vector_norm(vec, max / f64::MIN_POSITIVE, found_nan);
    }
    let scale = ldexp(1.0, -max_e);
    let (mut csum, mut frac1, mut frac2) = (1.0f64, 0.0f64, 0.0f64);
    for x in vec.iter() {
        let x = x * scale; // lossless scaling
        let pr = dl_mul(x, x); // lossless squaring
        let sm = dl_fast_sum(csum, pr.hi); // lossless addition
        csum = sm.hi;
        frac1 += pr.lo; // lossy addition
        frac2 += sm.lo; // lossy addition
    }
    let mut h = (csum - 1.0 + (frac1 + frac2)).sqrt();
    let pr = dl_mul(-h, h);
    let sm = dl_fast_sum(csum, pr.hi);
    csum = sm.hi;
    frac1 += pr.lo;
    frac2 += sm.lo;
    let x = csum - 1.0 + (frac1 + frac2);
    h += x / (2.0 * h); // differential correction
    h / scale
}

/// `math.hypot(*coordinates)`.
pub fn hypot(coordinates: &[f64]) -> f64 {
    let mut vec: Vec<f64> = coordinates.iter().map(|x| x.abs()).collect();
    let found_nan = vec.iter().any(|x| x.is_nan());
    let max = vec.iter().fold(0.0f64, |m, &x| if x > m { x } else { m });
    vector_norm(&mut vec, max, found_nan)
}

/// `math.hypot(x, y)`.
pub fn hypot2(x: f64, y: f64) -> f64 {
    hypot(&[x, y])
}

/// `math.dist(p, q)` for 2-D points.
pub fn dist2(p: (f64, f64), q: (f64, f64)) -> f64 {
    hypot(&[p.0 - q.0, p.1 - q.1])
}

/// The builtin `sum(values)` of floats (start 0): the first value is added
/// to the int 0, the rest by Neumaier summation, and the compensation is
/// applied at the end when finite and nonzero. An empty sum is the int 0.
pub fn sum(values: impl IntoIterator<Item = f64>) -> f64 {
    let mut iter = values.into_iter();
    let Some(first) = iter.next() else { return 0.0 };
    let mut result = 0.0 + first; // int 0 + float: -0.0 becomes 0.0
    let mut c = 0.0f64;
    for x in iter {
        let t = result + x;
        if result.abs() >= x.abs() {
            c += (result - t) + x;
        } else {
            c += (x - t) + result;
        }
        result = t;
    }
    if c != 0.0 && c.is_finite() {
        result += c;
    }
    result
}

unsafe extern "C" {
    #[link_name = "pow"]
    fn libm_pow(x: f64, y: f64) -> f64;
    #[link_name = "cos"]
    fn libm_cos(x: f64) -> f64;
    #[link_name = "sin"]
    fn libm_sin(x: f64) -> f64;
}

/// Python `x ** y` for floats: the C library's `pow`. Its result is not
/// always `x * x` for `y == 2` (macOS libm, about 0.1% of values), so the
/// exponent is hidden from LLVM, which would otherwise rewrite the call.
pub fn pow(x: f64, y: f64) -> f64 {
    unsafe { libm_pow(x, std::hint::black_box(y)) }
}

/// `math.cos` and `math.sin`: separate C library calls (LLVM would fuse a
/// sin/cos pair into `__sincos_stret`).
pub fn cos(x: f64) -> f64 {
    unsafe { libm_cos(std::hint::black_box(x)) }
}

pub fn sin(x: f64) -> f64 {
    unsafe { libm_sin(std::hint::black_box(x)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hypot_matches_simple_cases() {
        assert_eq!(hypot2(3.0, 4.0), 5.0);
        assert_eq!(hypot2(0.0, 0.0), 0.0);
        assert_eq!(hypot2(-2.0, 0.0), 2.0);
    }

    #[test]
    fn sum_compensates() {
        assert_eq!(sum([1e16, 1.0, -1e16]), 1.0);
        assert_eq!(sum([-0.0]), 0.0);
        assert!(sum([-0.0]).is_sign_positive());
    }
}
