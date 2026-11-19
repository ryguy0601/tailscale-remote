use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

/// A customizable quick-launch tile displayed on the phone's Launch & Web deck.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShortcutItem {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub target: String,
    pub icon: String,
    pub color: String,
}

/// An available audio output endpoint (e.g., Speaker, TV HDMI, Headphones).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AudioOutput {
    pub id: String,
    pub name: String,
    pub active: bool,
}

/// Current host system audio volume, mute state, and available outputs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AudioState {
    pub volume: Option<u8>,
    pub muted: bool,
    pub outputs: Vec<AudioOutput>,
}

/// Active media session metadata retrieved from MPRIS / WinRT / AppleScript.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NowPlaying {
    pub status: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub player: Option<String>,
}

/// Tailscale peer summary with live TCP probe status for multi-PC switching.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TailnetPeerStatus {
    pub hostname: String,
    pub os: String,
    pub ip: String,
    pub dns_name: Option<String>,
    pub online: bool,
    pub remote_ready: bool,
    pub url: String,
    pub is_self: bool,
}

/// An open desktop window returned by the window switcher / close-under-cursor API.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WindowInfo {
    pub id: String,
    pub title: String,
    pub app: String,
    pub workspace: String,
    pub focused: bool,
    pub under_cursor: bool,
}

fn new_bg_cmd(program: &str) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Returns the default curated set of media PC & streaming shortcuts.
pub fn default_shortcuts() -> Vec<ShortcutItem> {
    vec![
        ShortcutItem {
            id: "youtube".into(),
            name: "YouTube".into(),
            kind: "url".into(),
            target: "https://www.youtube.com".into(),
            icon: "▶".into(),
            color: "#ef4444".into(),
        },
        ShortcutItem {
            id: "browser".into(),
            name: "Browser".into(),
            kind: "preset".into(),
            target: "browser".into(),
            icon: "🌐".into(),
            color: "#38bdf8".into(),
        },
        ShortcutItem {
            id: "netflix".into(),
            name: "Netflix".into(),
            kind: "url".into(),
            target: "https://www.netflix.com".into(),
            icon: "N".into(),
            color: "#e50914".into(),
        },
        ShortcutItem {
            id: "twitch".into(),
            name: "Twitch".into(),
            kind: "url".into(),
            target: "https://www.twitch.tv".into(),
            icon: "📺".into(),
            color: "#9146ff".into(),
        },
        ShortcutItem {
            id: "spotify".into(),
            name: "Spotify".into(),
            kind: "url".into(),
            target: "https://open.spotify.com".into(),
            icon: "♫".into(),
            color: "#1db954".into(),
        },
        ShortcutItem {
            id: "plex".into(),
            name: "Plex".into(),
            kind: "url".into(),
            target: "https://app.plex.tv".into(),
            icon: "◈".into(),
            color: "#e5a00d".into(),
        },
        ShortcutItem {
            id: "disney".into(),
            name: "Disney+".into(),
            kind: "url".into(),
            target: "https://www.disneyplus.com".into(),
            icon: "✦".into(),
            color: "#2563eb".into(),
        },
        ShortcutItem {
            id: "prime".into(),
            name: "Prime Video".into(),
            kind: "url".into(),
            target: "https://www.amazon.com/gp/video/storefront".into(),
            icon: "P".into(),
            color: "#0ea5e9".into(),
        },
        ShortcutItem {
            id: "steam".into(),
            name: "Steam Big Picture".into(),
            kind: "preset".into(),
            target: "steam".into(),
            icon: "🎮".into(),
            color: "#6366f1".into(),
        },
    ]
}

/// Resolves the persistent JSON path for user shortcuts.
pub fn shortcuts_config_path() -> PathBuf {
    #[cfg(windows)]
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));

    #[cfg(not(windows))]
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));

    base.join("tailscale-remote").join("shortcuts.json")
}

/// Loads saved shortcuts from disk, falling back to `default_shortcuts()`.
pub fn load_shortcuts() -> Vec<ShortcutItem> {
    let path = shortcuts_config_path();
    if let Ok(raw) = std::fs::read_to_string(&path) {
        if let Ok(items) = serde_json::from_str::<Vec<ShortcutItem>>(&raw) {
            if !items.is_empty() {
                return items;
            }
        }
    }
    default_shortcuts()
}

/// Persists the shortcuts list to disk atomically enough for local config.
pub fn save_shortcuts(items: &[ShortcutItem]) -> Result<(), String> {
    let path = shortcuts_config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create shortcuts dir: {e}"))?;
    }
    let json = serde_json::to_string_pretty(items)
        .map_err(|e| format!("failed to serialize shortcuts: {e}"))?;
    std::fs::write(&path, json).map_err(|e| format!("failed to write {}: {e}", path.display()))
}

/// Validates and normalizes a user-supplied URL, allowing only `http://` and `https://`.
pub fn normalize_http_url(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("URL cannot be empty".to_string());
    }
    if trimmed.len() > 2048 {
        return Err("URL exceeds maximum length (2048 bytes)".to_string());
    }
    if trimmed.chars().any(|c| {
        c.is_control()
            || c.is_whitespace()
            || matches!(
                c,
                '"' | '\'' | '\\' | '<' | '>' | '`' | '{' | '}' | '|' | '^'
            )
    }) {
        return Err("URL cannot contain whitespace, control, or unsafe characters".to_string());
    }

    let lower = trimmed.to_ascii_lowercase();
    let candidate = if let Some(idx) = lower.find("://") {
        let scheme = &lower[..idx];
        if scheme != "http" && scheme != "https" {
            return Err(format!("Unsupported URL scheme '{scheme}'; only http/https allowed"));
        }
        trimmed.to_string()
    } else {
        // Reject dangerous schemeless URIs such as `javascript:alert(1)` or `file:/etc/passwd`
        let authority_end = trimmed.find(['/', '?', '#']).unwrap_or(trimmed.len());
        let before_path = &trimmed[..authority_end];
        if let Some(colon) = before_path.find(':') {
            let prefix = &before_path[..colon];
            let suffix = &before_path[colon + 1..];
            // Only allow `host:port` where port is digits; reject arbitrary `scheme:payload`.
            if !prefix.is_empty() && (suffix.is_empty() || !suffix.chars().all(|c| c.is_ascii_digit())) {
                return Err("Unsupported URI scheme; only http:// or https:// URLs are allowed".to_string());
            }
        }
        format!("https://{trimmed}")
    };

    let after_scheme = candidate
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or("");
    let host = after_scheme.split(['/', '?', '#']).next().unwrap_or("");
    if host.is_empty() {
        return Err("URL must include a hostname".to_string());
    }
    Ok(candidate)
}

/// Pure-Rust RFC 3986 percent-encoding helper for query strings.
pub fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len() * 2);
    for &b in input.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push('%');
            const HEX: &[u8; 16] = b"0123456789ABCDEF";
            out.push(HEX[(b >> 4) as usize] as char);
            out.push(HEX[(b & 0x0F) as usize] as char);
        }
    }
    out
}

/// Constructs a sanitized search URL for YouTube, Twitch, or Google.
pub fn build_search_url(engine: &str, query: &str) -> Result<String, String> {
    let q = query.trim();
    if q.is_empty() {
        return Err("Search query cannot be empty".to_string());
    }
    let encoded = percent_encode(q);
    let url = match engine.trim().to_ascii_lowercase().as_str() {
        "youtube" | "yt" => format!("https://www.youtube.com/results?search_query={encoded}"),
        "twitch" => format!("https://www.twitch.tv/search?term={encoded}"),
        _ => format!("https://www.google.com/search?q={encoded}"),
    };
    Ok(url)
}

/// Adds a validated shortcut item to `list` and saves to disk unless `persist` is false.
pub fn add_shortcut_to_list(
    list: &mut Vec<ShortcutItem>,
    name: &str,
    kind: &str,
    target: &str,
    icon: Option<&str>,
    color: Option<&str>,
) -> Result<ShortcutItem, String> {
    if list.len() >= 64 {
        return Err("Maximum of 64 shortcuts reached".to_string());
    }
    let clean_name = name.trim();
    if clean_name.is_empty() {
        return Err("Shortcut name cannot be empty".to_string());
    }
    if clean_name.len() > 36 {
        return Err("Shortcut name must be 36 characters or fewer".to_string());
    }

    let clean_kind = kind.trim().to_ascii_lowercase();
    let clean_target = match clean_kind.as_str() {
        "url" => normalize_http_url(target)?,
        "preset" => match target.trim().to_ascii_lowercase().as_str() {
            "browser" => "browser".to_string(),
            "steam" => "steam".to_string(),
            other => return Err(format!("Unknown preset '{other}'")),
        },
        other => return Err(format!("Invalid shortcut kind '{other}'")),
    };

    let base_slug: String = clean_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    let slug = if base_slug.is_empty() {
        "custom".to_string()
    } else {
        base_slug
    };
    let mut unique_id = slug.clone();
    let mut counter = 2usize;
    while list.iter().any(|s| s.id == unique_id) {
        unique_id = format!("{slug}-{counter}");
        counter += 1;
    }

    let final_icon = icon
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().take(4).collect::<String>())
        .unwrap_or_else(|| "★".to_string());

    let final_color = color
        .map(str::trim)
        .filter(|s| {
            s.starts_with('#')
                && (s.len() == 4 || s.len() == 7)
                && s[1..].bytes().all(|b| b.is_ascii_hexdigit())
        })
        .unwrap_or("#38bdf8")
        .to_string();

    let item = ShortcutItem {
        id: unique_id,
        name: clean_name.to_string(),
        kind: clean_kind,
        target: clean_target,
        icon: final_icon,
        color: final_color,
    };
    list.push(item.clone());
    Ok(item)
}

/// Launches a validated `http://`/`https://` URL on the media PC's browser.
pub fn launch_url(url: &str, app_mode: bool, dry_run: bool) -> Result<(), String> {
    let clean = normalize_http_url(url)?;
    if dry_run {
        eprintln!("[dry-run] launch_url url={clean} app_mode={app_mode}");
        return Ok(());
    }
    spawn_url_detached(&clean, app_mode)
}

/// Launches an allowlisted built-in preset (`"browser"` or `"steam"`).
pub fn launch_preset(preset: &str, dry_run: bool) -> Result<(), String> {
    let norm = preset.trim().to_ascii_lowercase();
    if dry_run {
        match norm.as_str() {
            "browser" | "steam" => {
                eprintln!("[dry-run] launch_preset preset={norm}");
                return Ok(());
            }
            other => return Err(format!("Unknown launch preset '{other}'")),
        }
    }

    match norm.as_str() {
        "browser" => {
            #[cfg(target_os = "linux")]
            {
                for bin in [
                    "chromium",
                    "chromium-browser",
                    "google-chrome",
                    "google-chrome-stable",
                    "firefox",
                    "brave",
                ] {
                    if spawn_detached(bin, &[]) {
                        return Ok(());
                    }
                }
                if spawn_detached("xdg-open", &["https://www.google.com"]) {
                    return Ok(());
                }
                Err("No web browser executable or xdg-open found".to_string())
            }
            #[cfg(windows)]
            {
                if spawn_detached("rundll32.exe", &["url.dll,FileProtocolHandler", "https://www.google.com"]) {
                    return Ok(());
                }
                Err("Failed to launch default browser".to_string())
            }
            #[cfg(target_os = "macos")]
            {
                if spawn_detached("open", &["https://www.google.com"]) {
                    return Ok(());
                }
                Err("Failed to launch default browser".to_string())
            }
            #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
            Err("Unsupported platform".to_string())
        }
        "steam" => {
            #[cfg(target_os = "linux")]
            {
                if spawn_detached("xdg-open", &["steam://open/bigpicture"])
                    || spawn_detached("steam", &["-bigpicture"])
                {
                    return Ok(());
                }
                Err("Failed to launch Steam Big Picture".to_string())
            }
            #[cfg(windows)]
            {
                if spawn_detached("rundll32.exe", &["url.dll,FileProtocolHandler", "steam://open/bigpicture"]) {
                    return Ok(());
                }
                Err("Failed to launch Steam Big Picture".to_string())
            }
            #[cfg(target_os = "macos")]
            {
                if spawn_detached("open", &["steam://open/bigpicture"]) {
                    return Ok(());
                }
                Err("Failed to launch Steam Big Picture".to_string())
            }
            #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
            Err("Unsupported platform".to_string())
        }
        other => Err(format!("Unknown launch preset '{other}'")),
    }
}

fn spawn_url_detached(url: &str, app_mode: bool) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        if app_mode {
            let app_arg = format!("--app={url}");
            for browser in [
                "chromium",
                "chromium-browser",
                "google-chrome",
                "google-chrome-stable",
                "brave",
                "microsoft-edge",
            ] {
                if spawn_detached(browser, &[&app_arg, "--start-fullscreen"]) {
                    return Ok(());
                }
            }
        }
        if spawn_detached("xdg-open", &[url]) {
            return Ok(());
        }
        for browser in ["chromium", "firefox", "google-chrome"] {
            if spawn_detached(browser, &[url]) {
                return Ok(());
            }
        }
        Err("Could not find xdg-open or a browser to open URL".to_string())
    }

    #[cfg(windows)]
    {
        if app_mode {
            let app_arg = format!("--app={url}");
            for browser in ["msedge.exe", "chrome.exe"] {
                if spawn_detached(browser, &[&app_arg, "--start-fullscreen"]) {
                    return Ok(());
                }
            }
        }
        if spawn_detached("rundll32.exe", &["url.dll,FileProtocolHandler", url]) {
            return Ok(());
        }
        Err("Failed to open URL via rundll32".to_string())
    }

    #[cfg(target_os = "macos")]
    {
        let _ = app_mode;
        if spawn_detached("open", &[url]) {
            return Ok(());
        }
        Err("Failed to run `open` on macOS".to_string())
    }

    #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
    {
        let _ = (url, app_mode);
        Err("Unsupported platform".to_string())
    }
}

fn spawn_detached(program: &str, args: &[&str]) -> bool {
    let mut cmd = new_bg_cmd(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    cmd.spawn().is_ok()
}

/// Parses `pactl --format=json list sinks` and `pactl get-default-sink` output into `AudioState`.
pub fn parse_pactl_sinks_json(sinks_json: &str, default_sink: &str) -> Option<AudioState> {
    let arr = serde_json::from_str::<Value>(sinks_json).ok()?;
    let items = arr.as_array()?;
    let default_trimmed = default_sink.trim();

    let mut outputs = Vec::new();
    let mut active_vol: Option<u8> = None;
    let mut active_muted = false;

    for sink in items {
        let name = sink.get("name").and_then(|v| v.as_str()).unwrap_or("").trim();
        if name.is_empty() {
            continue;
        }
        let desc = sink
            .get("description")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(name);
        let is_active = name == default_trimmed;

        outputs.push(AudioOutput {
            id: name.to_string(),
            name: desc.to_string(),
            active: is_active,
        });

        if is_active || active_vol.is_none() {
            let muted = sink.get("mute").and_then(|v| v.as_bool()).unwrap_or(false);
            let mut percents: Vec<u32> = Vec::new();
            if let Some(vol_obj) = sink.get("volume").and_then(|v| v.as_object()) {
                for ch in vol_obj.values() {
                    if let Some(pct_str) = ch.get("value_percent").and_then(|v| v.as_str()) {
                        let digits = pct_str.trim().trim_end_matches('%').trim();
                        if let Ok(n) = digits.parse::<u32>() {
                            percents.push(n);
                        }
                    }
                }
            }
            if !percents.is_empty() {
                let avg = (percents.iter().sum::<u32>() / percents.len() as u32).min(100) as u8;
                if is_active || active_vol.is_none() {
                    active_vol = Some(avg);
                    active_muted = muted;
                }
            } else if is_active {
                active_muted = muted;
            }
        }
    }

    Some(AudioState {
        volume: active_vol,
        muted: active_muted,
        outputs,
    })
}

/// Parses `wpctl get-volume @DEFAULT_AUDIO_SINK@` output (e.g. `"Volume: 0.45 [MUTED]"`).
pub fn parse_wpctl_volume(output: &str) -> Option<(u8, bool)> {
    let trimmed = output.trim();
    let muted = trimmed.contains("[MUTED]");
    for token in trimmed.split_whitespace() {
        if let Ok(val) = token.parse::<f32>() {
            let pct = (val * 100.0).round().clamp(0.0, 100.0) as u8;
            return Some((pct, muted));
        }
    }
    None
}

/// Queries the current host audio volume, mute state, and available output sinks.
pub fn get_audio_state() -> AudioState {
    #[cfg(target_os = "linux")]
    {
        if let (Ok(sinks_out), Ok(def_out)) = (
            new_bg_cmd("pactl").args(["--format=json", "list", "sinks"]).output(),
            new_bg_cmd("pactl").args(["get-default-sink"]).output(),
        ) {
            if sinks_out.status.success() {
                let sinks_str = String::from_utf8_lossy(&sinks_out.stdout);
                let def_str = String::from_utf8_lossy(&def_out.stdout);
                if let Some(state) = parse_pactl_sinks_json(&sinks_str, &def_str) {
                    return state;
                }
            }
        }
        if let Ok(out) = new_bg_cmd("wpctl")
            .args(["get-volume", "@DEFAULT_AUDIO_SINK@"])
            .output()
        {
            if out.status.success() {
                let text = String::from_utf8_lossy(&out.stdout);
                if let Some((vol, muted)) = parse_wpctl_volume(&text) {
                    return AudioState {
                        volume: Some(vol),
                        muted,
                        outputs: Vec::new(),
                    };
                }
            }
        }
        AudioState {
            volume: None,
            muted: false,
            outputs: Vec::new(),
        }
    }

    #[cfg(target_os = "macos")]
    {
        let mut vol: Option<u8> = None;
        let mut muted = false;
        if let Ok(out) = new_bg_cmd("osascript")
            .args(["-e", "output volume of (get volume settings)"])
            .output()
        {
            if out.status.success() {
                if let Ok(v) = String::from_utf8_lossy(&out.stdout).trim().parse::<u8>() {
                    vol = Some(v.min(100));
                }
            }
        }
        if let Ok(out) = new_bg_cmd("osascript")
            .args(["-e", "output muted of (get volume settings)"])
            .output()
        {
            if out.status.success() {
                muted = String::from_utf8_lossy(&out.stdout).trim() == "true";
            }
        }
        let mut outputs = Vec::new();
        let active_name = new_bg_cmd("SwitchAudioSource")
            .args(["-c", "-t", "output"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();
        if let Ok(list_out) = new_bg_cmd("SwitchAudioSource")
            .args(["-a", "-t", "output"])
            .output()
        {
            if list_out.status.success() {
                for line in String::from_utf8_lossy(&list_out.stdout).lines() {
                    let name = line.trim();
                    if !name.is_empty() {
                        outputs.push(AudioOutput {
                            id: name.to_string(),
                            name: name.to_string(),
                            active: name == active_name,
                        });
                    }
                }
            }
        }
        AudioState {
            volume: vol,
            muted,
            outputs,
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        AudioState {
            volume: None,
            muted: false,
            outputs: Vec::new(),
        }
    }
}

/// Sets the host master audio volume (`0..=100`).
pub fn set_volume(percent: u8, dry_run: bool) -> Result<u8, String> {
    let clamped = percent.min(100);
    if dry_run {
        eprintln!("[dry-run] set_volume percent={clamped}%");
        return Ok(clamped);
    }

    #[cfg(target_os = "linux")]
    {
        let arg = format!("{clamped}%");
        if new_bg_cmd("pactl")
            .args(["set-sink-volume", "@DEFAULT_SINK@", &arg])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            return Ok(clamped);
        }
        if new_bg_cmd("wpctl")
            .args(["set-volume", "@DEFAULT_AUDIO_SINK@", &arg])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            return Ok(clamped);
        }
        Err("Failed to set audio volume via pactl or wpctl".to_string())
    }

    #[cfg(target_os = "macos")]
    {
        let script = format!("set volume output volume {clamped}");
        if new_bg_cmd("osascript")
            .args(["-e", &script])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            return Ok(clamped);
        }
        Err("Failed to set volume via osascript".to_string())
    }

    #[cfg(windows)]
    {
        // Use Win32 CoreAudio COM via PowerShell fallback or report gracefully.
        let ps = format!(
            "$ws = New-Object -ComObject WScript.Shell; \
             for($i=0; $i -lt 50; $i++){{$ws.SendKeys([char]174)}}; \
             for($i=0; $i -lt {}; $i++){{$ws.SendKeys([char]175)}}",
            (clamped as u32 + 1) / 2
        );
        if new_bg_cmd("powershell.exe")
            .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &ps])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            return Ok(clamped);
        }
        Err("Failed to adjust Windows volume".to_string())
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    Err("Audio volume control unsupported on this OS".to_string())
}

/// Sets or toggles the default audio sink mute state.
pub fn set_mute(muted: Option<bool>, dry_run: bool) -> Result<(), String> {
    if dry_run {
        eprintln!("[dry-run] set_mute muted={muted:?}");
        return Ok(());
    }

    #[cfg(target_os = "linux")]
    {
        let val = match muted {
            Some(true) => "1",
            Some(false) => "0",
            None => "toggle",
        };
        if new_bg_cmd("pactl")
            .args(["set-sink-mute", "@DEFAULT_SINK@", val])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            return Ok(());
        }
        if new_bg_cmd("wpctl")
            .args(["set-mute", "@DEFAULT_AUDIO_SINK@", val])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            return Ok(());
        }
        Err("Failed to toggle mute via pactl or wpctl".to_string())
    }

    #[cfg(target_os = "macos")]
    {
        let script = match muted {
            Some(true) => "set volume with output muted",
            Some(false) => "set volume without output muted",
            None => "set volume output muted (not (output muted of (get volume settings)))",
        };
        if new_bg_cmd("osascript")
            .args(["-e", script])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            return Ok(());
        }
        Err("Failed to set mute via osascript".to_string())
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = muted;
        Err("Direct mute API unavailable; use media mute key".to_string())
    }
}

/// Switches the active host audio output device after validating `output_id` against existing sinks.
pub fn set_audio_output(output_id: &str, dry_run: bool) -> Result<(), String> {
    let clean_id = output_id.trim();
    if clean_id.is_empty() {
        return Err("Audio output ID cannot be empty".to_string());
    }
    if clean_id.starts_with('-') {
        return Err("Invalid audio output ID".to_string());
    }

    let current = get_audio_state();
    if !current.outputs.iter().any(|o| o.id == clean_id) {
        return Err(format!("Unknown audio output '{clean_id}'"));
    }

    if dry_run {
        eprintln!("[dry-run] set_audio_output id={clean_id}");
        return Ok(());
    }

    #[cfg(target_os = "linux")]
    {
        if new_bg_cmd("pactl")
            .args(["set-default-sink", clean_id])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            // Move any currently playing streams to the newly selected output.
            if let Ok(inputs) = new_bg_cmd("pactl")
                .args(["list", "short", "sink-inputs"])
                .output()
            {
                if inputs.status.success() {
                    for line in String::from_utf8_lossy(&inputs.stdout).lines() {
                        if let Some(stream_id) = line.split_whitespace().next() {
                            if stream_id.chars().all(|c| c.is_ascii_digit()) {
                                let _ = new_bg_cmd("pactl")
                                    .args(["move-sink-input", stream_id, clean_id])
                                    .status();
                            }
                        }
                    }
                }
            }
            return Ok(());
        }
        Err(format!("Failed to switch default audio output to '{clean_id}'"))
    }

    #[cfg(target_os = "macos")]
    {
        if new_bg_cmd("SwitchAudioSource")
            .args(["-t", "output", "-s", clean_id])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            return Ok(());
        }
        Err("Failed to switch output via SwitchAudioSource".to_string())
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    Err("Audio output switching unsupported on this OS".to_string())
}

/// Parses `busctl --user --json=short get-property` MPRIS responses into `NowPlaying`.
pub fn parse_mpris_busctl_metadata(
    status_json: &str,
    meta_json: &str,
    bus_name: &str,
) -> Option<NowPlaying> {
    let status_val = serde_json::from_str::<Value>(status_json).ok()?;
    let status = status_val
        .get("data")
        .and_then(|v| v.as_str())
        .unwrap_or("Stopped")
        .to_string();

    let meta_val = serde_json::from_str::<Value>(meta_json).ok()?;
    let data = meta_val.get("data")?.as_object()?;

    let title = data
        .get("xesam:title")
        .and_then(|entry| entry.get("data"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let artist = data
        .get("xesam:artist")
        .and_then(|entry| entry.get("data"))
        .and_then(|v| {
            if let Some(s) = v.as_str() {
                Some(s.to_string())
            } else if let Some(arr) = v.as_array() {
                let parts: Vec<&str> = arr
                    .iter()
                    .filter_map(|x| x.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .collect();
                if parts.is_empty() {
                    None
                } else {
                    Some(parts.join(", "))
                }
            } else {
                None
            }
        })
        .filter(|s| !s.trim().is_empty());

    let short_player = bus_name
        .strip_prefix("org.mpris.MediaPlayer2.")
        .unwrap_or(bus_name)
        .split('.')
        .next()
        .unwrap_or(bus_name)
        .to_string();

    Some(NowPlaying {
        status,
        title,
        artist,
        player: Some(short_player),
    })
}

/// Queries active media session metadata ("Now Playing") across Linux MPRIS, macOS, or Windows.
pub fn get_now_playing() -> NowPlaying {
    #[cfg(target_os = "linux")]
    {
        if let Ok(out) = new_bg_cmd("playerctl")
            .args([
                "metadata",
                "--format",
                "{{status}}\t{{title}}\t{{artist}}\t{{playerName}}",
            ])
            .output()
        {
            if out.status.success() {
                let raw = String::from_utf8_lossy(&out.stdout);
                let line = raw.trim();
                if !line.is_empty() {
                    let parts: Vec<&str> = line.split('\t').collect();
                    let status = parts.first().copied().unwrap_or("Stopped").to_string();
                    let title = parts
                        .get(1)
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty());
                    let artist = parts
                        .get(2)
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty());
                    let player = parts
                        .get(3)
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty());
                    if title.is_some() || status == "Playing" || status == "Paused" {
                        return NowPlaying {
                            status,
                            title,
                            artist,
                            player,
                        };
                    }
                }
            }
        }

        // Fallback: native D-Bus MPRIS discovery via `busctl --user` (works without playerctl)
        if let Ok(list_out) = new_bg_cmd("busctl")
            .args(["--user", "list", "--no-legend"])
            .output()
        {
            if list_out.status.success() {
                let mut best: Option<NowPlaying> = None;
                for line in String::from_utf8_lossy(&list_out.stdout).lines() {
                    let Some(bus) = line.split_whitespace().next() else {
                        continue;
                    };
                    if !bus.starts_with("org.mpris.MediaPlayer2.") {
                        continue;
                    }
                    let Ok(st_out) = new_bg_cmd("busctl")
                        .args([
                            "--user",
                            "--json=short",
                            "get-property",
                            bus,
                            "/org/mpris/MediaPlayer2",
                            "org.mpris.MediaPlayer2.Player",
                            "PlaybackStatus",
                        ])
                        .output()
                    else {
                        continue;
                    };
                    let Ok(meta_out) = new_bg_cmd("busctl")
                        .args([
                            "--user",
                            "--json=short",
                            "get-property",
                            bus,
                            "/org/mpris/MediaPlayer2",
                            "org.mpris.MediaPlayer2.Player",
                            "Metadata",
                        ])
                        .output()
                    else {
                        continue;
                    };
                    if st_out.status.success() && meta_out.status.success() {
                        let st_str = String::from_utf8_lossy(&st_out.stdout);
                        let meta_str = String::from_utf8_lossy(&meta_out.stdout);
                        if let Some(np) = parse_mpris_busctl_metadata(&st_str, &meta_str, bus) {
                            if np.status == "Playing" {
                                return np;
                            }
                            if best.is_none() && np.title.is_some() {
                                best = Some(np);
                            }
                        }
                    }
                }
                if let Some(np) = best {
                    return np;
                }
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        for (app_name, script) in [
            (
                "Spotify",
                "if application \"Spotify\" is running then tell application \"Spotify\" to (player state as string) & \"\t\" & (name of current track) & \"\t\" & (artist of current track)",
            ),
            (
                "Music",
                "if application \"Music\" is running then tell application \"Music\" to (player state as string) & \"\t\" & (name of current track) & \"\t\" & (artist of current track)",
            ),
        ] {
            if let Ok(out) = new_bg_cmd("osascript").args(["-e", script]).output() {
                if out.status.success() {
                    let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    if !raw.is_empty() {
                        let parts: Vec<&str> = raw.split('\t').collect();
                        let st = match parts.first().copied().unwrap_or("") {
                            "playing" => "Playing",
                            "paused" => "Paused",
                            _ => "Stopped",
                        };
                        return NowPlaying {
                            status: st.to_string(),
                            title: parts.get(1).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
                            artist: parts.get(2).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
                            player: Some(app_name.to_string()),
                        };
                    }
                }
            }
        }
    }

    NowPlaying {
        status: "None".to_string(),
        title: None,
        artist: None,
        player: None,
    }
}

/// Captures an on-demand JPEG screenshot of the full display or a 640x420 crop around the cursor.
pub fn capture_screen_jpeg(cursor_zoom: bool) -> Result<Vec<u8>, String> {
    #[cfg(target_os = "linux")]
    {
        if cursor_zoom {
            if let Ok(pos_out) = new_bg_cmd("hyprctl").arg("cursorpos").output() {
                if pos_out.status.success() {
                    let pos_text = String::from_utf8_lossy(&pos_out.stdout);
                    if let Some((xs, ys)) = pos_text.trim().split_once(',') {
                        if let (Ok(cx), Ok(cy)) = (xs.trim().parse::<i32>(), ys.trim().parse::<i32>()) {
                            let gx = (cx - 320).max(0);
                            let gy = (cy - 210).max(0);
                            let geom = format!("{gx},{gy} 640x420");
                            if let Ok(grim_out) = new_bg_cmd("grim")
                                .args(["-g", &geom, "-t", "jpeg", "-q", "78", "-"])
                                .output()
                            {
                                if grim_out.status.success() && grim_out.stdout.starts_with(&[0xFF, 0xD8]) {
                                    return Ok(grim_out.stdout);
                                }
                            }
                        }
                    }
                }
            }
        }

        if let Ok(grim_out) = new_bg_cmd("grim")
            .args(["-t", "jpeg", "-q", "72", "-"])
            .output()
        {
            if grim_out.status.success() && grim_out.stdout.starts_with(&[0xFF, 0xD8]) {
                return Ok(grim_out.stdout);
            }
        }

        let display = std::env::var("DISPLAY").unwrap_or_else(|_| ":0".to_string());
        if let Ok(ff_out) = new_bg_cmd("ffmpeg")
            .args([
                "-y",
                "-f",
                "x11grab",
                "-i",
                &display,
                "-frames:v",
                "1",
                "-f",
                "image2pipe",
                "-vcodec",
                "mjpeg",
                "-q:v",
                "6",
                "pipe:1",
            ])
            .output()
        {
            if ff_out.status.success() && ff_out.stdout.starts_with(&[0xFF, 0xD8]) {
                return Ok(ff_out.stdout);
            }
        }

        Err("Screen capture failed (ensure `grim` for Wayland or `ffmpeg` for X11 is installed)".to_string())
    }

    #[cfg(windows)]
    {
        let script = if cursor_zoom {
            "Add-Type -AssemblyName System.Drawing,System.Windows.Forms; \
             $p=[System.Windows.Forms.Cursor]::Position; \
             $b=[System.Windows.Forms.Screen]::PrimaryScreen.Bounds; \
             $w=640; $h=420; \
             $x=[Math]::Max($b.Left, [Math]::Min($p.X - 320, $b.Right - $w)); \
             $y=[Math]::Max($b.Top, [Math]::Min($p.Y - 210, $b.Bottom - $h)); \
             $bmp=New-Object System.Drawing.Bitmap $w,$h; \
             $g=[System.Drawing.Graphics]::FromImage($bmp); \
             $g.CopyFromScreen($x,$y,0,0,$bmp.Size); \
             $ms=New-Object System.IO.MemoryStream; \
             $bmp.Save($ms,[System.Drawing.Imaging.ImageFormat]::Jpeg); \
             [Convert]::ToBase64String($ms.ToArray())"
        } else {
            "Add-Type -AssemblyName System.Drawing,System.Windows.Forms; \
             $b=[System.Windows.Forms.Screen]::PrimaryScreen.Bounds; \
             $bmp=New-Object System.Drawing.Bitmap $b.Width,$b.Height; \
             $g=[System.Drawing.Graphics]::FromImage($bmp); \
             $g.CopyFromScreen($b.Location,[System.Drawing.Point]::Empty,$b.Size); \
             $ms=New-Object System.IO.MemoryStream; \
             $bmp.Save($ms,[System.Drawing.Imaging.ImageFormat]::Jpeg); \
             [Convert]::ToBase64String($ms.ToArray())"
        };
        let out = new_bg_cmd("powershell.exe")
            .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", script])
            .output()
            .map_err(|e| format!("PowerShell screen capture failed: {e}"))?;
        if !out.status.success() {
            return Err("Windows screen capture returned an error".to_string());
        }
        let b64 = String::from_utf8_lossy(&out.stdout).trim().to_string();
        decode_base64(&b64).ok_or_else(|| "Invalid base64 JPEG from Windows capture".to_string())
    }

    #[cfg(target_os = "macos")]
    {
        let _ = cursor_zoom;
        let tmp = std::env::temp_dir().join(format!(
            "tsr_peek_{}_{}.jpg",
            std::process::id(),
            crate::qr::generate_token(12)
        ));
        let tmp_str = tmp.to_string_lossy().into_owned();
        let status = new_bg_cmd("screencapture")
            .args(["-x", "-t", "jpg", &tmp_str])
            .status();
        let res = match status {
            Ok(s) if s.success() => {
                std::fs::read(&tmp).map_err(|e| format!("read capture failed: {e}"))
            }
            Ok(_) => Err("macOS screencapture exited non-zero".to_string()),
            Err(e) => Err(format!("screencapture failed: {e}")),
        };
        let _ = std::fs::remove_file(&tmp);
        res
    }

    #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
    {
        let _ = cursor_zoom;
        Err("Screen capture unsupported on this OS".to_string())
    }
}

#[cfg(windows)]
fn decode_base64(input: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut buf = 0u32;
    let mut bits = 0u8;
    for &b in input.as_bytes() {
        let val = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b'\r' | b'\n' | b' ' => continue,
            _ => return None,
        };
        buf = (buf << 6) | val as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xFF) as u8);
        }
    }
    Some(out)
}

#[cfg(target_os = "linux")]
fn hypr_output_ok(out: &std::process::Output) -> bool {
    if !out.status.success() {
        return false;
    }
    let stdout = String::from_utf8_lossy(&out.stdout).to_ascii_lowercase();
    let stderr = String::from_utf8_lossy(&out.stderr).to_ascii_lowercase();
    !stdout.contains("error:") && !stderr.contains("error:")
}

/// Executes a Hyprland dispatcher using Hyprland 0.56+ Lua syntax first and falling back
/// to legacy `hyprctl dispatch <args...>` for older Hyprland releases, capturing output cleanly.
#[cfg(target_os = "linux")]
fn hypr_dispatch(lua_expr: &str, legacy_args: &[&str]) -> bool {
    if let Ok(out) = new_bg_cmd("hyprctl").args(["dispatch", lua_expr]).output() {
        if hypr_output_ok(&out) {
            return true;
        }
    }
    if let Ok(out) = new_bg_cmd("hyprctl")
        .arg("dispatch")
        .args(legacy_args)
        .output()
    {
        return hypr_output_ok(&out);
    }
    false
}

/// Validates a hexadecimal window address (`0x` followed by hex digits).
pub fn is_valid_hex_addr(id: &str) -> bool {
    let Some(hex) = id.strip_prefix("0x") else {
        return false;
    };
    !hex.is_empty() && hex.len() <= 20 && hex.bytes().all(|b| b.is_ascii_hexdigit())
}

fn friendly_app_name(raw_class: &str) -> String {
    let trimmed = raw_class.trim();
    if trimmed.is_empty() {
        return "App".to_string();
    }
    if trimmed.contains('.') {
        let last = trimmed.rsplit('.').next().unwrap_or(trimmed).trim();
        if !last.is_empty() {
            return last.to_string();
        }
    }
    trimmed.to_string()
}

/// Parses `hyprctl clients -j` output along with active window, visible workspaces, and cursor
/// coordinates into a sorted `Vec<WindowInfo>` with exact `focused` and `under_cursor` flags.
pub fn parse_hypr_windows(
    clients_json: &str,
    active_addr: Option<&str>,
    active_workspaces: &[i64],
    cursor: Option<(i32, i32)>,
) -> Vec<WindowInfo> {
    let Ok(val) = serde_json::from_str::<Value>(clients_json) else {
        return Vec::new();
    };
    let Some(arr) = val.as_array() else {
        return Vec::new();
    };

    struct Candidate {
        info: WindowInfo,
        focus_hist: i64,
        floating: bool,
        hit_cursor: bool,
    }

    let mut candidates = Vec::new();
    for item in arr {
        let mapped = item.get("mapped").and_then(|v| v.as_bool()).unwrap_or(true);
        let hidden = item.get("hidden").and_then(|v| v.as_bool()).unwrap_or(false);
        if !mapped || hidden {
            continue;
        }
        let address = item
            .get("address")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if address.is_empty() {
            continue;
        }

        let raw_class = item
            .get("class")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .or_else(|| item.get("initialClass").and_then(|v| v.as_str()))
            .unwrap_or("App");
        let app = friendly_app_name(raw_class);

        let title = item
            .get("title")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(&app)
            .to_string();

        let ws_obj = item.get("workspace");
        let ws_id = ws_obj.and_then(|w| w.get("id")).and_then(|v| v.as_i64());
        let ws_name = ws_obj
            .and_then(|w| w.get("name"))
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| ws_id.map(|id| id.to_string()))
            .unwrap_or_else(|| "?".to_string());

        let visible = item.get("visible").and_then(|v| v.as_bool()).unwrap_or(true);
        let floating = item.get("floating").and_then(|v| v.as_bool()).unwrap_or(false);
        let focus_hist = item
            .get("focusHistoryID")
            .and_then(|v| v.as_i64())
            .unwrap_or(i64::MAX);

        let at = item.get("at").and_then(|v| v.as_array());
        let size = item.get("size").and_then(|v| v.as_array());
        let hit_cursor = if let (Some((cx, cy)), Some(at_arr), Some(sz_arr)) = (cursor, at, size) {
            let ws_visible = match ws_id {
                Some(id) if !active_workspaces.is_empty() => active_workspaces.contains(&id),
                _ => visible,
            };
            if ws_visible && visible && at_arr.len() >= 2 && sz_arr.len() >= 2 {
                let x = at_arr[0].as_i64().unwrap_or(0) as i32;
                let y = at_arr[1].as_i64().unwrap_or(0) as i32;
                let w = sz_arr[0].as_i64().unwrap_or(0) as i32;
                let h = sz_arr[1].as_i64().unwrap_or(0) as i32;
                w > 0 && h > 0 && cx >= x && cx < x + w && cy >= y && cy < y + h
            } else {
                false
            }
        } else {
            false
        };

        let focused = active_addr == Some(address.as_str());
        candidates.push(Candidate {
            info: WindowInfo {
                id: address,
                title,
                app,
                workspace: ws_name,
                focused,
                under_cursor: false,
            },
            focus_hist,
            floating,
            hit_cursor,
        });
    }

    // Mark the topmost matching candidate under the cursor (floating wins over tiled, then lowest focusHistoryID)
    if let Some(best_idx) = candidates
        .iter()
        .enumerate()
        .filter(|(_, c)| c.hit_cursor)
        .min_by_key(|(_, c)| (!c.floating, c.focus_hist))
        .map(|(idx, _)| idx)
    {
        candidates[best_idx].info.under_cursor = true;
    }

    candidates.sort_by_key(|c| (!c.info.focused, !c.info.under_cursor, c.focus_hist));
    candidates.into_iter().map(|c| c.info).collect()
}

/// Lists open desktop windows across Hyprland, X11 (`wmctrl`), Windows, or macOS.
pub fn list_open_windows() -> Vec<WindowInfo> {
    #[cfg(target_os = "linux")]
    {
        if let Ok(clients_out) = new_bg_cmd("hyprctl").args(["clients", "-j"]).output() {
            if clients_out.status.success() {
                let clients_json = String::from_utf8_lossy(&clients_out.stdout);
                let active_addr = new_bg_cmd("hyprctl")
                    .args(["activewindow", "-j"])
                    .output()
                    .ok()
                    .filter(|o| o.status.success())
                    .and_then(|o| serde_json::from_slice::<Value>(&o.stdout).ok())
                    .and_then(|v| v.get("address").and_then(|a| a.as_str()).map(|s| s.to_string()));

                let active_workspaces: Vec<i64> = new_bg_cmd("hyprctl")
                    .args(["monitors", "-j"])
                    .output()
                    .ok()
                    .filter(|o| o.status.success())
                    .and_then(|o| serde_json::from_slice::<Value>(&o.stdout).ok())
                    .and_then(|v| {
                        v.as_array().map(|arr| {
                            arr.iter()
                                .filter_map(|m| m.get("activeWorkspace")?.get("id")?.as_i64())
                                .collect()
                        })
                    })
                    .unwrap_or_default();

                let cursor = new_bg_cmd("hyprctl")
                    .arg("cursorpos")
                    .output()
                    .ok()
                    .filter(|o| o.status.success())
                    .and_then(|o| {
                        let txt = String::from_utf8_lossy(&o.stdout);
                        let (xs, ys) = txt.trim().split_once(',')?;
                        Some((xs.trim().parse::<i32>().ok()?, ys.trim().parse::<i32>().ok()?))
                    });

                return parse_hypr_windows(
                    &clients_json,
                    active_addr.as_deref(),
                    &active_workspaces,
                    cursor,
                );
            }
        }

        // X11 fallback via `wmctrl -lp`
        if let Ok(out) = new_bg_cmd("wmctrl").args(["-lp"]).output() {
            if out.status.success() {
                let mut windows = Vec::new();
                for line in String::from_utf8_lossy(&out.stdout).lines() {
                    let parts: Vec<&str> = line.splitn(5, char::is_whitespace).filter(|s| !s.is_empty()).collect();
                    if parts.len() >= 5 {
                        let id = parts[0].trim().to_string();
                        let ws = parts[1].trim().to_string();
                        let title = parts[4].trim().to_string();
                        windows.push(WindowInfo {
                            id,
                            title: title.clone(),
                            app: "Window".to_string(),
                            workspace: ws,
                            focused: false,
                            under_cursor: false,
                        });
                    }
                }
                return windows;
            }
        }
        Vec::new()
    }

    #[cfg(windows)]
    {
        let script = "Get-Process | Where-Object {$_.MainWindowHandle -ne 0 -and $_.MainWindowTitle} | \
            Select-Object @{n='id';e={'0x' + $_.MainWindowHandle.ToString('x')}}, @{n='title';e={$_.MainWindowTitle}}, @{n='app';e={$_.ProcessName}} | \
            ConvertTo-Json -Compress";
        let Ok(out) = new_bg_cmd("powershell.exe")
            .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", script])
            .output()
        else {
            return Vec::new();
        };
        if !out.status.success() {
            return Vec::new();
        }
        let Ok(val) = serde_json::from_slice::<Value>(&out.stdout) else {
            return Vec::new();
        };
        let items = if let Some(arr) = val.as_array() {
            arr.clone()
        } else if val.is_object() {
            vec![val]
        } else {
            Vec::new()
        };
        items
            .into_iter()
            .filter_map(|v| {
                Some(WindowInfo {
                    id: v.get("id")?.as_str()?.to_string(),
                    title: v.get("title")?.as_str()?.to_string(),
                    app: v.get("app")?.as_str()?.to_string(),
                    workspace: "Desktop".to_string(),
                    focused: false,
                    under_cursor: false,
                })
            })
            .collect()
    }

    #[cfg(target_os = "macos")]
    {
        let script = "tell application \"System Events\" to get name of every application process whose visible is true and background only is false";
        let Ok(out) = new_bg_cmd("osascript").args(["-e", script]).output() else {
            return Vec::new();
        };
        if !out.status.success() {
            return Vec::new();
        }
        String::from_utf8_lossy(&out.stdout)
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|name| WindowInfo {
                id: name.to_string(),
                title: name.to_string(),
                app: name.to_string(),
                workspace: "Desktop".to_string(),
                focused: false,
                under_cursor: false,
            })
            .collect()
    }

    #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
    Vec::new()
}

/// Focuses and raises an open window verified against `list_open_windows()`.
pub fn focus_window(id: &str, dry_run: bool) -> Result<String, String> {
    let clean_id = id.trim();
    let windows = list_open_windows();
    let found = windows
        .into_iter()
        .find(|w| w.id == clean_id)
        .ok_or_else(|| format!("Window '{clean_id}' not found"))?;

    if dry_run {
        eprintln!("[dry-run] focus_window id={} title={}", found.id, found.title);
        return Ok(found.title);
    }

    #[cfg(target_os = "linux")]
    {
        if is_valid_hex_addr(&found.id)
            && (hypr_dispatch(
                &format!(r#"hl.dsp.focus({{ window = "address:{}" }})"#, found.id),
                &["focuswindow", &format!("address:{}", found.id)],
            ) || new_bg_cmd("wmctrl")
                .args(["-i", "-a", &found.id])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false))
        {
            return Ok(found.title);
        }
        Err(format!("Failed to focus window '{}'", found.title))
    }

    #[cfg(windows)]
    {
        if !is_valid_hex_addr(&found.id) {
            return Err("Invalid window handle".to_string());
        }
        let handle_hex = found.id.trim_start_matches("0x");
        let ps = format!(
            "Add-Type -TypeDefinition 'using System;using System.Runtime.InteropServices;public class W{{[DllImport(\"user32.dll\")]public static extern bool ShowWindow(IntPtr h,int c);[DllImport(\"user32.dll\")]public static extern bool SetForegroundWindow(IntPtr h);}}'; \
             $h=[IntPtr]::new([Convert]::ToInt64('{handle_hex}',16)); [W]::ShowWindow($h,9)|Out-Null; [W]::SetForegroundWindow($h)|Out-Null"
        );
        if new_bg_cmd("powershell.exe")
            .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &ps])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return Ok(found.title);
        }
        Err(format!("Failed to focus '{}'", found.title))
    }

    #[cfg(target_os = "macos")]
    {
        if new_bg_cmd("osascript")
            .args([
                "-e",
                "on run argv",
                "-e",
                "tell application (item 1 of argv) to activate",
                "-e",
                "end run",
                "--",
                &found.id,
            ])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return Ok(found.title);
        }
        Err(format!("Failed to activate '{}'", found.title))
    }

    #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
    Err("Window focus unsupported on this OS".to_string())
}

/// Closes a specific open window verified against `list_open_windows()`.
pub fn close_window(id: &str, dry_run: bool) -> Result<String, String> {
    let clean_id = id.trim();
    let windows = list_open_windows();
    let found = windows
        .into_iter()
        .find(|w| w.id == clean_id)
        .ok_or_else(|| format!("Window '{clean_id}' not found"))?;

    if dry_run {
        eprintln!("[dry-run] close_window id={} title={}", found.id, found.title);
        return Ok(found.title);
    }

    #[cfg(target_os = "linux")]
    {
        if is_valid_hex_addr(&found.id)
            && (hypr_dispatch(
                &format!(r#"hl.dsp.window.close({{ window = "address:{}" }})"#, found.id),
                &["closewindow", &format!("address:{}", found.id)],
            ) || new_bg_cmd("wmctrl")
                .args(["-i", "-c", &found.id])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false))
        {
            return Ok(found.title);
        }
        Err(format!("Failed to close window '{}'", found.title))
    }

    #[cfg(windows)]
    {
        if !is_valid_hex_addr(&found.id) {
            return Err("Invalid window handle".to_string());
        }
        let handle_hex = found.id.trim_start_matches("0x");
        let ps = format!(
            "Add-Type -TypeDefinition 'using System;using System.Runtime.InteropServices;public class W{{[DllImport(\"user32.dll\")]public static extern bool PostMessage(IntPtr h,uint m,IntPtr w,IntPtr l);}}'; \
             $h=[IntPtr]::new([Convert]::ToInt64('{handle_hex}',16)); [W]::PostMessage($h,0x0010,[IntPtr]::Zero,[IntPtr]::Zero)|Out-Null"
        );
        if new_bg_cmd("powershell.exe")
            .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &ps])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return Ok(found.title);
        }
        Err(format!("Failed to close '{}'", found.title))
    }

    #[cfg(target_os = "macos")]
    {
        if new_bg_cmd("osascript")
            .args([
                "-e",
                "on run argv",
                "-e",
                "tell application (item 1 of argv) to close front window",
                "-e",
                "end run",
                "--",
                &found.id,
            ])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return Ok(found.title);
        }
        Err(format!("Failed to close '{}'", found.title))
    }

    #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
    Err("Window close unsupported on this OS".to_string())
}

/// Closes whichever window is currently under the mouse cursor (falling back to the focused window).
pub fn close_window_under_cursor(dry_run: bool) -> Result<String, String> {
    let windows = list_open_windows();
    if let Some(target) = windows
        .iter()
        .find(|w| w.under_cursor)
        .or_else(|| windows.iter().find(|w| w.focused))
        .or_else(|| windows.first())
    {
        return close_window(&target.id, dry_run);
    }

    #[cfg(target_os = "linux")]
    if !dry_run && hypr_dispatch("hl.dsp.window.close()", &["killactive"]) {
        return Ok("Active window".to_string());
    }

    Err("No open window found".to_string())
}

/// Switches to the next (`"next"`) or previous (`"prev"`) desktop/workspace.
/// Returns `Ok(true)` if handled natively (or under `dry_run`), or `Ok(false)` if the
/// caller should emit the OS desktop-switch keyboard shortcut fallback.
pub fn switch_workspace(dir: &str, dry_run: bool) -> Result<bool, String> {
    let norm = dir.trim().to_ascii_lowercase();
    if norm != "next" && norm != "prev" {
        return Err(format!("Unsupported workspace direction '{dir}'"));
    }

    if dry_run {
        eprintln!("[dry-run] switch_workspace dir={norm}");
        return Ok(true);
    }

    #[cfg(target_os = "linux")]
    {
        let handled = if norm == "next" {
            hypr_dispatch(r#"hl.dsp.focus({ workspace = "r+1" })"#, &["workspace", "r+1"])
        } else {
            hypr_dispatch(r#"hl.dsp.focus({ workspace = "r-1" })"#, &["workspace", "r-1"])
        };
        if handled {
            return Ok(true);
        }
    }

    Ok(false)
}

/// Validates and executes an allowlisted host power action (`display_off`, `lock`, `sleep`, `reboot`, `shutdown`).
pub fn execute_power_action(action: &str, dry_run: bool) -> Result<(), String> {
    let norm = action.trim().to_ascii_lowercase();
    if !matches!(
        norm.as_str(),
        "display_off" | "lock" | "sleep" | "reboot" | "shutdown"
    ) {
        return Err(format!("Unsupported power action '{action}'"));
    }

    if dry_run {
        eprintln!("[dry-run] power action={norm}");
        return Ok(());
    }

    #[cfg(target_os = "linux")]
    {
        let ok = match norm.as_str() {
            "display_off" => {
                hypr_dispatch(r#"hl.dsp.dpms({ action = "off" })"#, &["dpms", "off"])
                    || new_bg_cmd("xset")
                        .args(["dpms", "force", "off"])
                        .output()
                        .map(|o| o.status.success())
                        .unwrap_or(false)
            }
            "lock" => {
                new_bg_cmd("loginctl")
                    .arg("lock-session")
                    .output()
                    .map(|o| o.status.success())
                    .unwrap_or(false)
                    || spawn_detached("hyprlock", &[])
                    || spawn_detached("swaylock", &[])
            }
            "sleep" => new_bg_cmd("systemctl")
                .arg("suspend")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false),
            "reboot" => new_bg_cmd("systemctl")
                .arg("reboot")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false),
            "shutdown" => new_bg_cmd("systemctl")
                .arg("poweroff")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false),
            _ => false,
        };
        if ok {
            Ok(())
        } else {
            Err(format!("Failed to execute power action '{norm}'"))
        }
    }

    #[cfg(windows)]
    {
        let ok = match norm.as_str() {
            "display_off" => new_bg_cmd("powershell.exe")
                .args([
                    "-NoProfile",
                    "-WindowStyle",
                    "Hidden",
                    "-Command",
                    "Add-Type -TypeDefinition 'using System;using System.Runtime.InteropServices;public class M{[DllImport(\"user32.dll\")]public static extern int SendMessage(int h,int m,int w,int l);}'; [M]::SendMessage(0xffff,0x0112,0xF170,2)",
                ])
                .status()
                .map(|s| s.success())
                .unwrap_or(false),
            "lock" => new_bg_cmd("rundll32.exe")
                .args(["user32.dll,LockWorkStation"])
                .status()
                .map(|s| s.success())
                .unwrap_or(false),
            "sleep" => new_bg_cmd("rundll32.exe")
                .args(["powrprof.dll,SetSuspendState", "0,1,0"])
                .status()
                .map(|s| s.success())
                .unwrap_or(false),
            "reboot" => new_bg_cmd("shutdown")
                .args(["/r", "/t", "0"])
                .status()
                .map(|s| s.success())
                .unwrap_or(false),
            "shutdown" => new_bg_cmd("shutdown")
                .args(["/s", "/t", "0"])
                .status()
                .map(|s| s.success())
                .unwrap_or(false),
            _ => false,
        };
        if ok {
            Ok(())
        } else {
            Err(format!("Failed to execute Windows power action '{norm}'"))
        }
    }

    #[cfg(target_os = "macos")]
    {
        let ok = match norm.as_str() {
            "display_off" | "lock" => new_bg_cmd("pmset")
                .arg("displaysleepnow")
                .status()
                .map(|s| s.success())
                .unwrap_or(false),
            "sleep" => new_bg_cmd("pmset")
                .arg("sleepnow")
                .status()
                .map(|s| s.success())
                .unwrap_or(false),
            "reboot" => new_bg_cmd("osascript")
                .args(["-e", "tell app \"System Events\" to restart"])
                .status()
                .map(|s| s.success())
                .unwrap_or(false),
            "shutdown" => new_bg_cmd("osascript")
                .args(["-e", "tell app \"System Events\" to shut down"])
                .status()
                .map(|s| s.success())
                .unwrap_or(false),
            _ => false,
        };
        if ok {
            Ok(())
        } else {
            Err(format!("Failed to execute macOS power action '{norm}'"))
        }
    }

    #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
    Err("Power actions unsupported on this OS".to_string())
}

/// Discovers Tailscale peers and concurrently probes `port` to find other PCs running `tailscale-remote`.
pub fn probe_tailnet_peers(port: u16, self_url: &str) -> Vec<TailnetPeerStatus> {
    let mut results = Vec::new();
    let Ok(output) = new_bg_cmd("tailscale").args(["status", "--json"]).output() else {
        return results;
    };
    if !output.status.success() {
        return results;
    }
    let Ok(val) = serde_json::from_slice::<Value>(&output.stdout) else {
        return results;
    };

    if let Some(self_obj) = val.get("Self").and_then(|v| v.as_object()) {
        let hostname = self_obj
            .get("HostName")
            .and_then(|v| v.as_str())
            .unwrap_or("This PC")
            .to_string();
        let os = self_obj
            .get("OS")
            .and_then(|v| v.as_str())
            .unwrap_or(std::env::consts::OS)
            .to_string();
        let dns_name = self_obj
            .get("DNSName")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().trim_end_matches('.').to_string())
            .filter(|s| !s.is_empty());
        let ip = self_obj
            .get("TailscaleIPs")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.iter().filter_map(|x| x.as_str()).find(|s| s.parse::<std::net::Ipv4Addr>().is_ok()))
            .unwrap_or("127.0.0.1")
            .to_string();

        results.push(TailnetPeerStatus {
            hostname,
            os,
            ip,
            dns_name,
            online: true,
            remote_ready: true,
            url: self_url.to_string(),
            is_self: true,
        });
    }

    let mut raw_peers: Vec<(String, String, String, Option<String>, bool)> = Vec::new();
    if let Some(peer_map) = val.get("Peer").and_then(|v| v.as_object()) {
        for (_, peer) in peer_map {
            let hostname = peer
                .get("HostName")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown")
                .to_string();
            let os = peer
                .get("OS")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let online = peer
                .get("Online")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let dns_name = peer
                .get("DNSName")
                .and_then(|v| v.as_str())
                .map(|s| s.trim().trim_end_matches('.').to_string())
                .filter(|s| !s.is_empty());
            let ip = peer
                .get("TailscaleIPs")
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.iter().filter_map(|x| x.as_str()).find(|s| s.parse::<std::net::Ipv4Addr>().is_ok()))
                .unwrap_or("")
                .to_string();

            if !ip.is_empty() {
                raw_peers.push((hostname, os, ip, dns_name, online));
            }
        }
    }

    raw_peers.truncate(64);
    let handles: Vec<_> = raw_peers
        .into_iter()
        .map(|(hostname, os, ip, dns_name, online)| {
            std::thread::spawn(move || {
                let remote_ready = if online {
                    if let Ok(parsed_ip) = ip.parse::<IpAddr>() {
                        let addr = SocketAddr::new(parsed_ip, port);
                        TcpStream::connect_timeout(&addr, Duration::from_millis(220)).is_ok()
                    } else {
                        false
                    }
                } else {
                    false
                };
                let url = format!("http://{ip}:{port}/");
                TailnetPeerStatus {
                    hostname,
                    os,
                    ip,
                    dns_name,
                    online,
                    remote_ready,
                    url,
                    is_self: false,
                }
            })
        })
        .collect();

    for h in handles {
        if let Ok(peer) = h.join() {
            results.push(peer);
        }
    }

    results.sort_by(|a, b| {
        b.is_self
            .cmp(&a.is_self)
            .then(b.remote_ready.cmp(&a.remote_ready))
            .then(b.online.cmp(&a.online))
            .then(a.hostname.to_lowercase().cmp(&b.hostname.to_lowercase()))
    });

    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_http_url() {
        assert_eq!(
            normalize_http_url("https://www.youtube.com/watch?v=dQw4w9WgXcQ").unwrap(),
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
        );
        assert_eq!(
            normalize_http_url("youtube.com").unwrap(),
            "https://youtube.com"
        );
        assert_eq!(
            normalize_http_url("http://localhost:32400/web").unwrap(),
            "http://localhost:32400/web"
        );

        assert!(normalize_http_url("").is_err());
        assert!(normalize_http_url("file:///etc/passwd").is_err());
        assert!(normalize_http_url("javascript:alert(1)").is_err());
        assert!(normalize_http_url("data:text/html,hi").is_err());
        assert!(normalize_http_url("https://example.com && rm -rf /").is_err());
    }

    #[test]
    fn test_percent_encode_and_search_url() {
        assert_eq!(percent_encode("hello world!"), "hello%20world%21");
        assert_eq!(
            build_search_url("youtube", "lofi hip hop").unwrap(),
            "https://www.youtube.com/results?search_query=lofi%20hip%20hop"
        );
        assert_eq!(
            build_search_url("google", "rust lang").unwrap(),
            "https://www.google.com/search?q=rust%20lang"
        );
        assert!(build_search_url("youtube", "   ").is_err());
    }

    #[test]
    fn test_default_shortcuts_contains_youtube_and_browser() {
        let list = default_shortcuts();
        assert!(list.iter().any(|s| s.id == "youtube" && s.target.contains("youtube.com")));
        assert!(list.iter().any(|s| s.id == "browser" && s.kind == "preset"));
    }

    #[test]
    fn test_parse_pactl_and_wpctl() {
        let sample_sinks = r#"[
            {"name":"alsa_output.hdmi1","description":"HDMI 1 Output","mute":false,"volume":{"front-left":{"value_percent":"100%"}}},
            {"name":"alsa_output.speaker","description":"Built-in Speaker","mute":true,"volume":{"front-left":{"value_percent":"45%"},"front-right":{"value_percent":"47%"}}}
        ]"#;
        let state = parse_pactl_sinks_json(sample_sinks, "alsa_output.speaker\n").unwrap();
        assert_eq!(state.volume, Some(46));
        assert!(state.muted);
        assert_eq!(state.outputs.len(), 2);
        assert!(!state.outputs[0].active);
        assert!(state.outputs[1].active);

        assert_eq!(parse_wpctl_volume("Volume: 0.45\n"), Some((45, false)));
        assert_eq!(parse_wpctl_volume("Volume: 0.72 [MUTED]\n"), Some((72, true)));
    }

    #[test]
    fn test_parse_mpris_busctl_metadata() {
        let status = r#"{"type":"s","data":"Playing"}"#;
        let meta = r#"{"type":"a{sv}","data":{"xesam:title":{"type":"s","data":"Lo-Fi Chill Beats"},"xesam:artist":{"type":"as","data":["ChilledCow","Lofi Girl"]}}}"#;
        let np = parse_mpris_busctl_metadata(status, meta, "org.mpris.MediaPlayer2.chromium.instance123").unwrap();
        assert_eq!(np.status, "Playing");
        assert_eq!(np.title.as_deref(), Some("Lo-Fi Chill Beats"));
        assert_eq!(np.artist.as_deref(), Some("ChilledCow, Lofi Girl"));
        assert_eq!(np.player.as_deref(), Some("chromium"));
    }

    #[test]
    fn test_dry_run_power_and_launch() {
        assert!(execute_power_action("display_off", true).is_ok());
        assert!(execute_power_action("sleep", true).is_ok());
        assert!(execute_power_action("invalid_cmd", true).is_err());

        assert!(launch_url("https://youtube.com", false, true).is_ok());
        assert!(launch_url("file:///etc/passwd", false, true).is_err());
        assert!(launch_preset("browser", true).is_ok());
        assert!(launch_preset("unknown", true).is_err());
    }

    #[test]
    fn test_hex_addr_and_parse_hypr_windows() {
        assert!(is_valid_hex_addr("0x5644db053700"));
        assert!(!is_valid_hex_addr("5644db053700"));
        assert!(!is_valid_hex_addr("0x"));
        assert!(!is_valid_hex_addr("0x123;evil"));

        let sample = r#"[
            {
                "address": "0x1111",
                "mapped": true,
                "hidden": false,
                "visible": true,
                "at": [0, 24],
                "size": [640, 776],
                "workspace": { "id": 1, "name": "1" },
                "floating": false,
                "class": "org.omarchy.agent",
                "title": "foot",
                "focusHistoryID": 2
            },
            {
                "address": "0x2222",
                "mapped": true,
                "hidden": false,
                "visible": true,
                "at": [640, 24],
                "size": [640, 776],
                "workspace": { "id": 1, "name": "1" },
                "floating": false,
                "class": "firefox",
                "title": "YouTube - Mozilla Firefox",
                "focusHistoryID": 0
            },
            {
                "address": "0x3333",
                "mapped": true,
                "hidden": false,
                "visible": true,
                "at": [500, 100],
                "size": [400, 400],
                "workspace": { "id": 1, "name": "1" },
                "floating": true,
                "class": "vlc",
                "title": "Movie.mkv",
                "focusHistoryID": 1
            },
            {
                "address": "0x4444",
                "mapped": false,
                "hidden": false,
                "visible": false,
                "at": [0, 0],
                "size": [100, 100],
                "workspace": { "id": 2, "name": "2" },
                "floating": false,
                "class": "hidden",
                "title": "Hidden",
                "focusHistoryID": 3
            }
        ]"#;

        // Cursor at (700, 200) overlaps both tiled firefox (0x2222) and floating vlc (0x3333); floating wins
        let wins = parse_hypr_windows(sample, Some("0x2222"), &[1], Some((700, 200)));
        assert_eq!(wins.len(), 3);
        assert_eq!(wins[0].id, "0x2222");
        assert!(wins[0].focused);
        assert!(!wins[0].under_cursor);
        assert_eq!(wins[1].id, "0x3333");
        assert!(wins[1].under_cursor);
        assert_eq!(wins[2].id, "0x1111");
        assert_eq!(wins[2].app, "agent");
    }

    #[test]
    fn test_switch_workspace_dry_run_and_validation() {
        assert_eq!(switch_workspace("next", true), Ok(true));
        assert_eq!(switch_workspace("prev", true), Ok(true));
        assert!(switch_workspace("sideways", true).is_err());
    }

    #[test]
    fn test_security_validation_url_shortcuts_and_audio_id() {
        // Unsafe characters and oversized URLs are rejected
        assert!(normalize_http_url("https://example.com/\"onclick=1").is_err());
        assert!(normalize_http_url("https://example.com/<script>").is_err());
        assert!(normalize_http_url("https://example.com/`id`").is_err());
        let too_long = format!("https://example.com/{}", "a".repeat(2040));
        assert!(normalize_http_url(&too_long).is_err());

        // Strict hex color validation and 64-shortcut cap
        let mut list = Vec::new();
        let item = add_shortcut_to_list(
            &mut list,
            "BadColor",
            "url",
            "https://example.com",
            None,
            Some("#zzzzzz"),
        )
        .unwrap();
        assert_eq!(item.color, "#38bdf8");
        let valid_color = add_shortcut_to_list(
            &mut list,
            "GoodColor",
            "url",
            "https://example.com",
            None,
            Some("#Ab12Ef"),
        )
        .unwrap();
        assert_eq!(valid_color.color, "#Ab12Ef");

        for i in 2..64 {
            add_shortcut_to_list(
                &mut list,
                &format!("Site {i}"),
                "url",
                "https://example.com",
                None,
                None,
            )
            .unwrap();
        }
        assert!(add_shortcut_to_list(
            &mut list,
            "Overflow",
            "url",
            "https://example.com",
            None,
            None,
        )
        .is_err());

        // Leading '-' audio sink ID is rejected before querying/invoking pactl
        assert!(set_audio_output("--evil-flag", true).is_err());
    }
}
