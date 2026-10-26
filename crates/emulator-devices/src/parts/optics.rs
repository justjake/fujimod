//! The physical lens wires: focus and aperture as the lens unit drives them,
//! and the focus-axis position its motor reports.
use std::sync::{Arc, Mutex};

/// The lens focus distance the image projection reads.
pub type FocusHandle = Arc<Mutex<f64>>;
/// The lens aperture the exposure reads.
pub type ApertureHandle = Arc<Mutex<f64>>;

/// The focus-axis motor position.
#[derive(Clone, Default)]
pub struct Position(Arc<Mutex<i64>>);
impl Position {
    pub fn value(&self) -> i64 {
        *self.0.lock().unwrap()
    }
    pub fn publish(&self, value: i64) {
        *self.0.lock().unwrap() = value;
    }
}
