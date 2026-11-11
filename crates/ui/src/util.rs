//! Small browser helpers.

use js_sys::{Date, Math};
use web_sys::window;

pub fn confirm(msg: &str) -> bool {
    window().and_then(|w| w.confirm_with_message(msg).ok()).unwrap_or(false)
}

pub fn random_u32(max: u32) -> u32 {
    (Math::random() * max as f64) as u32
}

/// "12 Nov, 14:05" in the browser's locale.
pub fn fmt_datetime(rfc3339: &str) -> String {
    let d = Date::new(&rfc3339.into());
    if d.get_time().is_nan() {
        return rfc3339.to_string();
    }
    let opts = js_sys::Object::new();
    for (k, v) in [("day", "numeric"), ("month", "short"), ("hour", "2-digit"), ("minute", "2-digit")] {
        let _ = js_sys::Reflect::set(&opts, &k.into(), &v.into());
    }
    d.to_locale_string("default", &opts).into()
}

/// "Monday 17 Nov".
pub fn fmt_day(rfc3339: &str) -> String {
    let d = Date::new(&rfc3339.into());
    if d.get_time().is_nan() {
        return rfc3339.to_string();
    }
    let opts = js_sys::Object::new();
    for (k, v) in [("weekday", "long"), ("day", "numeric"), ("month", "short")] {
        let _ = js_sys::Reflect::set(&opts, &k.into(), &v.into());
    }
    d.to_locale_date_string("default", &opts).into()
}

/// Stable colour for a version branch.
pub fn branch_color(branch: &str) -> String {
    if branch.is_empty() {
        return "var(--primary)".into();
    }
    let h = branch.bytes().fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
    format!("hsl({}, 70%, 48%)", (h * 47) % 360)
}
