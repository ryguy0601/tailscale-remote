use qrcode::render::svg;
use qrcode::{Color, EcLevel, QrCode};

const BASE62: &[u8; 62] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

/// Generates a cryptographically random Base62 pairing token using OS entropy
/// with rejection sampling to eliminate modulo bias.
pub fn generate_token(len: usize) -> String {
    let mut out = String::with_capacity(len);
    let mut buf = [0u8; 64];
    // 248 is the largest multiple of 62 <= 256 (62 * 4 = 248).
    const LIMIT: u8 = 248;

    while out.len() < len {
        getrandom::getrandom(&mut buf).expect("OS CSPRNG failed");
        for &b in &buf {
            if b < LIMIT {
                out.push(BASE62[(b % 62) as usize] as char);
                if out.len() == len {
                    break;
                }
            }
        }
    }
    out
}

/// Renders a crisp, high-contrast SVG QR code suitable for dark-mode TV/desktop pages.
pub fn generate_svg(data: &str) -> Result<String, String> {
    let code = QrCode::with_error_correction_level(data.as_bytes(), EcLevel::M)
        .map_err(|e| format!("QR encode failed: {e}"))?;
    let svg_xml = code
        .render::<svg::Color>()
        .min_dimensions(340, 340)
        .quiet_zone(true)
        .dark_color(svg::Color("#0b1120"))
        .light_color(svg::Color("#ffffff"))
        .build();
    Ok(svg_xml)
}

/// Renders a compact, scannable Unicode half-block QR code for terminal output
/// with explicit ANSI black-on-bright-white colors and a 2-module quiet zone.
pub fn generate_terminal_qr(data: &str) -> Result<String, String> {
    let code = QrCode::with_error_correction_level(data.as_bytes(), EcLevel::M)
        .map_err(|e| format!("QR encode failed: {e}"))?;
    let w = code.width() as isize;
    let quiet: isize = 2;
    let total = w + quiet * 2;

    let is_dark = |x: isize, y: isize| -> bool {
        let mx = x - quiet;
        let my = y - quiet;
        if mx < 0 || my < 0 || mx >= w || my >= w {
            false
        } else {
            code[(mx as usize, my as usize)] == Color::Dark
        }
    };

    let mut out = String::new();
    let mut y = 0;
    while y < total {
        out.push_str("\x1b[30;107m");
        for x in 0..total {
            let top = is_dark(x, y);
            let bottom = if y + 1 < total {
                is_dark(x, y + 1)
            } else {
                false
            };
            let ch = match (top, bottom) {
                (true, true) => '█',
                (true, false) => '▀',
                (false, true) => '▄',
                (false, false) => ' ',
            };
            out.push(ch);
        }
        out.push_str("\x1b[0m\n");
        y += 2;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_token_properties() {
        let t1 = generate_token(12);
        let t2 = generate_token(12);
        assert_eq!(t1.len(), 12);
        assert_eq!(t2.len(), 12);
        assert_ne!(t1, t2);
        assert!(t1.bytes().all(|b| BASE62.contains(&b)));
    }

    #[test]
    fn test_generate_svg_and_terminal_qr() {
        let url = "http://100.64.0.10:9880/?t=AbCdEf123456";
        let svg = generate_svg(url).expect("SVG QR should succeed");
        assert!(svg.contains("<svg"));
        assert!(svg.contains("</svg>"));
        assert!(svg.contains("#0b1120"));

        let term = generate_terminal_qr(url).expect("Terminal QR should succeed");
        let lines: Vec<&str> = term.lines().collect();
        assert!(lines.len() >= 10);
        assert!(lines.iter().all(|l| l.starts_with("\x1b[30;107m")));
        assert!(lines.iter().all(|l| l.ends_with("\x1b[0m")));
    }
}
