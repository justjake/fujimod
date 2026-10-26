//! Pure, bounded JPEG scanline kernels. Firmware/device policy stays in Python.
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use std::ffi::{CStr, c_char, c_int, c_uint};

unsafe extern "C" {
    fn camera_jpeg_encode(
        source: *const u8,
        width: c_uint,
        height: c_uint,
        stride: usize,
        tables: *const c_uint,
        restart: c_uint,
        output: *mut *mut u8,
        length: *mut usize,
        message: *mut c_char,
    ) -> c_int;
    fn camera_jpeg_free(output: *mut u8);
    fn camera_jpeg_decode(
        encoded: *const u8,
        count: usize,
        width: c_uint,
        height: c_uint,
        pitch: usize,
        output: *mut u8,
        message: *mut c_char,
    ) -> c_int;
}

#[pyfunction]
pub fn jpeg_decode_yycbcr<'py>(
    py: Python<'py>,
    encoded: &[u8],
    width: u32,
    height: u32,
    pitch: usize,
    rows: usize,
) -> PyResult<Bound<'py, PyBytes>> {
    let capacity = pitch.checked_mul(rows).and_then(|n| n.checked_mul(2));
    if width < 2
        || width > 65535
        || width % 2 != 0
        || height == 0
        || height > 65535
        || u64::from(width) * u64::from(height) > 32_000_000
        || pitch != (width as usize + 15) & !15
        || rows != (height as usize + 7) & !7
        || capacity.is_none_or(|n| n > 64 * 1024 * 1024)
        || encoded.is_empty()
        || encoded.len() > 64 * 1024 * 1024
    {
        return Err(PyValueError::new_err(
            "JPEG decode geometry outside bounded class",
        ));
    }
    PyBytes::new_with(py, capacity.unwrap(), |output| {
        output.fill(0);
        let mut message = [0 as c_char; 200];
        let ok = unsafe {
            camera_jpeg_decode(
                encoded.as_ptr(),
                encoded.len(),
                width,
                height,
                pitch,
                output.as_mut_ptr(),
                message.as_mut_ptr(),
            )
        };
        if ok == 0 {
            Err(PyValueError::new_err(
                unsafe { CStr::from_ptr(message.as_ptr()) }
                    .to_string_lossy()
                    .into_owned(),
            ))
        } else {
            Ok(())
        }
    })
}

#[pyfunction]
pub fn jpeg_encode_yycbcr<'py>(
    py: Python<'py>,
    source: &[u8],
    width: u32,
    height: u32,
    stride: usize,
    tables: Vec<u32>,
    restart: u32,
) -> PyResult<Bound<'py, PyBytes>> {
    if width < 2
        || width > 65535
        || width % 2 != 0
        || height == 0
        || height > 65535
        || u64::from(width) * u64::from(height) > 32_000_000
        || stride < width as usize * 2
        || stride.checked_mul(height as usize) != Some(source.len())
        || source.len() > 128 * 1024 * 1024
        || restart > 65535
        || tables.len() != 128
        || tables.iter().any(|v| !(1..=255).contains(v))
    {
        return Err(PyValueError::new_err(
            "JPEG scanline geometry/tables outside bounded class",
        ));
    }
    let mut output = std::ptr::null_mut();
    let mut length = 0;
    let mut message = [0 as c_char; 200]; // JMSG_LENGTH_MAX is 200.
    // C owns its error jump and returns normally before Rust constructs bytes.
    let ok = unsafe {
        camera_jpeg_encode(
            source.as_ptr(),
            width,
            height,
            stride,
            tables.as_ptr(),
            restart,
            &mut output,
            &mut length,
            message.as_mut_ptr(),
        )
    };
    if ok == 0 {
        return Err(PyValueError::new_err(
            unsafe { CStr::from_ptr(message.as_ptr()) }
                .to_string_lossy()
                .into_owned(),
        ));
    }
    let result = PyBytes::new(py, unsafe { std::slice::from_raw_parts(output, length) });
    unsafe { camera_jpeg_free(output) };
    Ok(result)
}
