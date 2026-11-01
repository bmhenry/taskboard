//! `taskboard-web`: serves Taskboard's web UI and the API it runs on, over
//! the same task store as `tb` and the desktop app.
//!
//! Usage: `taskboard-web [--bind ADDR] [--static DIR] [DIR]` — DIR is the
//! project new tasks go in by default, as for `taskboard`.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use taskboard_core::{GlobalSettings, Store, project_path, project_root};
use taskboard_web::Config;

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Address to listen on. Keep it on localhost, and reach it from other
    /// machines through an SSH tunnel or Tailscale.
    #[arg(long, value_name = "ADDR", default_value = "127.0.0.1:7878")]
    bind: String,
    /// Serve the web UI from this directory (trunk's `dist/`).
    #[arg(long = "static", value_name = "DIR")]
    static_dir: Option<PathBuf>,
    /// The project new tasks go in by default. Without it, that's the git
    /// repo the current directory is in, if any (or the current directory,
    /// if it already has tasks).
    dir: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let named = cli.dir.is_some();
    let start = cli
        .dir
        .unwrap_or_else(|| std::env::current_dir().expect("current directory is accessible"));
    let store = match Store::open_default() {
        Ok(store) => store,
        Err(e) => {
            eprintln!("taskboard-web: couldn't open the task store: {e}");
            return ExitCode::FAILURE;
        }
    };
    let app = taskboard_web::router(Config {
        store,
        settings_path: GlobalSettings::default_path(),
        project: project_path(&project_root(&start)),
        named,
        static_dir: cli.static_dir,
    });
    let listener = match tokio::net::TcpListener::bind(&cli.bind).await {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("taskboard-web: couldn't listen on {}: {e}", cli.bind);
            return ExitCode::FAILURE;
        }
    };
    if let Ok(addr) = listener.local_addr() {
        eprintln!("taskboard-web: serving http://{addr}/");
    }
    // No graceful shutdown: event streams never end, so it would never finish.
    match axum::serve(listener, app).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("taskboard-web: {e}");
            ExitCode::FAILURE
        }
    }
}
