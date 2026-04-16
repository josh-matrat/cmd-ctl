mod app;
mod settings;
mod url_detect;

use anyhow::Result;

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    tracing::info!("Starting CMDCTL");

    // Start daemon if not already running.
    if cmdctl_daemon::daemon::is_running() {
        tracing::info!("Connecting to existing daemon");
    } else {
        tracing::info!("Starting daemon");
        let handle = cmdctl_daemon::daemon::start_background()?;
        // Intentionally leak the handle so the daemon keeps running
        // after the UI window closes. Sessions persist until the daemon
        // is explicitly shut down (via cmdctl-cli shutdown or process kill).
        std::mem::forget(handle);
    }

    // Give daemon a moment to bind the socket.
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Run the UI (blocking). When this returns, the window closed
    // but the daemon (and all sessions) keep running.
    app::run()
}
