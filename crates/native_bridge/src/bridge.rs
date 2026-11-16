mod protocol;
mod state;
mod swift;

use std::io::{self, BufRead, Write};

use gpui::{App, AsyncApp};
use protocol::Request;
use state::Bridge;

/// Runs the headless Zed engine on this process's main thread, driven by
/// newline-delimited JSON commands read from stdin, with one JSON response
/// per line written to stdout. Must be called from the process main thread:
/// GPUI's macOS backend panics if constructed off it.
pub fn run() {
    env_logger::init();
    // Downloaded language servers and Node live under the app's own folder, apart from any
    // installed Zed's, so neither upgrades or deletes the other's.
    if let Some(home) = std::env::var_os("HOME") {
        let data_dir = std::path::Path::new(&home)
            .join("Library/Application Support/Pitsi Studio/Zed");
        match std::fs::create_dir_all(&data_dir) {
            Ok(()) => {
                paths::set_custom_data_dir(&data_dir.to_string_lossy());
            }
            Err(error) => log::error!("couldn't create {}: {error}", data_dir.display()),
        }
    }
    let (line_tx, line_rx) = async_channel::unbounded::<String>();
    std::thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            match line {
                Ok(line) if !line.trim().is_empty() => {
                    if line_tx.send_blocking(line).is_err() {
                        break;
                    }
                }
                Ok(_) => continue,
                Err(_) => break,
            }
        }
    });

    let app = gpui_platform::headless();
    app.run(move |cx: &mut App| {
        let bridge = Bridge::new(cx);
        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Ok(line) = line_rx.recv().await {
                let response = match serde_json::from_str::<Request>(&line) {
                    Ok(request) => bridge.handle(request, cx).await,
                    Err(error) => serde_json::json!({ "err": error.to_string() }),
                };
                let mut stdout = io::stdout();
                if writeln!(stdout, "{response}").is_err() || stdout.flush().is_err() {
                    break;
                }
            }
        })
        .detach();
    });
}
