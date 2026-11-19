mod input;
mod keymap;
mod protocol;
mod qr;
mod server;
mod system;
mod tailscale;

use crate::server::{build_router, AppState};
use crate::tailscale::{PeerSummary, TailscaleInfo};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::process::{Command, Stdio};
use std::time::Duration;
use tokio::net::TcpListener;

const DEFAULT_PORT: u16 = 9880;

struct CliArgs {
    port: u16,
    token: Option<String>,
    no_auth: bool,
    use_magicdns: bool,
    no_browser: bool,
    dry_run: bool,
    allow_lan: bool,
    bind: Option<String>,
}

fn print_help() {
    println!(
        "\
tailscale-remote {} — Turn your phone into a Tailscale trackpad, keyboard & media remote

USAGE:
    tailscale-remote [OPTIONS]

OPTIONS:
    -p, --port <PORT>      TCP port to listen on (default: {DEFAULT_PORT})
        --token <TOKEN>    Use a custom pairing token instead of a random 12-char token (or set TSR_TOKEN env var)
        --no-auth          Disable pairing token authentication (still Tailscale-only)
        --use-magicdns     Embed MagicDNS hostname instead of Tailscale IPv4 in QR URL
        --no-browser       Print terminal QR only (alias: --cli)
        --cli              Alias for --no-browser
        --dry-run          Log input events without creating/emitting OS HID devices
        --allow-lan        Allow non-Tailscale LAN IPs (unsafe on untrusted networks)
        --bind <IP[:PORT]> Override or specify bind IP address (e.g. 0.0.0.0)
    -h, --help             Show this help message
    -V, --version          Show version information",
        env!("CARGO_PKG_VERSION")
    );
}

fn validate_custom_token(val: &str) -> Result<String, String> {
    if val.is_empty() {
        return Err("token cannot be empty".to_string());
    }
    if val.len() > 128 {
        return Err("token must be 128 characters or fewer".to_string());
    }
    if !val
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~'))
    {
        return Err(
            "token may only contain ASCII letters, digits, and '-', '_', '.', '~'".to_string(),
        );
    }
    Ok(val.to_string())
}

fn parse_args() -> Result<Option<CliArgs>, String> {
    let mut args = CliArgs {
        port: DEFAULT_PORT,
        token: None,
        no_auth: false,
        use_magicdns: false,
        no_browser: false,
        dry_run: false,
        allow_lan: false,
        bind: None,
    };

    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print_help();
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("tailscale-remote {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "-p" | "--port" => {
                let val = iter
                    .next()
                    .ok_or_else(|| format!("{arg} requires a port number"))?;
                args.port = val
                    .parse::<u16>()
                    .map_err(|_| format!("invalid port '{val}'"))?;
            }
            "--token" => {
                let val = iter
                    .next()
                    .ok_or_else(|| "--token requires a value".to_string())?;
                args.token = Some(validate_custom_token(&val).map_err(|e| format!("--token {e}"))?);
            }
            "--no-auth" => args.no_auth = true,
            "--use-magicdns" => args.use_magicdns = true,
            "--no-browser" | "--cli" => args.no_browser = true,
            "--dry-run" => args.dry_run = true,
            "--allow-lan" => args.allow_lan = true,
            "--bind" => {
                let val = iter
                    .next()
                    .ok_or_else(|| "--bind requires an IP or IP:PORT".to_string())?;
                args.bind = Some(val);
            }
            other => {
                return Err(format!("unknown argument '{other}'. Run with --help for usage."));
            }
        }
    }

    Ok(Some(args))
}

fn resolve_bind_addrs(args: &CliArgs, ts: &TailscaleInfo) -> Result<Vec<SocketAddr>, String> {
    let loopback = SocketAddr::from(([127, 0, 0, 1], args.port));

    if let Some(ref raw_bind) = args.bind {
        let addr: SocketAddr = if let Ok(sa) = raw_bind.parse::<SocketAddr>() {
            sa
        } else if let Ok(ip) = raw_bind.parse::<IpAddr>() {
            SocketAddr::new(ip, args.port)
        } else {
            return Err(format!("invalid --bind address '{raw_bind}'"));
        };

        if addr.ip().is_unspecified() {
            return Ok(vec![addr]);
        }
        if addr.ip().is_loopback() {
            return Ok(vec![addr]);
        }
        return Ok(vec![loopback, addr]);
    }

    let ts_ip: IpAddr = ts
        .ipv4
        .parse()
        .map_err(|e| format!("invalid Tailscale IPv4 '{}': {e}", ts.ipv4))?;

    if ts_ip.is_loopback() {
        Ok(vec![loopback])
    } else {
        Ok(vec![loopback, SocketAddr::new(ts_ip, args.port)])
    }
}

fn spawn_qr_browser(qr_url: String) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(160));
        if open_app_browser(&qr_url) {
            return;
        }
        eprintln!("[*] Could not auto-open browser window; open {qr_url} manually or scan the terminal QR above.");
    });
}

fn open_app_browser(url: &str) -> bool {
    #[cfg(target_os = "linux")]
    {
        let app_arg = format!("--app={url}");
        for browser in [
            "chromium",
            "chromium-browser",
            "google-chrome",
            "google-chrome-stable",
            "brave",
            "microsoft-edge",
        ] {
            if Command::new(browser)
                .args([&app_arg, "--window-size=560,780"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .is_ok()
            {
                return true;
            }
        }
        Command::new("xdg-open")
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .is_ok()
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        let app_arg = format!("--app={url}");
        for browser in ["msedge.exe", "chrome.exe"] {
            if Command::new(browser)
                .args([&app_arg, "--window-size=560,780"])
                .creation_flags(CREATE_NO_WINDOW)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .is_ok()
            {
                return true;
            }
        }
        Command::new("cmd")
            .args(["/C", "start", "", url])
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .is_ok()
    }

    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .is_ok()
    }

    #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
    {
        let _ = url;
        false
    }
}

#[tokio::main]
async fn main() {
    let args = match parse_args() {
        Ok(Some(parsed)) => parsed,
        Ok(None) => return,
        Err(err) => {
            eprintln!("Error: {err}");
            std::process::exit(2);
        }
    };

    let ts_info = match tailscale::detect_tailscale() {
        Ok(info) => info,
        Err(err) => {
            if args.allow_lan || args.bind.is_some() {
                eprintln!("[!] {err}");
                eprintln!("[!] Continuing in fallback mode because --allow-lan or --bind was provided.");
                let fallback_ip = args
                    .bind
                    .as_deref()
                    .and_then(|b| b.split(':').next())
                    .filter(|ip| *ip != "0.0.0.0" && *ip != "::")
                    .unwrap_or("127.0.0.1")
                    .to_string();
                let hostname = std::env::var("HOSTNAME")
                    .or_else(|_| std::env::var("COMPUTERNAME"))
                    .unwrap_or_else(|_| "media-pc".to_string());
                TailscaleInfo {
                    ipv4: fallback_ip,
                    ipv6: None,
                    hostname,
                    dns_name: None,
                    peers: HashMap::<String, PeerSummary>::new(),
                }
            } else {
                eprintln!("Error: {err}");
                std::process::exit(1);
            }
        }
    };

    let input_tx = match input::spawn_input_worker(args.dry_run) {
        Ok(tx) => tx,
        Err(err) => {
            eprintln!("Error: {err}");
            std::process::exit(1);
        }
    };

    let auth_enabled = !args.no_auth;
    let raw_token = match args.token.clone() {
        Some(t) => Some(t),
        None => match std::env::var("TSR_TOKEN") {
            Ok(env_t) if !env_t.is_empty() => {
                Some(validate_custom_token(&env_t).unwrap_or_else(|err| {
                    eprintln!("Error: invalid TSR_TOKEN environment variable: {err}");
                    std::process::exit(2);
                }))
            }
            _ => None,
        },
    };
    let token = if auth_enabled {
        let tok = raw_token.unwrap_or_else(|| qr::generate_token(12));
        if tok.len() < 8 {
            eprintln!("[WARN] Custom token is shorter than 8 characters; consider using a longer random token.");
        }
        tok
    } else {
        String::new()
    };

    let bind_addrs = match resolve_bind_addrs(&args, &ts_info) {
        Ok(addrs) => addrs,
        Err(err) => {
            eprintln!("Error: {err}");
            std::process::exit(2);
        }
    };

    let mut listeners = Vec::with_capacity(bind_addrs.len());
    for addr in &bind_addrs {
        match TcpListener::bind(addr).await {
            Ok(l) => listeners.push(l),
            Err(err) => {
                eprintln!("Error: failed to bind HTTP server on {addr}: {err}");
                std::process::exit(1);
            }
        }
    }

    let actual_port = listeners
        .first()
        .and_then(|l| l.local_addr().ok())
        .map(|a| a.port())
        .unwrap_or(args.port);

    let state = AppState::new(
        token,
        auth_enabled,
        args.allow_lan,
        args.dry_run,
        actual_port,
        args.use_magicdns,
        ts_info,
        input_tx,
    );

    let pairing_url = state.pairing_url();
    let local_qr_url = format!("http://127.0.0.1:{actual_port}/qr");

    println!();
    println!("================================================================");
    println!("               Tailscale Remote — Phone KVM Ready               ");
    println!("================================================================");
    println!("  Host        : {} ({})", state.tailscale.hostname, std::env::consts::OS);
    println!("  Tailscale   : {}", state.tailscale.ipv4);
    if let Some(ref dns) = state.tailscale.dns_name {
        println!("  MagicDNS    : {dns}");
    }
    println!(
        "  Input Mode  : {}",
        if args.dry_run {
            "DRY-RUN (logging events only)"
        } else {
            "Live Virtual HID"
        }
    );
    let access_mode = match (args.allow_lan, auth_enabled) {
        (false, true) => "Tailscale-Only + Token Auth",
        (false, false) => "Tailscale-Only (NO TOKEN AUTH — --no-auth)",
        (true, true) => "Tailscale + LAN (--allow-lan) + Token Auth",
        (true, false) => "Tailscale + LAN (--allow-lan, NO TOKEN AUTH)",
    };
    println!("  Access Mode : {access_mode}");
    if args.allow_lan {
        println!("  [WARN] --allow-lan is active: non-Tailscale local network clients can reach this port.");
    }
    if !auth_enabled {
        println!("  [WARN] --no-auth is active: any reachable client can control this host without a pairing token.");
    }
    println!("  Pairing URL : {pairing_url}");
    println!("  Local QR UI : {local_qr_url}");
    println!();

    match qr::generate_terminal_qr(&pairing_url) {
        Ok(term_qr) => {
            for line in term_qr.lines() {
                println!("  {line}");
            }
        }
        Err(err) => eprintln!("  [!] Could not render terminal QR: {err}"),
    }

    println!();
    println!("  Scan the QR code with your phone (connected to Tailscale).");
    println!("  Press Ctrl+C to stop.");
    println!();

    if !args.no_browser {
        spawn_qr_browser(local_qr_url);
    }

    let app = build_router(state.clone());

    let mut join_handles = Vec::with_capacity(listeners.len());
    for listener in listeners {
        let app_clone = app.clone();
        join_handles.push(tokio::spawn(async move {
            let _ = axum::serve(
                listener,
                app_clone.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(shutdown_signal())
            .await;
        }));
    }

    for handle in join_handles {
        let _ = handle.await;
    }

    let _ = state.input_tx.send(input::InputCommand::ReleaseAll).await;
    println!("\nShutting down Tailscale Remote.");
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
