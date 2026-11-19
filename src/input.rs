use crate::keymap;
use std::collections::HashSet;
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;
use tokio::sync::mpsc;

/// Dispatched input actions processed sequentially on a dedicated OS input thread.
#[derive(Debug, Clone)]
pub enum InputCommand {
    MouseMove(i32, i32),
    MouseClick {
        btn: String,
        down: Option<bool>,
    },
    MouseScroll(i32, i32),
    Key {
        scancode: u16,
        down: Option<bool>,
        mods: Vec<u16>,
    },
    TypeText {
        text: String,
        enter: bool,
    },
    Clipboard {
        text: String,
        paste: bool,
    },
    ReleaseAll,
}

/// Initializes the platform input adapter synchronously (fail-fast on permission errors)
/// and spawns a dedicated worker thread reading commands from a bounded channel.
pub fn spawn_input_worker(dry_run: bool) -> Result<mpsc::Sender<InputCommand>, String> {
    let mut adapter = if dry_run {
        None
    } else {
        Some(InputAdapter::new()?)
    };

    let (tx, mut rx) = mpsc::channel::<InputCommand>(512);

    std::thread::Builder::new()
        .name("input-worker".to_string())
        .spawn(move || {
            let mut held_buttons: HashSet<String> = HashSet::new();
            let mut held_keys: HashSet<u16> = HashSet::new();

            while let Some(cmd) = rx.blocking_recv() {
                match cmd {
                    InputCommand::MouseMove(dx, dy) => {
                        if let Some(ref mut dev) = adapter {
                            dev.emit_mouse_move(dx, dy);
                        } else {
                            eprintln!("[dry-run] mouse_move dx={dx} dy={dy}");
                        }
                    }
                    InputCommand::MouseClick { btn, down } => {
                        let norm = btn.to_ascii_lowercase();
                        match down {
                            Some(true) => {
                                held_buttons.insert(norm.clone());
                                if let Some(ref mut dev) = adapter {
                                    dev.emit_mouse_click(&norm, true);
                                } else {
                                    eprintln!("[dry-run] mouse_click btn={norm} down=true");
                                }
                            }
                            Some(false) => {
                                held_buttons.remove(&norm);
                                if let Some(ref mut dev) = adapter {
                                    dev.emit_mouse_click(&norm, false);
                                } else {
                                    eprintln!("[dry-run] mouse_click btn={norm} down=false");
                                }
                            }
                            None => {
                                if let Some(ref mut dev) = adapter {
                                    dev.emit_mouse_click(&norm, true);
                                    std::thread::sleep(Duration::from_millis(8));
                                    dev.emit_mouse_click(&norm, false);
                                } else {
                                    eprintln!("[dry-run] mouse_click btn={norm} tap");
                                }
                            }
                        }
                    }
                    InputCommand::MouseScroll(dx, dy) => {
                        if let Some(ref mut dev) = adapter {
                            dev.emit_mouse_wheel(dx, dy);
                        } else {
                            eprintln!("[dry-run] mouse_scroll dx={dx} dy={dy}");
                        }
                    }
                    InputCommand::Key {
                        scancode,
                        down,
                        mods,
                    } => {
                        if let Some(is_down) = down {
                            if is_down {
                                for &m in &mods {
                                    held_keys.insert(m);
                                }
                                held_keys.insert(scancode);
                            } else {
                                held_keys.remove(&scancode);
                                for &m in &mods {
                                    held_keys.remove(&m);
                                }
                            }
                            if let Some(ref mut dev) = adapter {
                                if is_down && !mods.is_empty() {
                                    for &m in &mods {
                                        dev.emit_key(m, true);
                                    }
                                }
                                dev.emit_key(scancode, is_down);
                                if !is_down && !mods.is_empty() {
                                    for &m in mods.iter().rev() {
                                        dev.emit_key(m, false);
                                    }
                                }
                            } else {
                                eprintln!(
                                    "[dry-run] key scancode={scancode} down={is_down} mods={mods:?}"
                                );
                            }
                        } else if let Some(ref mut dev) = adapter {
                            for &m in &mods {
                                dev.emit_key(m, true);
                                std::thread::sleep(Duration::from_millis(2));
                            }
                            dev.emit_key(scancode, true);
                            std::thread::sleep(Duration::from_millis(8));
                            dev.emit_key(scancode, false);
                            for &m in mods.iter().rev() {
                                std::thread::sleep(Duration::from_millis(2));
                                dev.emit_key(m, false);
                            }
                        } else {
                            eprintln!("[dry-run] key_tap scancode={scancode} mods={mods:?}");
                        }
                    }
                    InputCommand::TypeText { text, enter } => {
                        if let Some(ref mut dev) = adapter {
                            dev.type_text(&text);
                            if enter {
                                std::thread::sleep(Duration::from_millis(4));
                                dev.emit_key(28, true);
                                std::thread::sleep(Duration::from_millis(6));
                                dev.emit_key(28, false);
                            }
                        } else {
                            eprintln!("[dry-run] type_text len={} enter={enter}", text.len());
                        }
                    }
                    InputCommand::Clipboard { text, paste } => {
                        if let Some(ref mut dev) = adapter {
                            if let Err(err) = set_clipboard(&text) {
                                eprintln!("[clipboard] {err}");
                            } else if paste {
                                std::thread::sleep(Duration::from_millis(40));
                                #[cfg(target_os = "macos")]
                                let mod_key = 125u16; // Cmd
                                #[cfg(not(target_os = "macos"))]
                                let mod_key = 29u16; // Left Ctrl

                                dev.emit_key(mod_key, true);
                                std::thread::sleep(Duration::from_millis(4));
                                dev.emit_key(47, true); // V
                                std::thread::sleep(Duration::from_millis(8));
                                dev.emit_key(47, false);
                                std::thread::sleep(Duration::from_millis(4));
                                dev.emit_key(mod_key, false);
                            }
                        } else {
                            eprintln!("[dry-run] clipboard len={} paste={paste}", text.len());
                        }
                    }
                    InputCommand::ReleaseAll => {
                        if let Some(ref mut dev) = adapter {
                            for btn in held_buttons.drain() {
                                dev.emit_mouse_click(&btn, false);
                            }
                            for key in held_keys.drain() {
                                dev.emit_key(key, false);
                            }
                        } else {
                            if !held_buttons.is_empty() || !held_keys.is_empty() {
                                eprintln!(
                                    "[dry-run] release_all buttons={held_buttons:?} keys={held_keys:?}"
                                );
                            }
                            held_buttons.clear();
                            held_keys.clear();
                        }
                    }
                }
            }
        })
        .map_err(|e| format!("failed to spawn input worker thread: {e}"))?;

    Ok(tx)
}

fn set_clipboard(text: &str) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        if pipe_to_cmd("wl-copy", &[], text) {
            return Ok(());
        }
        if pipe_to_cmd("xclip", &["-selection", "clipboard"], text) {
            return Ok(());
        }
        if pipe_to_cmd("xsel", &["--clipboard", "--input"], text) {
            return Ok(());
        }
        Err("no clipboard tool found (install wl-clipboard, xclip, or xsel)".to_string())
    }

    #[cfg(windows)]
    {
        if pipe_to_cmd("clip.exe", &[], text) {
            return Ok(());
        }
        Err("failed to invoke clip.exe".to_string())
    }

    #[cfg(target_os = "macos")]
    {
        if pipe_to_cmd("pbcopy", &[], text) {
            return Ok(());
        }
        Err("failed to invoke pbcopy".to_string())
    }

    #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
    {
        let _ = text;
        Err("clipboard not supported on this platform".to_string())
    }
}

fn pipe_to_cmd(program: &str, args: &[&str], data: &str) -> bool {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let Ok(mut child) = cmd.spawn() else {
        return false;
    };
    if let Some(mut stdin) = child.stdin.take() {
        if stdin.write_all(data.as_bytes()).is_err() {
            return false;
        }
    }
    child.wait().map(|s| s.success()).unwrap_or(false)
}

#[cfg(target_os = "linux")]
use evdev::uinput::{VirtualDevice, VirtualDeviceBuilder};
#[cfg(target_os = "linux")]
use evdev::{AttributeSet, EventType, InputEvent, Key, RelativeAxisType};

struct InputAdapter {
    #[cfg(target_os = "linux")]
    device: VirtualDevice,
    #[cfg(target_os = "macos")]
    left_down: bool,
    #[cfg(target_os = "macos")]
    right_down: bool,
}

impl InputAdapter {
    fn new() -> Result<Self, String> {
        #[cfg(target_os = "linux")]
        {
            let mut key_set = AttributeSet::<Key>::new();
            for code in 1..=248 {
                key_set.insert(Key::new(code));
            }
            key_set.insert(Key::BTN_LEFT);
            key_set.insert(Key::BTN_RIGHT);
            key_set.insert(Key::BTN_MIDDLE);
            key_set.insert(Key::BTN_SIDE);
            key_set.insert(Key::BTN_EXTRA);

            let mut rel_set = AttributeSet::<RelativeAxisType>::new();
            rel_set.insert(RelativeAxisType::REL_X);
            rel_set.insert(RelativeAxisType::REL_Y);
            rel_set.insert(RelativeAxisType::REL_WHEEL);
            rel_set.insert(RelativeAxisType::REL_HWHEEL);

            let virtual_device = VirtualDeviceBuilder::new()
                .map_err(format_uinput_error)?
                .name("Tailscale-Remote-Virtual-HID")
                .with_keys(&key_set)
                .map_err(format_uinput_error)?
                .with_relative_axes(&rel_set)
                .map_err(format_uinput_error)?
                .build()
                .map_err(format_uinput_error)?;

            // Allow udev / Wayland compositors / libinput time to bind the new virtual HID node.
            std::thread::sleep(Duration::from_millis(150));

            Ok(Self {
                device: virtual_device,
            })
        }

        #[cfg(windows)]
        {
            Ok(Self {})
        }

        #[cfg(target_os = "macos")]
        {
            Ok(Self {
                left_down: false,
                right_down: false,
            })
        }

        #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
        {
            Ok(Self {})
        }
    }

    fn emit_mouse_move(&mut self, dx: i32, dy: i32) {
        #[cfg(target_os = "linux")]
        {
            let ev_x = InputEvent::new(EventType::RELATIVE, RelativeAxisType::REL_X.0, dx);
            let ev_y = InputEvent::new(EventType::RELATIVE, RelativeAxisType::REL_Y.0, dy);
            let syn = InputEvent::new(EventType::SYNCHRONIZATION, 0, 0);
            let _ = self.device.emit(&[ev_x, ev_y, syn]);
        }

        #[cfg(windows)]
        {
            use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
            let input = INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dx,
                        dy,
                        mouseData: 0,
                        dwFlags: MOUSEEVENTF_MOVE,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            };
            unsafe {
                SendInput(1, &input, std::mem::size_of::<INPUT>() as i32);
            }
        }

        #[cfg(target_os = "macos")]
        {
            macos_ffi::move_mouse_relative(dx, dy, self.left_down, self.right_down);
        }
    }

    fn emit_mouse_wheel(&mut self, dx: i32, dy: i32) {
        #[cfg(target_os = "linux")]
        {
            let mut events = Vec::with_capacity(3);
            if dy != 0 {
                events.push(InputEvent::new(
                    EventType::RELATIVE,
                    RelativeAxisType::REL_WHEEL.0,
                    dy,
                ));
            }
            if dx != 0 {
                events.push(InputEvent::new(
                    EventType::RELATIVE,
                    RelativeAxisType::REL_HWHEEL.0,
                    dx,
                ));
            }
            if !events.is_empty() {
                events.push(InputEvent::new(EventType::SYNCHRONIZATION, 0, 0));
                let _ = self.device.emit(&events);
            }
        }

        #[cfg(windows)]
        {
            use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
            if dy != 0 {
                let input = INPUT {
                    r#type: INPUT_MOUSE,
                    Anonymous: INPUT_0 {
                        mi: MOUSEINPUT {
                            dx: 0,
                            dy: 0,
                            mouseData: (dy * 120) as u32,
                            dwFlags: MOUSEEVENTF_WHEEL,
                            time: 0,
                            dwExtraInfo: 0,
                        },
                    },
                };
                unsafe {
                    SendInput(1, &input, std::mem::size_of::<INPUT>() as i32);
                }
            }
            if dx != 0 {
                let input = INPUT {
                    r#type: INPUT_MOUSE,
                    Anonymous: INPUT_0 {
                        mi: MOUSEINPUT {
                            dx: 0,
                            dy: 0,
                            mouseData: (dx * 120) as u32,
                            dwFlags: MOUSEEVENTF_HWHEEL,
                            time: 0,
                            dwExtraInfo: 0,
                        },
                    },
                };
                unsafe {
                    SendInput(1, &input, std::mem::size_of::<INPUT>() as i32);
                }
            }
        }

        #[cfg(target_os = "macos")]
        {
            macos_ffi::scroll_wheel(dx, dy);
        }
    }

    fn emit_mouse_click(&mut self, btn: &str, is_down: bool) {
        #[cfg(target_os = "linux")]
        {
            let btn_code = match btn {
                "right" => Key::BTN_RIGHT.code(),
                "middle" => Key::BTN_MIDDLE.code(),
                "back" => Key::BTN_SIDE.code(),
                "forward" => Key::BTN_EXTRA.code(),
                _ => Key::BTN_LEFT.code(),
            };
            let act_val = if is_down { 1 } else { 0 };
            let ev = InputEvent::new(EventType::KEY, btn_code, act_val);
            let syn = InputEvent::new(EventType::SYNCHRONIZATION, 0, 0);
            let _ = self.device.emit(&[ev, syn]);
        }

        #[cfg(windows)]
        {
            use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
            use windows_sys::Win32::UI::WindowsAndMessaging::{XBUTTON1, XBUTTON2};
            let (flag, data) = match btn {
                "right" => (
                    if is_down {
                        MOUSEEVENTF_RIGHTDOWN
                    } else {
                        MOUSEEVENTF_RIGHTUP
                    },
                    0,
                ),
                "middle" => (
                    if is_down {
                        MOUSEEVENTF_MIDDLEDOWN
                    } else {
                        MOUSEEVENTF_MIDDLEUP
                    },
                    0,
                ),
                "back" => (
                    if is_down {
                        MOUSEEVENTF_XDOWN
                    } else {
                        MOUSEEVENTF_XUP
                    },
                    XBUTTON1 as u32,
                ),
                "forward" => (
                    if is_down {
                        MOUSEEVENTF_XDOWN
                    } else {
                        MOUSEEVENTF_XUP
                    },
                    XBUTTON2 as u32,
                ),
                _ => (
                    if is_down {
                        MOUSEEVENTF_LEFTDOWN
                    } else {
                        MOUSEEVENTF_LEFTUP
                    },
                    0,
                ),
            };
            let input = INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dx: 0,
                        dy: 0,
                        mouseData: data,
                        dwFlags: flag,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            };
            unsafe {
                SendInput(1, &input, std::mem::size_of::<INPUT>() as i32);
            }
        }

        #[cfg(target_os = "macos")]
        {
            match btn {
                "right" => self.right_down = is_down,
                "middle" | "back" | "forward" => {}
                _ => self.left_down = is_down,
            }
            macos_ffi::mouse_click(btn, is_down);
        }
    }

    fn emit_key(&mut self, scancode: u16, is_down: bool) {
        #[cfg(target_os = "linux")]
        {
            let act_val = if is_down { 1 } else { 0 };
            let ev = InputEvent::new(EventType::KEY, scancode, act_val);
            let syn = InputEvent::new(EventType::SYNCHRONIZATION, 0, 0);
            let _ = self.device.emit(&[ev, syn]);
        }

        #[cfg(windows)]
        {
            use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
            let base_flags = if is_down { 0 } else { KEYEVENTF_KEYUP };
            let input = if let Some((vk, extended)) = keymap::evdev_to_windows_vk(scancode) {
                let ext_flag = if extended { KEYEVENTF_EXTENDEDKEY } else { 0 };
                INPUT {
                    r#type: INPUT_KEYBOARD,
                    Anonymous: INPUT_0 {
                        ki: KEYBDINPUT {
                            wVk: vk,
                            wScan: 0,
                            dwFlags: base_flags | ext_flag,
                            time: 0,
                            dwExtraInfo: 0,
                        },
                    },
                }
            } else {
                INPUT {
                    r#type: INPUT_KEYBOARD,
                    Anonymous: INPUT_0 {
                        ki: KEYBDINPUT {
                            wVk: 0,
                            wScan: scancode,
                            dwFlags: base_flags | KEYEVENTF_SCANCODE,
                            time: 0,
                            dwExtraInfo: 0,
                        },
                    },
                }
            };
            unsafe {
                SendInput(1, &input, std::mem::size_of::<INPUT>() as i32);
            }
        }

        #[cfg(target_os = "macos")]
        {
            if is_down {
                match scancode {
                    113 => {
                        let _ = Command::new("osascript")
                            .args(["-e", "set volume output muted (not (output muted of (get volume settings)))"])
                            .status();
                        return;
                    }
                    114 => {
                        let _ = Command::new("osascript")
                            .args(["-e", "set volume output volume ((output volume of (get volume settings)) - 6)"])
                            .status();
                        return;
                    }
                    115 => {
                        let _ = Command::new("osascript")
                            .args(["-e", "set volume output volume ((output volume of (get volume settings)) + 6)"])
                            .status();
                        return;
                    }
                    _ => {}
                }
            }
            if let Some(mac_kc) = keymap::evdev_to_macos_keycode(scancode) {
                macos_ffi::key_event(mac_kc, is_down);
            }
        }
    }

    fn type_text(&mut self, text: &str) {
        #[cfg(target_os = "linux")]
        {
            const KEY_LEFTSHIFT: u16 = 42;
            for ch in text.chars() {
                let Some((code, shifted)) = keymap::ascii_to_evdev(ch) else {
                    continue;
                };
                if shifted {
                    self.emit_key(KEY_LEFTSHIFT, true);
                    std::thread::sleep(Duration::from_millis(2));
                }
                self.emit_key(code, true);
                std::thread::sleep(Duration::from_millis(2));
                self.emit_key(code, false);
                if shifted {
                    std::thread::sleep(Duration::from_millis(2));
                    self.emit_key(KEY_LEFTSHIFT, false);
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        }

        #[cfg(windows)]
        {
            use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
            for ch in text.chars() {
                if ch == '\n' {
                    self.emit_key(28, true);
                    std::thread::sleep(Duration::from_millis(2));
                    self.emit_key(28, false);
                    continue;
                }
                if ch == '\t' {
                    self.emit_key(15, true);
                    std::thread::sleep(Duration::from_millis(2));
                    self.emit_key(15, false);
                    continue;
                }
                let mut units = [0u16; 2];
                for &u in ch.encode_utf16(&mut units) {
                    let inputs = [
                        INPUT {
                            r#type: INPUT_KEYBOARD,
                            Anonymous: INPUT_0 {
                                ki: KEYBDINPUT {
                                    wVk: 0,
                                    wScan: u,
                                    dwFlags: KEYEVENTF_UNICODE,
                                    time: 0,
                                    dwExtraInfo: 0,
                                },
                            },
                        },
                        INPUT {
                            r#type: INPUT_KEYBOARD,
                            Anonymous: INPUT_0 {
                                ki: KEYBDINPUT {
                                    wVk: 0,
                                    wScan: u,
                                    dwFlags: KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
                                    time: 0,
                                    dwExtraInfo: 0,
                                },
                            },
                        },
                    ];
                    unsafe {
                        SendInput(2, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32);
                    }
                }
            }
        }

        #[cfg(target_os = "macos")]
        {
            macos_ffi::type_unicode_string(text);
        }
    }
}

#[cfg(target_os = "linux")]
fn format_uinput_error(err: std::io::Error) -> String {
    format!(
        "Failed to initialize /dev/uinput virtual device ({err}).\n\
         Ensure the uinput module is loaded and your user has access:\n\
         \n\
         echo uinput | sudo tee /etc/modules-load.d/uinput.conf\n\
         echo 'KERNEL==\"uinput\", MODE=\"0660\", GROUP=\"input\", OPTIONS+=\"static_node=uinput\"' | sudo tee /etc/udev/rules.d/99-uinput.rules\n\
         sudo usermod -aG input \"$USER\"\n\
         sudo modprobe uinput && sudo udevadm control --reload-rules && sudo udevadm trigger\n\
         \n\
         Or run `tailscale-remote --dry-run` to test UI/Tailscale without OS input injection."
    )
}

#[cfg(target_os = "macos")]
mod macos_ffi {
    use std::ffi::c_void;

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct CGPoint {
        pub x: f64,
        pub y: f64,
    }

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn CGMainDisplayID() -> u32;
        fn CGDisplayPixelsWide(display: u32) -> usize;
        fn CGDisplayPixelsHigh(display: u32) -> usize;
        fn CGEventCreate(source: *const c_void) -> *mut c_void;
        fn CGEventGetLocation(event: *const c_void) -> CGPoint;
        fn CGEventCreateMouseEvent(
            source: *const c_void,
            mouse_type: u32,
            mouse_cursor_position: CGPoint,
            mouse_button: u32,
        ) -> *mut c_void;
        fn CGEventCreateKeyboardEvent(
            source: *const c_void,
            virtual_key: u16,
            key_down: bool,
        ) -> *mut c_void;
        fn CGEventKeyboardSetUnicodeString(
            event: *mut c_void,
            string_length: usize,
            unicode_string: *const u16,
        );
        fn CGEventCreateScrollWheelEvent(
            source: *const c_void,
            units: u32,
            wheel_count: u32,
            wheel1: i32,
            wheel2: i32,
        ) -> *mut c_void;
        fn CGEventPost(tap: u32, event: *mut c_void);
        fn CFRelease(cf: *const c_void);
    }

    const K_CG_HID_EVENT_TAP: u32 = 0;
    const K_CG_EVENT_LEFT_MOUSE_DOWN: u32 = 1;
    const K_CG_EVENT_LEFT_MOUSE_UP: u32 = 2;
    const K_CG_EVENT_RIGHT_MOUSE_DOWN: u32 = 3;
    const K_CG_EVENT_RIGHT_MOUSE_UP: u32 = 4;
    const K_CG_EVENT_MOUSE_MOVED: u32 = 5;
    const K_CG_EVENT_LEFT_MOUSE_DRAGGED: u32 = 6;
    const K_CG_EVENT_RIGHT_MOUSE_DRAGGED: u32 = 7;
    const K_CG_EVENT_OTHER_MOUSE_DOWN: u32 = 25;
    const K_CG_EVENT_OTHER_MOUSE_UP: u32 = 26;

    fn display_bounds() -> (i32, i32) {
        unsafe {
            let id = CGMainDisplayID();
            let w = CGDisplayPixelsWide(id) as i32;
            let h = CGDisplayPixelsHigh(id) as i32;
            (w.max(1280), h.max(720))
        }
    }

    fn cursor_pos() -> Option<(i32, i32)> {
        unsafe {
            let ev = CGEventCreate(std::ptr::null());
            if ev.is_null() {
                return None;
            }
            let pt = CGEventGetLocation(ev);
            CFRelease(ev);
            Some((pt.x.round() as i32, pt.y.round() as i32))
        }
    }

    pub fn move_mouse_relative(dx: i32, dy: i32, left_down: bool, right_down: bool) {
        let (max_x, max_y) = display_bounds();
        let (cx, cy) = cursor_pos().unwrap_or((max_x / 2, max_y / 2));
        let nx = (cx + dx).clamp(0, max_x - 1);
        let ny = (cy + dy).clamp(0, max_y - 1);
        let pt = CGPoint {
            x: nx as f64,
            y: ny as f64,
        };
        let (ev_type, btn) = if left_down {
            (K_CG_EVENT_LEFT_MOUSE_DRAGGED, 0)
        } else if right_down {
            (K_CG_EVENT_RIGHT_MOUSE_DRAGGED, 1)
        } else {
            (K_CG_EVENT_MOUSE_MOVED, 0)
        };
        unsafe {
            let ev = CGEventCreateMouseEvent(std::ptr::null(), ev_type, pt, btn);
            if !ev.is_null() {
                CGEventPost(K_CG_HID_EVENT_TAP, ev);
                CFRelease(ev);
            }
        }
    }

    pub fn mouse_click(btn: &str, is_down: bool) {
        let (cx, cy) = cursor_pos().unwrap_or((500, 500));
        let pt = CGPoint {
            x: cx as f64,
            y: cy as f64,
        };
        let (ev_type, button_id) = match btn {
            "right" => (
                if is_down {
                    K_CG_EVENT_RIGHT_MOUSE_DOWN
                } else {
                    K_CG_EVENT_RIGHT_MOUSE_UP
                },
                1,
            ),
            "middle" => (
                if is_down {
                    K_CG_EVENT_OTHER_MOUSE_DOWN
                } else {
                    K_CG_EVENT_OTHER_MOUSE_UP
                },
                2,
            ),
            "back" => (
                if is_down {
                    K_CG_EVENT_OTHER_MOUSE_DOWN
                } else {
                    K_CG_EVENT_OTHER_MOUSE_UP
                },
                3,
            ),
            "forward" => (
                if is_down {
                    K_CG_EVENT_OTHER_MOUSE_DOWN
                } else {
                    K_CG_EVENT_OTHER_MOUSE_UP
                },
                4,
            ),
            _ => (
                if is_down {
                    K_CG_EVENT_LEFT_MOUSE_DOWN
                } else {
                    K_CG_EVENT_LEFT_MOUSE_UP
                },
                0,
            ),
        };
        unsafe {
            let ev = CGEventCreateMouseEvent(std::ptr::null(), ev_type, pt, button_id);
            if !ev.is_null() {
                CGEventPost(K_CG_HID_EVENT_TAP, ev);
                CFRelease(ev);
            }
        }
    }

    pub fn scroll_wheel(dx: i32, dy: i32) {
        unsafe {
            let ev = CGEventCreateScrollWheelEvent(std::ptr::null(), 1, 2, dy * 3, dx * 3);
            if !ev.is_null() {
                CGEventPost(K_CG_HID_EVENT_TAP, ev);
                CFRelease(ev);
            }
        }
    }

    pub fn key_event(keycode: u16, is_down: bool) {
        unsafe {
            let ev = CGEventCreateKeyboardEvent(std::ptr::null(), keycode, is_down);
            if !ev.is_null() {
                CGEventPost(K_CG_HID_EVENT_TAP, ev);
                CFRelease(ev);
            }
        }
    }

    pub fn type_unicode_string(text: &str) {
        let utf16: Vec<u16> = text.encode_utf16().collect();
        for chunk in utf16.chunks(16) {
            unsafe {
                let down = CGEventCreateKeyboardEvent(std::ptr::null(), 0, true);
                if !down.is_null() {
                    CGEventKeyboardSetUnicodeString(down, chunk.len(), chunk.as_ptr());
                    CGEventPost(K_CG_HID_EVENT_TAP, down);
                    CFRelease(down);
                }
                let up = CGEventCreateKeyboardEvent(std::ptr::null(), 0, false);
                if !up.is_null() {
                    CGEventKeyboardSetUnicodeString(up, chunk.len(), chunk.as_ptr());
                    CGEventPost(K_CG_HID_EVENT_TAP, up);
                    CFRelease(up);
                }
            }
        }
    }
}
