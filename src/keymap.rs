/// Translates browser `KeyboardEvent.code` identifiers and friendly aliases
/// into canonical Linux `evdev` scancodes used internally across all OS backends.
pub fn web_key_to_evdev(code: &str) -> Option<u16> {
    match code.trim() {
        // Letters
        "KeyA" => Some(30),
        "KeyB" => Some(48),
        "KeyC" => Some(46),
        "KeyD" => Some(32),
        "KeyE" => Some(18),
        "KeyF" => Some(33),
        "KeyG" => Some(34),
        "KeyH" => Some(35),
        "KeyI" => Some(23),
        "KeyJ" => Some(36),
        "KeyK" => Some(37),
        "KeyL" => Some(38),
        "KeyM" => Some(50),
        "KeyN" => Some(49),
        "KeyO" => Some(24),
        "KeyP" => Some(25),
        "KeyQ" => Some(16),
        "KeyR" => Some(19),
        "KeyS" => Some(31),
        "KeyT" => Some(20),
        "KeyU" => Some(22),
        "KeyV" => Some(47),
        "KeyW" => Some(17),
        "KeyX" => Some(45),
        "KeyY" => Some(21),
        "KeyZ" => Some(44),

        // Digits
        "Digit1" => Some(2),
        "Digit2" => Some(3),
        "Digit3" => Some(4),
        "Digit4" => Some(5),
        "Digit5" => Some(6),
        "Digit6" => Some(7),
        "Digit7" => Some(8),
        "Digit8" => Some(9),
        "Digit9" => Some(10),
        "Digit0" => Some(11),

        // Punctuation & symbols
        "Minus" => Some(12),
        "Equal" => Some(13),
        "BracketLeft" => Some(26),
        "BracketRight" => Some(27),
        "Backslash" => Some(43),
        "Semicolon" => Some(39),
        "Quote" => Some(40),
        "Backquote" => Some(41),
        "Comma" => Some(51),
        "Period" => Some(52),
        "Slash" => Some(53),

        // Control & editing keys
        "Escape" | "escape" | "esc" => Some(1),
        "Backspace" | "backspace" | "bksp" => Some(14),
        "Tab" | "tab" => Some(15),
        "Enter" | "NumpadEnter" | "enter" | "ok" => Some(28),
        "Space" | "space" => Some(57),
        "CapsLock" | "capslock" => Some(58),
        "PrintScreen" | "printscreen" => Some(99),
        "Home" | "home" => Some(102),
        "ArrowUp" | "up" => Some(103),
        "PageUp" | "pageup" | "pgup" => Some(104),
        "ArrowLeft" | "left" => Some(105),
        "ArrowRight" | "right" => Some(106),
        "End" | "end" => Some(107),
        "ArrowDown" | "down" => Some(108),
        "PageDown" | "pagedown" | "pgdn" => Some(109),
        "Insert" | "insert" | "ins" => Some(110),
        "Delete" | "delete" | "del" => Some(111),
        "ContextMenu" | "menu" => Some(127),

        // Modifiers
        "ControlLeft" | "ctrl" | "control" => Some(29),
        "ControlRight" => Some(97),
        "ShiftLeft" | "shift" => Some(42),
        "ShiftRight" => Some(54),
        "AltLeft" | "alt" => Some(56),
        "AltRight" => Some(100),
        "MetaLeft" | "OSLeft" | "meta" | "super" | "win" | "cmd" => Some(125),
        "MetaRight" | "OSRight" => Some(126),

        // Function keys
        "F1" | "f1" => Some(59),
        "F2" | "f2" => Some(60),
        "F3" | "f3" => Some(61),
        "F4" | "f4" => Some(62),
        "F5" | "f5" => Some(63),
        "F6" | "f6" => Some(64),
        "F7" | "f7" => Some(65),
        "F8" | "f8" => Some(66),
        "F9" | "f9" => Some(67),
        "F10" | "f10" => Some(68),
        "F11" | "f11" => Some(87),
        "F12" | "f12" => Some(88),

        // Single ASCII letter/digit shorthand (e.g. "a".."z", "0".."9")
        other if other.len() == 1 => {
            let b = other.as_bytes()[0];
            match b {
                b'a'..=b'z' | b'A'..=b'Z' => ascii_to_evdev(b.to_ascii_lowercase() as char).map(|(k, _)| k),
                b'0'..=b'9' => ascii_to_evdev(b as char).map(|(k, _)| k),
                _ => None,
            }
        }

        _ => None,
    }
}

/// Maps remote media action identifiers to Linux `evdev` media keycodes.
pub fn media_action_to_evdev(action: &str) -> Option<u16> {
    match action.trim().to_ascii_lowercase().as_str() {
        "mute" => Some(113),            // KEY_MUTE
        "vol_down" => Some(114),        // KEY_VOLUMEDOWN
        "vol_up" => Some(115),          // KEY_VOLUMEUP
        "next" => Some(163),            // KEY_NEXTSONG
        "play_pause" => Some(164),      // KEY_PLAYPAUSE
        "prev" => Some(165),            // KEY_PREVIOUSSONG
        "stop" => Some(166),            // KEY_STOPCD
        "rewind" => Some(168),          // KEY_REWIND
        "fast_forward" => Some(208),    // KEY_FASTFORWARD
        "brightness_down" => Some(224), // KEY_BRIGHTNESSDOWN
        "brightness_up" => Some(225),   // KEY_BRIGHTNESSUP
        _ => None,
    }
}

/// Maps printable US-QWERTY ASCII characters (plus `\n` and `\t`) to `(evdev_scancode, needs_shift)`.
pub fn ascii_to_evdev(ch: char) -> Option<(u16, bool)> {
    match ch {
        '\n' | '\r' => Some((28, false)),
        '\t' => Some((15, false)),
        ' ' => Some((57, false)),

        'a' => Some((30, false)),
        'b' => Some((48, false)),
        'c' => Some((46, false)),
        'd' => Some((32, false)),
        'e' => Some((18, false)),
        'f' => Some((33, false)),
        'g' => Some((34, false)),
        'h' => Some((35, false)),
        'i' => Some((23, false)),
        'j' => Some((36, false)),
        'k' => Some((37, false)),
        'l' => Some((38, false)),
        'm' => Some((50, false)),
        'n' => Some((49, false)),
        'o' => Some((24, false)),
        'p' => Some((25, false)),
        'q' => Some((16, false)),
        'r' => Some((19, false)),
        's' => Some((31, false)),
        't' => Some((20, false)),
        'u' => Some((22, false)),
        'v' => Some((47, false)),
        'w' => Some((17, false)),
        'x' => Some((45, false)),
        'y' => Some((21, false)),
        'z' => Some((44, false)),

        'A' => Some((30, true)),
        'B' => Some((48, true)),
        'C' => Some((46, true)),
        'D' => Some((32, true)),
        'E' => Some((18, true)),
        'F' => Some((33, true)),
        'G' => Some((34, true)),
        'H' => Some((35, true)),
        'I' => Some((23, true)),
        'J' => Some((36, true)),
        'K' => Some((37, true)),
        'L' => Some((38, true)),
        'M' => Some((50, true)),
        'N' => Some((49, true)),
        'O' => Some((24, true)),
        'P' => Some((25, true)),
        'Q' => Some((16, true)),
        'R' => Some((19, true)),
        'S' => Some((31, true)),
        'T' => Some((20, true)),
        'U' => Some((22, true)),
        'V' => Some((47, true)),
        'W' => Some((17, true)),
        'X' => Some((45, true)),
        'Y' => Some((21, true)),
        'Z' => Some((44, true)),

        '1' => Some((2, false)),
        '2' => Some((3, false)),
        '3' => Some((4, false)),
        '4' => Some((5, false)),
        '5' => Some((6, false)),
        '6' => Some((7, false)),
        '7' => Some((8, false)),
        '8' => Some((9, false)),
        '9' => Some((10, false)),
        '0' => Some((11, false)),

        '!' => Some((2, true)),
        '@' => Some((3, true)),
        '#' => Some((4, true)),
        '$' => Some((5, true)),
        '%' => Some((6, true)),
        '^' => Some((7, true)),
        '&' => Some((8, true)),
        '*' => Some((9, true)),
        '(' => Some((10, true)),
        ')' => Some((11, true)),

        '-' => Some((12, false)),
        '_' => Some((12, true)),
        '=' => Some((13, false)),
        '+' => Some((13, true)),
        '[' => Some((26, false)),
        '{' => Some((26, true)),
        ']' => Some((27, false)),
        '}' => Some((27, true)),
        '\\' => Some((43, false)),
        '|' => Some((43, true)),
        ';' => Some((39, false)),
        ':' => Some((39, true)),
        '\'' => Some((40, false)),
        '"' => Some((40, true)),
        '`' => Some((41, false)),
        '~' => Some((41, true)),
        ',' => Some((51, false)),
        '<' => Some((51, true)),
        '.' => Some((52, false)),
        '>' => Some((52, true)),
        '/' => Some((53, false)),
        '?' => Some((53, true)),

        _ => None,
    }
}

/// Maps Linux `evdev` scancodes to Windows `(VK_*, is_extended)` pairs.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn evdev_to_windows_vk(code: u16) -> Option<(u16, bool)> {
    match code {
        1 => Some((0x1B, false)),   // KEY_ESC -> VK_ESCAPE
        2 => Some((0x31, false)),   // 1
        3 => Some((0x32, false)),   // 2
        4 => Some((0x33, false)),   // 3
        5 => Some((0x34, false)),   // 4
        6 => Some((0x35, false)),   // 5
        7 => Some((0x36, false)),   // 6
        8 => Some((0x37, false)),   // 7
        9 => Some((0x38, false)),   // 8
        10 => Some((0x39, false)),  // 9
        11 => Some((0x30, false)),  // 0
        12 => Some((0xBD, false)),  // -_
        13 => Some((0xBB, false)),  // =+
        14 => Some((0x08, false)),  // Backspace
        15 => Some((0x09, false)),  // Tab
        16 => Some((0x51, false)),  // Q
        17 => Some((0x57, false)),  // W
        18 => Some((0x45, false)),  // E
        19 => Some((0x52, false)),  // R
        20 => Some((0x54, false)),  // T
        21 => Some((0x59, false)),  // Y
        22 => Some((0x55, false)),  // U
        23 => Some((0x49, false)),  // I
        24 => Some((0x4F, false)),  // O
        25 => Some((0x50, false)),  // P
        26 => Some((0xDB, false)),  // [{
        27 => Some((0xDD, false)),  // ]}
        28 => Some((0x0D, false)),  // Enter
        29 => Some((0xA2, false)),  // Left Ctrl
        30 => Some((0x41, false)),  // A
        31 => Some((0x53, false)),  // S
        32 => Some((0x44, false)),  // D
        33 => Some((0x46, false)),  // F
        34 => Some((0x47, false)),  // G
        35 => Some((0x48, false)),  // H
        36 => Some((0x4A, false)),  // J
        37 => Some((0x4B, false)),  // K
        38 => Some((0x4C, false)),  // L
        39 => Some((0xBA, false)),  // ;:
        40 => Some((0xDE, false)),  // '"
        41 => Some((0xC0, false)),  // `~
        42 => Some((0xA0, false)),  // Left Shift
        43 => Some((0xDC, false)),  // \|
        44 => Some((0x5A, false)),  // Z
        45 => Some((0x58, false)),  // X
        46 => Some((0x43, false)),  // C
        47 => Some((0x56, false)),  // V
        48 => Some((0x42, false)),  // B
        49 => Some((0x4E, false)),  // N
        50 => Some((0x4D, false)),  // M
        51 => Some((0xBC, false)),  // ,<
        52 => Some((0xBE, false)),  // .>
        53 => Some((0xBF, false)),  // /?
        54 => Some((0xA1, false)),  // Right Shift
        56 => Some((0xA4, false)),  // Left Alt
        57 => Some((0x20, false)),  // Space
        58 => Some((0x14, false)),  // CapsLock
        59 => Some((0x70, false)),  // F1
        60 => Some((0x71, false)),  // F2
        61 => Some((0x72, false)),  // F3
        62 => Some((0x73, false)),  // F4
        63 => Some((0x74, false)),  // F5
        64 => Some((0x75, false)),  // F6
        65 => Some((0x76, false)),  // F7
        66 => Some((0x77, false)),  // F8
        67 => Some((0x78, false)),  // F9
        68 => Some((0x79, false)),  // F10
        87 => Some((0x7A, false)),  // F11
        88 => Some((0x7B, false)),  // F12
        97 => Some((0xA3, true)),   // Right Ctrl
        99 => Some((0x2C, true)),   // PrintScreen
        100 => Some((0xA5, true)),  // Right Alt
        102 => Some((0x24, true)),  // Home
        103 => Some((0x26, true)),  // Up Arrow
        104 => Some((0x21, true)),  // PageUp
        105 => Some((0x25, true)),  // Left Arrow
        106 => Some((0x27, true)),  // Right Arrow
        107 => Some((0x23, true)),  // End
        108 => Some((0x28, true)),  // Down Arrow
        109 => Some((0x22, true)),  // PageDown
        110 => Some((0x2D, true)),  // Insert
        111 => Some((0x2E, true)),  // Delete
        113 => Some((0xAD, false)), // VK_VOLUME_MUTE
        114 => Some((0xAE, false)), // VK_VOLUME_DOWN
        115 => Some((0xAF, false)), // VK_VOLUME_UP
        125 => Some((0x5B, true)),  // Left Win
        126 => Some((0x5C, true)),  // Right Win
        127 => Some((0x5D, true)),  // Menu / Apps
        163 => Some((0xB0, false)), // VK_MEDIA_NEXT_TRACK
        164 => Some((0xB3, false)), // VK_MEDIA_PLAY_PAUSE
        165 => Some((0xB1, false)), // VK_MEDIA_PREV_TRACK
        166 => Some((0xB2, false)), // VK_MEDIA_STOP
        _ => None,
    }
}

/// Maps Linux `evdev` scancodes to macOS `CGKeyCode`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn evdev_to_macos_keycode(code: u16) -> Option<u16> {
    match code {
        1 => Some(53),    // Escape
        2 => Some(18),    // 1
        3 => Some(19),    // 2
        4 => Some(20),    // 3
        5 => Some(21),    // 4
        6 => Some(23),    // 5
        7 => Some(22),    // 6
        8 => Some(26),    // 7
        9 => Some(28),    // 8
        10 => Some(25),   // 9
        11 => Some(29),   // 0
        12 => Some(27),   // -
        13 => Some(24),   // =
        14 => Some(51),   // Backspace
        15 => Some(48),   // Tab
        16 => Some(12),   // Q
        17 => Some(13),   // W
        18 => Some(14),   // E
        19 => Some(15),   // R
        20 => Some(17),   // T
        21 => Some(16),   // Y
        22 => Some(32),   // U
        23 => Some(34),   // I
        24 => Some(31),   // O
        25 => Some(35),   // P
        26 => Some(33),   // [
        27 => Some(30),   // ]
        28 => Some(36),   // Return
        29 => Some(59),   // Left Control
        30 => Some(0),    // A
        31 => Some(1),    // S
        32 => Some(2),    // D
        33 => Some(3),    // F
        34 => Some(5),    // G
        35 => Some(4),    // H
        36 => Some(38),   // J
        37 => Some(40),   // K
        38 => Some(37),   // L
        39 => Some(41),   // ;
        40 => Some(39),   // '
        41 => Some(50),   // `
        42 => Some(56),   // Left Shift
        43 => Some(42),   // \
        44 => Some(6),    // Z
        45 => Some(7),    // X
        46 => Some(8),    // C
        47 => Some(9),    // V
        48 => Some(11),   // B
        49 => Some(45),   // N
        50 => Some(46),   // M
        51 => Some(43),   // ,
        52 => Some(47),   // .
        53 => Some(44),   // /
        54 => Some(60),   // Right Shift
        56 => Some(58),   // Left Option/Alt
        57 => Some(49),   // Space
        58 => Some(57),   // CapsLock
        59 => Some(122),  // F1
        60 => Some(120),  // F2
        61 => Some(99),   // F3
        62 => Some(118),  // F4
        63 => Some(96),   // F5
        64 => Some(97),   // F6
        65 => Some(98),   // F7
        66 => Some(100),  // F8
        67 => Some(101),  // F9
        68 => Some(109),  // F10
        87 => Some(103),  // F11
        88 => Some(111),  // F12
        97 => Some(62),   // Right Control
        100 => Some(61),  // Right Option/Alt
        102 => Some(115), // Home
        103 => Some(126), // Up Arrow
        104 => Some(116), // PageUp
        105 => Some(123), // Left Arrow
        106 => Some(124), // Right Arrow
        107 => Some(119), // End
        108 => Some(125), // Down Arrow
        109 => Some(121), // PageDown
        111 => Some(117), // Forward Delete
        125 => Some(55),  // Left Command
        126 => Some(54),  // Right Command
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_web_key_to_evdev() {
        assert_eq!(web_key_to_evdev("KeyA"), Some(30));
        assert_eq!(web_key_to_evdev("Enter"), Some(28));
        assert_eq!(web_key_to_evdev("enter"), Some(28));
        assert_eq!(web_key_to_evdev("ArrowUp"), Some(103));
        assert_eq!(web_key_to_evdev("up"), Some(103));
        assert_eq!(web_key_to_evdev("ctrl"), Some(29));
        assert_eq!(web_key_to_evdev("super"), Some(125));
        assert_eq!(web_key_to_evdev("F11"), Some(87));
        assert_eq!(web_key_to_evdev("f"), Some(33));
        assert_eq!(web_key_to_evdev("UnknownFoo"), None);
    }

    #[test]
    fn test_media_actions() {
        assert_eq!(media_action_to_evdev("mute"), Some(113));
        assert_eq!(media_action_to_evdev("vol_down"), Some(114));
        assert_eq!(media_action_to_evdev("vol_up"), Some(115));
        assert_eq!(media_action_to_evdev("next"), Some(163));
        assert_eq!(media_action_to_evdev("play_pause"), Some(164));
        assert_eq!(media_action_to_evdev("prev"), Some(165));
        assert_eq!(media_action_to_evdev("stop"), Some(166));
        assert_eq!(media_action_to_evdev("rewind"), Some(168));
        assert_eq!(media_action_to_evdev("fast_forward"), Some(208));
        assert_eq!(media_action_to_evdev("brightness_down"), Some(224));
        assert_eq!(media_action_to_evdev("brightness_up"), Some(225));
    }

    #[test]
    fn test_ascii_mapping() {
        assert_eq!(ascii_to_evdev('a'), Some((30, false)));
        assert_eq!(ascii_to_evdev('A'), Some((30, true)));
        assert_eq!(ascii_to_evdev('!'), Some((2, true)));
        assert_eq!(ascii_to_evdev('?'), Some((53, true)));
        assert_eq!(ascii_to_evdev('\n'), Some((28, false)));
        assert_eq!(ascii_to_evdev(' '), Some((57, false)));
        assert_eq!(ascii_to_evdev('é'), None);
    }

    #[test]
    fn test_windows_and_macos_tables() {
        assert_eq!(evdev_to_windows_vk(30), Some((0x41, false)));
        assert_eq!(evdev_to_windows_vk(103), Some((0x26, true)));
        assert_eq!(evdev_to_windows_vk(164), Some((0xB3, false)));
        assert_eq!(evdev_to_windows_vk(115), Some((0xAF, false)));

        assert_eq!(evdev_to_macos_keycode(30), Some(0));
        assert_eq!(evdev_to_macos_keycode(28), Some(36));
        assert_eq!(evdev_to_macos_keycode(125), Some(55));
    }
}
