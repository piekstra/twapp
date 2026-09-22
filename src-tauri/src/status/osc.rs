//! Streaming scanner for the OSC sequences a harness writes to its terminal.
//!
//! Only window titles (OSC 0 and 2) and desktop notifications (OSC 9 and
//! OSC 777) matter to the status engine. Everything else passes through
//! untouched: the scanner never modifies the byte stream, it only watches it.

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;
const MAX_PAYLOAD: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OscEvent {
    Title(String),
    Notify { title: Option<String>, body: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Ground,
    Escape,
    Payload,
    PayloadEscape,
    Overflow,
    OverflowEscape,
}

/// Holds partial state between chunks, so a sequence split across two reads
/// is still recognized.
#[derive(Debug)]
pub struct OscScanner {
    mode: Mode,
    payload: Vec<u8>,
}

impl Default for OscScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl OscScanner {
    pub fn new() -> Self {
        Self {
            mode: Mode::Ground,
            payload: Vec::new(),
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Vec<OscEvent> {
        let mut events = Vec::new();
        for &b in bytes {
            match self.mode {
                Mode::Ground => {
                    if b == ESC {
                        self.mode = Mode::Escape;
                    }
                }
                Mode::Escape => {
                    if b == b']' {
                        self.payload.clear();
                        self.mode = Mode::Payload;
                    } else if b != ESC {
                        self.mode = Mode::Ground;
                    }
                }
                Mode::Payload => match b {
                    BEL => self.finish(&mut events),
                    ESC => self.mode = Mode::PayloadEscape,
                    0x18 | 0x1a => self.mode = Mode::Ground,
                    _ => {
                        if self.payload.len() >= MAX_PAYLOAD {
                            self.payload.clear();
                            self.mode = Mode::Overflow;
                        } else {
                            self.payload.push(b);
                        }
                    }
                },
                Mode::PayloadEscape => {
                    if b == b'\\' {
                        self.finish(&mut events);
                    } else if b == b']' {
                        // An unterminated sequence followed by a new one.
                        self.payload.clear();
                        self.mode = Mode::Payload;
                    } else {
                        self.payload.clear();
                        self.mode = Mode::Ground;
                    }
                }
                Mode::Overflow => match b {
                    BEL | 0x18 | 0x1a => self.mode = Mode::Ground,
                    ESC => self.mode = Mode::OverflowEscape,
                    _ => {}
                },
                Mode::OverflowEscape => {
                    self.mode = if b == b'\\' {
                        Mode::Ground
                    } else {
                        Mode::Overflow
                    };
                }
            }
        }
        events
    }

    fn finish(&mut self, events: &mut Vec<OscEvent>) {
        self.mode = Mode::Ground;
        let payload = String::from_utf8_lossy(&self.payload).into_owned();
        self.payload.clear();
        if let Some(event) = parse_payload(&payload) {
            events.push(event);
        }
    }
}

fn parse_payload(payload: &str) -> Option<OscEvent> {
    let (code, rest) = payload.split_once(';').unwrap_or((payload, ""));
    match code {
        "0" | "2" => Some(OscEvent::Title(rest.to_string())),
        "9" => {
            // ConEmu reuses OSC 9 for numbered sub-commands such as progress
            // (`9;4;1;50`); those are not notifications.
            let sub = rest.split(';').next().unwrap_or("");
            if rest.is_empty() || (!sub.is_empty() && sub.bytes().all(|c| c.is_ascii_digit())) {
                None
            } else {
                Some(OscEvent::Notify {
                    title: None,
                    body: rest.to_string(),
                })
            }
        }
        "777" => {
            let mut parts = rest.splitn(3, ';');
            if parts.next()? != "notify" {
                return None;
            }
            let title = parts.next().map(str::to_string).filter(|t| !t.is_empty());
            let body = parts.next().unwrap_or("").to_string();
            Some(OscEvent::Notify { title, body })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_with_bel_and_st_terminators() {
        let mut s = OscScanner::new();
        assert_eq!(
            s.feed(b"x\x1b]0;\xe2\x97\x90 fix login\x07y"),
            vec![OscEvent::Title("◐ fix login".into())]
        );
        assert_eq!(
            s.feed(b"\x1b]2;plain\x1b\\"),
            vec![OscEvent::Title("plain".into())]
        );
    }

    #[test]
    fn sequence_split_across_chunks() {
        let mut s = OscScanner::new();
        let whole = "\x1b]0;✳ topic\x07".as_bytes();
        let mut events = Vec::new();
        for chunk in whole.chunks(1) {
            events.extend(s.feed(chunk));
        }
        assert_eq!(events, vec![OscEvent::Title("✳ topic".into())]);

        let mut s = OscScanner::new();
        assert!(s.feed(b"\x1b]777;notify;Claude Code;needs").is_empty());
        assert!(s.feed(b" your permission\x1b").is_empty());
        assert_eq!(
            s.feed(b"\\"),
            vec![OscEvent::Notify {
                title: Some("Claude Code".into()),
                body: "needs your permission".into()
            }]
        );
    }

    #[test]
    fn osc9_notification_but_not_conemu_progress() {
        let mut s = OscScanner::new();
        assert_eq!(
            s.feed(b"\x1b]9;Approval requested\x07\x1b]9;4;1;50\x07"),
            vec![OscEvent::Notify {
                title: None,
                body: "Approval requested".into()
            }]
        );
    }

    #[test]
    fn ignores_other_osc_codes_and_csi() {
        let mut s = OscScanner::new();
        assert!(s
            .feed(b"\x1b[2J\x1b]8;;https://x.test\x07link\x1b]8;;\x07\x1b]1337;foo\x07")
            .is_empty());
    }

    #[test]
    fn oversized_payload_is_dropped_and_scanner_recovers() {
        let mut s = OscScanner::new();
        let mut big = b"\x1b]0;".to_vec();
        big.extend(vec![b'a'; MAX_PAYLOAD + 10]);
        big.push(BEL);
        assert!(s.feed(&big).is_empty());
        assert_eq!(s.feed(b"\x1b]0;ok\x07"), vec![OscEvent::Title("ok".into())]);
    }

    #[test]
    fn cancel_byte_aborts_sequence() {
        let mut s = OscScanner::new();
        assert!(s.feed(b"\x1b]0;half\x18rest\x07").is_empty());
        assert_eq!(s.feed(b"\x1b]0;ok\x07"), vec![OscEvent::Title("ok".into())]);
    }
}
