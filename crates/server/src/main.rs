//! Musik Faktory: a kid-friendly music studio on top of the ElevenLabs Music
//! API, with parent-controlled spending limits.

mod api;
mod auth;
mod budget;
mod eleven;
mod store;
mod web;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(name = "musik-faktory", version, about = "Kid-friendly music studio for the ElevenLabs Music API, with parental spending limits")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,

    /// Port to listen on / to open.
    #[arg(long, global = true, env = "MUSIK_PORT", default_value_t = 8787)]
    port: u16,

    /// Where tracks, audio and settings are stored.
    #[arg(long, global = true, env = "MUSIK_DATA_DIR")]
    data_dir: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Run the server (default).
    Serve {
        /// Address to bind. Use 0.0.0.0 to reach it from other devices at home.
        #[arg(long, env = "MUSIK_BIND", default_value = "127.0.0.1")]
        bind: String,
        /// Fake ElevenLabs: makes simple synthesized songs, costs nothing.
        #[arg(long, env = "MUSIK_MOCK")]
        mock: bool,
        /// How long a mock generation takes.
        #[arg(long, default_value_t = 3000)]
        mock_delay_ms: u64,
        /// Extra host names the server answers to (e.g. "music.example.com",
        /// "*.example.com"). IP addresses, localhost, this machine's own name
        /// and its Tailscale MagicDNS names always work. Names in the
        /// MUSIK_ALLOWED_HOSTS variable (comma-separated) are added too.
        #[arg(long = "allowed-host", value_delimiter = ',')]
        allowed_hosts: Vec<String>,
    },
    /// Start the server in the background if needed and open the app in the browser.
    Open,
    /// Forget the parent password (the next visit to the Parent page sets a new one).
    ResetPassword,
    /// Save the ElevenLabs API key (alternative to the Parents page). Run it
    /// while the server is stopped. Without KEY it asks for it.
    SetApiKey {
        key: Option<String>,
    },
}

fn default_data_dir() -> PathBuf {
    dirs::data_dir().map(|d| d.join("musik-faktory")).unwrap_or_else(|| PathBuf::from("musik-data"))
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let data_dir = cli.data_dir.clone().unwrap_or_else(default_data_dir);
    match cli.cmd.unwrap_or(Cmd::Serve { bind: "127.0.0.1".into(), mock: false, mock_delay_ms: 3000, allowed_hosts: vec![] }) {
        Cmd::Serve { bind, mock, mock_delay_ms, allowed_hosts } => serve(&bind, cli.port, data_dir, mock, mock_delay_ms, allowed_hosts).await,
        Cmd::Open => open(cli.port, data_dir),
        Cmd::SetApiKey { key } => set_api_key(cli.port, &data_dir, key).await,
        Cmd::ResetPassword => {
            let (store, mut db) = store::Store::open(&data_dir)?;
            db.admin_password_hash = None;
            store.save(&db)?;
            println!("Parent password removed. Open the Parent page to choose a new one.");
            println!("(Restart the server if it is running.)");
            Ok(())
        }
    }
}

async fn set_api_key(port: u16, data_dir: &std::path::Path, key: Option<String>) -> Result<()> {
    if healthy(port) {
        anyhow::bail!(
            "The server is running. Set the key on the 🔒 Parents page instead, or stop the server first (systemctl --user stop musik-faktory)."
        );
    }
    let key = match key {
        Some(k) => k,
        None => {
            print!("Paste your ElevenLabs API key and press Enter: ");
            std::io::stdout().flush()?;
            let mut line = String::new();
            std::io::stdin().read_line(&mut line)?;
            line
        }
    };
    let key = key.trim().to_string();
    if !api::looks_like_api_key(&key) {
        anyhow::bail!("That doesn't look like an ElevenLabs API key.");
    }
    match eleven::Backend::real().subscription(&key).await {
        Ok(sub) => println!("Key works. Plan: {}, credits used: {} of {}.", sub.tier, sub.credits_used, sub.credits_limit),
        Err(e) if e.is_invalid_key() => anyhow::bail!("ElevenLabs says this key is invalid. Nothing was saved."),
        Err(e) if matches!(e.http_status, Some(401) | Some(403)) => println!(
            "Note: {} (fine if the key is limited to Music only; costs are then estimated, not measured).",
            e.message
        ),
        Err(e) => println!("Note: could not test the key now: {}", e.message),
    }
    let (store, mut db) = store::Store::open(data_dir)?;
    db.api_key = Some(key);
    store.save(&db)?;
    println!("Saved in {}", data_dir.join("db.json").display());
    Ok(())
}

async fn serve(bind: &str, port: u16, data_dir: PathBuf, mock: bool, mock_delay_ms: u64, mut allowed_hosts: Vec<String>) -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "musik_faktory=info,tower_http=warn".into()))
        .init();

    let (store, mut db) = store::Store::open(&data_dir)?;
    let lost = api::recover_interrupted(&mut db, &store);
    if lost > 0 {
        tracing::warn!("{lost} generation(s) were interrupted by the last shutdown");
        store.save(&db)?;
    }
    let backend = if mock { eleven::Backend::mock(mock_delay_ms) } else { eleven::Backend::real() };
    let env_api_key = std::env::var("ELEVENLABS_API_KEY").ok().map(|k| k.trim().to_string()).filter(|k| !k.is_empty());
    if let Some(k) = &env_api_key {
        if !api::looks_like_api_key(k) {
            anyhow::bail!("ELEVENLABS_API_KEY doesn't look like an API key (unexpected characters or length)");
        }
    }
    // The variable adds to the flag (clap alone would ignore it when the flag is given).
    if let Ok(extra) = std::env::var("MUSIK_ALLOWED_HOSTS") {
        allowed_hosts.extend(extra.split(',').map(|h| h.trim().to_string()).filter(|h| !h.is_empty()));
    }
    // This machine's own name works too ("musicbox", "musicbox.local").
    if let Ok(h) = std::fs::read_to_string("/etc/hostname") {
        let h = h.trim().to_string();
        if !h.is_empty() {
            allowed_hosts.push(format!("{h}.local"));
            allowed_hosts.push(h);
        }
    }
    let tailscale = api::detect_tailscale().await;
    let mut known: Vec<String> = vec!["localhost".into()];
    known.extend(allowed_hosts.iter().cloned());
    let mut seen = std::collections::HashSet::new();
    known.retain(|h| seen.insert(h.to_ascii_lowercase()));
    tracing::info!("answering to: {} and any IP address", known.join(", "));
    tracing::info!("Tailscale: {}", tailscale.describe());
    let ts_names = match &tailscale {
        api::Tailscale::Names(n) => n.clone(),
        _ => Vec::new(),
    };
    let state = Arc::new(api::AppState {
        db: Mutex::new(db),
        store,
        backend,
        sessions: Default::default(),
        env_api_key,
        login_gate: Arc::new(tokio::sync::Semaphore::new(1)),
        recording: Default::default(),
        allowed_hosts,
        tailscale_hosts: std::sync::Mutex::new(ts_names),
    });
    api::spawn_tailscale_refresher(state.clone(), tailscale);

    let app = api::router(state.clone())
        .fallback(web::static_handler)
        .layer(axum::middleware::from_fn_with_state(state, api::host_guard))
        .layer(tower_http::trace::TraceLayer::new_for_http());

    let addr: SocketAddr = format!("{bind}:{port}").parse().with_context(|| format!("bad bind address {bind}:{port}"))?;
    let listener = tokio::net::TcpListener::bind(addr).await.with_context(|| format!("cannot listen on {addr}"))?;
    tracing::info!("Musik Faktory on http://{addr}  (data: {}){}", data_dir.display(), if mock { "  [MOCK MODE]" } else { "" });
    axum::serve(listener, app).with_graceful_shutdown(shutdown_signal()).await?;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = term => {} }
}

fn healthy(port: u16) -> bool {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut s) = TcpStream::connect_timeout(&addr, Duration::from_millis(300)) else { return false };
    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
    if s.write_all(b"GET /api/health HTTP/1.0\r\nHost: localhost\r\n\r\n").is_err() {
        return false;
    }
    let mut buf = String::new();
    let _ = s.read_to_string(&mut buf);
    buf.starts_with("HTTP/1.") && buf.contains(" 200 ")
}

/// Used by the desktop launcher (ChromeOS Linux / any Linux desktop).
fn open(port: u16, data_dir: PathBuf) -> Result<()> {
    if !healthy(port) {
        let via_systemd = std::process::Command::new("systemctl")
            .args(["--user", "start", "musik-faktory.service"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !via_systemd {
            let exe = std::env::current_exe()?;
            let mut cmd = std::process::Command::new(exe);
            cmd.arg("--port")
                .arg(port.to_string())
                .arg("--data-dir")
                .arg(&data_dir)
                .arg("serve")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                cmd.process_group(0);
            }
            cmd.spawn().context("starting the server")?;
        }
        for _ in 0..50 {
            if healthy(port) {
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    let url = format!("http://localhost:{port}/");
    let opened = ["xdg-open", "garcon-url-handler", "sensible-browser", "open"]
        .iter()
        .any(|b| std::process::Command::new(b).arg(&url).status().map(|s| s.success()).unwrap_or(false));
    if !opened {
        println!("Open {url} in your browser.");
    }
    Ok(())
}
