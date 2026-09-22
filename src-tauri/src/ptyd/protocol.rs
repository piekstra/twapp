//! Wire format shared by the ptyd server and its clients.
//!
//! A frame is a 4-byte big-endian payload length, a 1-byte kind, then the
//! payload. Kind [`KIND_CONTROL`] carries one JSON message; kind
//! [`KIND_OUTPUT`] carries an 8-byte big-endian PTY id followed by raw
//! terminal output; kind [`KIND_REPLAY`] has the same layout and carries
//! buffered output resent by an `Attach` with `replay: true`.

use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

pub const PROTOCOL: u32 = 2;

pub const KIND_CONTROL: u8 = 1;
pub const KIND_OUTPUT: u8 = 2;
pub const KIND_REPLAY: u8 = 3;

/// Upper bound on a single frame's payload. Output is chunked well below this;
/// anything larger means a corrupt or hostile stream.
pub const MAX_FRAME: usize = 16 * 1024 * 1024;

pub type PtyId = u64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    Control(Vec<u8>),
    Output { pty: PtyId, data: Vec<u8> },
    Replay { pty: PtyId, data: Vec<u8> },
}

impl Frame {
    pub fn control<T: Serialize>(msg: &T) -> Frame {
        Frame::Control(serde_json::to_vec(msg).expect("protocol messages serialize"))
    }

    pub fn encode(&self) -> Vec<u8> {
        let (kind, payload_len) = match self {
            Frame::Control(body) => (KIND_CONTROL, body.len()),
            Frame::Output { data, .. } => (KIND_OUTPUT, 8 + data.len()),
            Frame::Replay { data, .. } => (KIND_REPLAY, 8 + data.len()),
        };
        let mut out = Vec::with_capacity(5 + payload_len);
        out.extend_from_slice(&(payload_len as u32).to_be_bytes());
        out.push(kind);
        match self {
            Frame::Control(body) => out.extend_from_slice(body),
            Frame::Output { pty, data } | Frame::Replay { pty, data } => {
                out.extend_from_slice(&pty.to_be_bytes());
                out.extend_from_slice(data);
            }
        }
        out
    }
}

pub fn write_frame<W: Write>(w: &mut W, frame: &Frame) -> io::Result<()> {
    w.write_all(&frame.encode())?;
    w.flush()
}

/// Read one frame. `Ok(None)` is a clean end of stream at a frame boundary.
pub fn read_frame<R: Read>(r: &mut R) -> io::Result<Option<Frame>> {
    let mut header = [0u8; 5];
    let mut filled = 0;
    while filled < header.len() {
        match r.read(&mut header[filled..]) {
            Ok(0) if filled == 0 => return Ok(None),
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "truncated frame header",
                ))
            }
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    let len = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
    if len > MAX_FRAME {
        return Err(invalid(format!("frame of {} bytes exceeds the limit", len)));
    }
    let mut payload = vec![0u8; len];
    r.read_exact(&mut payload)?;
    match header[4] {
        KIND_CONTROL => Ok(Some(Frame::Control(payload))),
        kind @ (KIND_OUTPUT | KIND_REPLAY) => {
            if payload.len() < 8 {
                return Err(invalid("output frame shorter than its pty id".to_string()));
            }
            let mut id = [0u8; 8];
            id.copy_from_slice(&payload[..8]);
            let pty = u64::from_be_bytes(id);
            let data = payload[8..].to_vec();
            Ok(Some(if kind == KIND_OUTPUT {
                Frame::Output { pty, data }
            } else {
                Frame::Replay { pty, data }
            }))
        }
        other => Err(invalid(format!("unknown frame kind {}", other))),
    }
}

fn invalid(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    pub body: RequestBody,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RequestBody {
    Hello { protocol: u32 },
    Spawn(SpawnRequest),
    Write { pty: PtyId, data_b64: String },
    Resize { pty: PtyId, rows: u16, cols: u16 },
    Kill { pty: PtyId },
    List,
    Attach { pty: PtyId, replay: bool },
    Detach { pty: PtyId },
    Shutdown { force: bool },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpawnRequest {
    pub session_key: String,
    pub tab: String,
    pub cwd: String,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub prefill: Option<String>,
    #[serde(default)]
    pub env: Vec<(String, String)>,
    pub rows: u16,
    pub cols: u16,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PtyInfo {
    pub id: PtyId,
    pub session_key: String,
    pub tab: String,
    pub cwd: String,
    pub shell_pid: Option<u32>,
    pub started_at: String,
    pub alive: bool,
    pub exit_code: Option<i32>,
    pub idle_ms: u64,
    pub bytes_out: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseBody {
    Hello {
        protocol: u32,
        version: String,
        pid: u32,
    },
    Spawned(PtyInfo),
    List {
        ptys: Vec<PtyInfo>,
    },
    Ok,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseResult {
    Ok(ResponseBody),
    Err(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub id: u64,
    pub result: ResponseResult,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Exited { pty: PtyId, code: Option<i32> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServerMessage {
    Response(Response),
    Event(Event),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn roundtrip(frame: Frame) {
        let bytes = frame.encode();
        let mut cursor = Cursor::new(bytes);
        assert_eq!(read_frame(&mut cursor).unwrap(), Some(frame));
        assert_eq!(read_frame(&mut cursor).unwrap(), None);
    }

    #[test]
    fn control_and_output_frames_roundtrip() {
        roundtrip(Frame::control(&Request {
            id: 7,
            body: RequestBody::List,
        }));
        roundtrip(Frame::Output {
            pty: u64::MAX - 3,
            data: b"\x1b[31mhi\r\n".to_vec(),
        });
        roundtrip(Frame::Output {
            pty: 1,
            data: Vec::new(),
        });
    }

    #[test]
    fn messages_roundtrip_through_json() {
        let req = Request {
            id: 3,
            body: RequestBody::Spawn(SpawnRequest {
                session_key: "/work/a".into(),
                tab: "main".into(),
                cwd: "/work/a".into(),
                command: Some("claude".into()),
                prefill: None,
                env: vec![("A".into(), "1".into())],
                rows: 40,
                cols: 120,
            }),
        };
        let json = serde_json::to_vec(&req).unwrap();
        assert_eq!(serde_json::from_slice::<Request>(&json).unwrap(), req);

        let msg = ServerMessage::Response(Response {
            id: 3,
            result: ResponseResult::Err("nope".into()),
        });
        let json = serde_json::to_vec(&msg).unwrap();
        assert_eq!(serde_json::from_slice::<ServerMessage>(&json).unwrap(), msg);

        let ev = ServerMessage::Event(Event::Exited {
            pty: 9,
            code: Some(0),
        });
        let json = serde_json::to_vec(&ev).unwrap();
        assert_eq!(serde_json::from_slice::<ServerMessage>(&json).unwrap(), ev);
    }

    #[test]
    fn rejects_oversized_frames() {
        let mut bytes = ((MAX_FRAME + 1) as u32).to_be_bytes().to_vec();
        bytes.push(KIND_CONTROL);
        let err = read_frame(&mut Cursor::new(bytes)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn rejects_unknown_kind_and_short_output() {
        let mut bytes = 1u32.to_be_bytes().to_vec();
        bytes.push(9);
        bytes.push(0);
        assert_eq!(
            read_frame(&mut Cursor::new(bytes)).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );

        let mut bytes = 3u32.to_be_bytes().to_vec();
        bytes.push(KIND_OUTPUT);
        bytes.extend_from_slice(&[1, 2, 3]);
        assert_eq!(
            read_frame(&mut Cursor::new(bytes)).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn truncated_frames_are_errors_not_clean_eof() {
        let full = Frame::control(&Request {
            id: 1,
            body: RequestBody::List,
        })
        .encode();
        let err = read_frame(&mut Cursor::new(full[..3].to_vec())).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
        let err = read_frame(&mut Cursor::new(full[..full.len() - 1].to_vec())).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }
}
