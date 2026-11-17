//! Length-prefixed frames: a 4-byte little-endian body length, then the body (JSON).

use serde::Serialize;
use std::io::{self, Read, Write};

/// Largest body accepted. Settings with a long personal dictionary are the biggest message
/// (tens of KB); anything near this is a bug or a hostile peer.
pub const MAX_FRAME: usize = 4 * 1024 * 1024;

/// Write one message as one frame, with a single `write_all`, so a writer behind a lock never
/// interleaves with another.
pub fn write<W: Write, T: Serialize>(w: &mut W, message: &T) -> io::Result<()> {
    let body = serde_json::to_vec(message).map_err(io::Error::other)?;
    if body.len() > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "message too large",
        ));
    }
    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame.extend_from_slice(&body);
    w.write_all(&frame)?;
    w.flush()
}

/// Read one frame's body. `Ok(None)` at a clean end of stream (between frames).
pub fn read<R: Read>(r: &mut R) -> io::Result<Option<Vec<u8>>> {
    let mut len = [0u8; 4];
    let mut got = 0;
    while got < len.len() {
        match r.read(&mut len[got..]) {
            Ok(0) if got == 0 => return Ok(None),
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame of {len} bytes is over the limit"),
        ));
    }
    let mut body = vec![0; len];
    r.read_exact(&mut body)?;
    Ok(Some(body))
}

/// Decode a frame body. A message this side doesn't know is an error the caller may skip: the
/// next frame still starts in the right place.
pub fn decode<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, serde_json::Error> {
    serde_json::from_slice(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{FromEngine, PROTOCOL};
    use std::io::Cursor;

    fn next(r: &mut Cursor<Vec<u8>>) -> Result<FromEngine, serde_json::Error> {
        decode(&read(r).unwrap().unwrap())
    }

    #[test]
    fn frames_round_trip_in_order() {
        let mut buf = Vec::new();
        let a = FromEngine::Hello {
            protocol: PROTOCOL,
            version: "1".into(),
        };
        let b = FromEngine::WordsChecked { count: 3 };
        write(&mut buf, &a).unwrap();
        write(&mut buf, &b).unwrap();
        let mut r = Cursor::new(buf);
        assert_eq!(next(&mut r).unwrap(), a);
        assert_eq!(next(&mut r).unwrap(), b);
        assert!(read(&mut r).unwrap().is_none());
    }

    #[test]
    fn rejects_oversized_and_truncated_frames() {
        let kind = |bytes: Vec<u8>| read(&mut Cursor::new(bytes)).unwrap_err().kind();
        let huge = ((MAX_FRAME + 1) as u32).to_le_bytes().to_vec();
        assert_eq!(kind(huge), io::ErrorKind::InvalidData);
        assert_eq!(kind(vec![10, 0, 0, 0, b'{']), io::ErrorKind::UnexpectedEof);
        assert_eq!(kind(vec![10, 0]), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn an_unknown_message_is_skipped_without_losing_the_stream() {
        let mut buf = Vec::new();
        write(&mut buf, &serde_json::json!({"type": "from-the-future"})).unwrap();
        write(&mut buf, &FromEngine::HookReinstalled).unwrap();
        let mut r = Cursor::new(buf);
        assert!(next(&mut r).is_err());
        assert_eq!(next(&mut r).unwrap(), FromEngine::HookReinstalled);
    }
}
