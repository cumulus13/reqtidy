//! Optional desktop notifications (the `notify` cargo feature).

/// Shows a desktop notification. Failures are logged, never fatal.
#[cfg(feature = "notify")]
pub fn send(title: &str, body: &str) {
    if let Err(e) = notify_rust::Notification::new()
        .appname("reqtidy")
        .summary(title)
        .body(body)
        .show()
    {
        log::warn!("could not show desktop notification: {e}");
    }
}

/// Without the `notify` feature this only warns.
#[cfg(not(feature = "notify"))]
pub fn send(_title: &str, _body: &str) {
    log::warn!("--notify ignored: reqtidy was built without the `notify` feature");
}
