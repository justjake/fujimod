//! Keep the camera window the shape of its contents: a 3:2 screen beside the
//! fixed-width controls panel, so the screen fills its area at every size.
use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
use objc2::{msg_send, sel};
use objc2_foundation::{NSRect, NSSize};
use super::{BAR, PANEL, SCREEN, window_width as width};

/// `windowWillResize:toSize:`: follow the edge the person drags most and
/// derive the other.
extern "C-unwind" fn will_resize(_: &AnyObject, _: Sel, window: &AnyObject, proposed: NSSize) -> NSSize {
    let frame: NSRect = unsafe { msg_send![window, frame] };
    let content: NSRect = unsafe { msg_send![window, contentRectForFrameRect: frame] };
    let chrome = NSSize::new(
        frame.size.width - content.size.width,
        frame.size.height - content.size.height,
    );
    let (w, h) = fit(
        (proposed.width - chrome.width, proposed.height - chrome.height),
        (content.size.width, content.size.height),
    );
    NSSize::new(w + chrome.width, h + chrome.height)
}

/// The content size for a proposed one: keep the dimension that moved most
/// and derive the other from it.
fn fit((w, h): (f64, f64), (current_w, current_h): (f64, f64)) -> (f64, f64) {
    if (w - current_w).abs() >= (h - current_h).abs() {
        (w, (w - PANEL) / SCREEN + BAR)
    } else {
        (width(h), h)
    }
}

/// Constrain live resizing of `ns_window`.
pub fn constrain(ns_window: *mut std::ffi::c_void) -> Result<(), String> {
    let window = unsafe { ns_window.cast::<AnyObject>().as_ref() }.ok_or("window has no NSWindow")?;
    let delegate: *mut AnyObject = unsafe { msg_send![window, delegate] };
    let delegate = unsafe { delegate.as_ref() }.ok_or("window has no delegate")?;
    let class: *const AnyClass = delegate.class();
    let imp: Imp = unsafe {
        std::mem::transmute::<
            extern "C-unwind" fn(&AnyObject, Sel, &AnyObject, NSSize) -> NSSize,
            Imp,
        >(will_resize)
    };
    // Adding fails only when the class already answers; the window toolkit
    // does not, so a refusal means another constraint owns resizing.
    let added = unsafe {
        objc2::ffi::class_addMethod(
            class.cast_mut(),
            sel!(windowWillResize:toSize:),
            imp,
            c"{CGSize=dd}@:@{CGSize=dd}".as_ptr(),
        )
    };
    if added.as_bool() {
        Ok(())
    } else {
        Err("the window delegate already constrains resizing".into())
    }
}

#[cfg(test)]
mod tests {
    use super::fit;

    #[test]
    fn keeps_the_screen_three_by_two_beside_the_panel_and_above_the_bar() {
        // Dragging mostly sideways keeps the width.
        assert_eq!(fit((1200.0, 760.0), (1440.0, 720.0)), (1200.0, 588.0));
        // Dragging mostly downward keeps the height.
        assert_eq!(fit((1460.0, 900.0), (1440.0, 720.0)), (1668.0, 900.0));
    }
}
