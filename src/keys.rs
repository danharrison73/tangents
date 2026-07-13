//! Translate crossterm key events back into the raw terminal byte sequences a
//! PTY child expects. crossterm parses stdin into structured events; claude,
//! living behind the PTY, wants the original bytes — so we re-encode.
//!
//! This targets the legacy xterm encoding (no kitty enhancement), which matches
//! the plainly-negotiated terminal we hand claude. Decorative protocols are the
//! documented known-limitation of the PTY-wrapper approach.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Encode a key press into bytes for the PTY. Returns `None` for keys we do not
/// forward (e.g. lone modifier presses).
pub fn encode_key(key: &KeyEvent) -> Option<Vec<u8>> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let mut out: Vec<u8> = Vec::new();

    match key.code {
        KeyCode::Char(c) => {
            if ctrl {
                let b = control_byte(c)?;
                if alt {
                    out.push(0x1b);
                }
                out.push(b);
            } else {
                if alt {
                    out.push(0x1b);
                }
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
        }
        KeyCode::Enter => {
            if alt {
                out.push(0x1b);
            }
            out.push(b'\r');
        }
        KeyCode::Tab => out.push(b'\t'),
        KeyCode::BackTab => out.extend_from_slice(b"\x1b[Z"),
        KeyCode::Backspace => {
            if alt {
                out.push(0x1b);
            }
            out.push(0x7f);
        }
        KeyCode::Esc => out.push(0x1b),
        KeyCode::Up => csi_letter(&mut out, 'A', shift, alt, ctrl),
        KeyCode::Down => csi_letter(&mut out, 'B', shift, alt, ctrl),
        KeyCode::Right => csi_letter(&mut out, 'C', shift, alt, ctrl),
        KeyCode::Left => csi_letter(&mut out, 'D', shift, alt, ctrl),
        KeyCode::Home => csi_letter(&mut out, 'H', shift, alt, ctrl),
        KeyCode::End => csi_letter(&mut out, 'F', shift, alt, ctrl),
        KeyCode::Insert => csi_tilde(&mut out, 2, shift, alt, ctrl),
        KeyCode::Delete => csi_tilde(&mut out, 3, shift, alt, ctrl),
        KeyCode::PageUp => csi_tilde(&mut out, 5, shift, alt, ctrl),
        KeyCode::PageDown => csi_tilde(&mut out, 6, shift, alt, ctrl),
        KeyCode::F(n) => encode_function_key(&mut out, n, shift, alt, ctrl),
        KeyCode::Null => out.push(0),
        _ => return None,
    }
    Some(out)
}

/// Map a character to its Ctrl-modified control byte.
fn control_byte(c: char) -> Option<u8> {
    match c {
        ' ' | '@' | '2' => Some(0x00),
        'a'..='z' => Some(c as u8 - b'a' + 1),
        'A'..='Z' => Some(c as u8 - b'A' + 1),
        '[' | '3' => Some(0x1b),
        '\\' | '4' => Some(0x1c),
        ']' | '5' => Some(0x1d),
        '^' | '6' => Some(0x1e),
        '_' | '7' => Some(0x1f),
        '8' => Some(0x7f),
        '?' => Some(0x7f),
        _ => None,
    }
}

/// xterm modifier parameter: 1 + sum of active modifier bits.
fn mod_code(shift: bool, alt: bool, ctrl: bool) -> u8 {
    1 + (shift as u8) + ((alt as u8) << 1) + ((ctrl as u8) << 2)
}

/// CSI sequences ending in a letter (arrows, Home, End).
fn csi_letter(out: &mut Vec<u8>, letter: char, shift: bool, alt: bool, ctrl: bool) {
    let m = mod_code(shift, alt, ctrl);
    if m == 1 {
        out.extend_from_slice(format!("\x1b[{letter}").as_bytes());
    } else {
        out.extend_from_slice(format!("\x1b[1;{m}{letter}").as_bytes());
    }
}

/// CSI sequences of the form `ESC [ <n> ~` (Ins/Del/PgUp/PgDn).
fn csi_tilde(out: &mut Vec<u8>, n: u8, shift: bool, alt: bool, ctrl: bool) {
    let m = mod_code(shift, alt, ctrl);
    if m == 1 {
        out.extend_from_slice(format!("\x1b[{n}~").as_bytes());
    } else {
        out.extend_from_slice(format!("\x1b[{n};{m}~").as_bytes());
    }
}

fn encode_function_key(out: &mut Vec<u8>, n: u8, shift: bool, alt: bool, ctrl: bool) {
    let m = mod_code(shift, alt, ctrl);
    // F1-F4 use SS3 (ESC O) when unmodified, CSI 1 ; m {P..S} otherwise.
    let ss3 = |out: &mut Vec<u8>, last: char| {
        if m == 1 {
            out.extend_from_slice(format!("\x1bO{last}").as_bytes());
        } else {
            out.extend_from_slice(format!("\x1b[1;{m}{last}").as_bytes());
        }
    };
    match n {
        1 => ss3(out, 'P'),
        2 => ss3(out, 'Q'),
        3 => ss3(out, 'R'),
        4 => ss3(out, 'S'),
        // F5-F12 use the CSI tilde form.
        5 => csi_tilde(out, 15, shift, alt, ctrl),
        6 => csi_tilde(out, 17, shift, alt, ctrl),
        7 => csi_tilde(out, 18, shift, alt, ctrl),
        8 => csi_tilde(out, 19, shift, alt, ctrl),
        9 => csi_tilde(out, 20, shift, alt, ctrl),
        10 => csi_tilde(out, 21, shift, alt, ctrl),
        11 => csi_tilde(out, 23, shift, alt, ctrl),
        12 => csi_tilde(out, 24, shift, alt, ctrl),
        _ => {}
    }
}

/// Wrap pasted text in bracketed-paste markers, as a modern TUI expects.
pub fn encode_paste(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + 12);
    out.extend_from_slice(b"\x1b[200~");
    out.extend_from_slice(text.as_bytes());
    out.extend_from_slice(b"\x1b[201~");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventKind;

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn plain_char() {
        assert_eq!(
            encode_key(&key(KeyCode::Char('a'), KeyModifiers::NONE)),
            Some(vec![b'a'])
        );
    }

    #[test]
    fn ctrl_c_is_etx() {
        assert_eq!(
            encode_key(&key(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(vec![0x03])
        );
    }

    #[test]
    fn enter_is_cr() {
        assert_eq!(
            encode_key(&key(KeyCode::Enter, KeyModifiers::NONE)),
            Some(vec![b'\r'])
        );
    }

    #[test]
    fn arrows_and_modifiers() {
        assert_eq!(
            encode_key(&key(KeyCode::Up, KeyModifiers::NONE)),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            encode_key(&key(KeyCode::Left, KeyModifiers::SHIFT)),
            Some(b"\x1b[1;2D".to_vec())
        );
    }

    #[test]
    fn alt_char_prefixes_esc() {
        assert_eq!(
            encode_key(&key(KeyCode::Char('x'), KeyModifiers::ALT)),
            Some(vec![0x1b, b'x'])
        );
    }

    #[test]
    fn function_keys() {
        assert_eq!(
            encode_key(&key(KeyCode::F(1), KeyModifiers::NONE)),
            Some(b"\x1bOP".to_vec())
        );
        assert_eq!(
            encode_key(&key(KeyCode::F(5), KeyModifiers::NONE)),
            Some(b"\x1b[15~".to_vec())
        );
    }

    #[test]
    fn press_kind_is_default() {
        // Sanity: KeyEvent::new produces a Press event.
        assert_eq!(
            key(KeyCode::Char('a'), KeyModifiers::NONE).kind,
            KeyEventKind::Press
        );
    }
}
