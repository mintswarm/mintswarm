//! The frames between the `msw-vcs` client and the per-attempt server, over the tool directory's
//! Unix socket: one request, then one reply, on each connection.
//!
//! A request is the argument count, then each argument as its length and its bytes. A reply is
//! the exit code as one byte, then stdout and stderr, each as its length and its bytes. Counts
//! and lengths are big-endian: `u32` in a request, `u64` in a reply.

use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

/// The largest request the server reads, its count and lengths included.
pub const MAX_REQUEST: usize = 1 << 20;

/// What a call returns.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reply {
    /// The exit code.
    pub exit: u8,
    /// The bytes for stdout.
    pub stdout: Vec<u8>,
    /// The bytes for stderr.
    pub stderr: Vec<u8>,
}

fn too_large() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "the request is too large")
}

/// The size of the request frame of `argv`.
fn request_size(argv: &[Vec<u8>]) -> usize {
    4 + argv.iter().map(|arg| 4 + arg.len()).sum::<usize>()
}

/// Write the request frame of `argv`; a frame over [`MAX_REQUEST`] is refused unwritten.
pub fn write_request(to: &mut impl Write, argv: &[Vec<u8>]) -> io::Result<()> {
    if request_size(argv) > MAX_REQUEST {
        return Err(too_large());
    }
    let length = |n: usize| u32::try_from(n).map_err(|_| too_large());
    let mut frame = Vec::with_capacity(request_size(argv));
    frame.extend(length(argv.len())?.to_be_bytes());
    for arg in argv {
        frame.extend(length(arg.len())?.to_be_bytes());
        frame.extend_from_slice(arg);
    }
    to.write_all(&frame)?;
    to.flush()
}

/// Read a request frame; a truncated frame or one over [`MAX_REQUEST`] is an error.
pub fn read_request(from: &mut impl Read) -> io::Result<Vec<Vec<u8>>> {
    let mut size = 4;
    let count = read_u32(from)?;
    let mut argv = Vec::new();
    for _ in 0..count {
        let length = read_u32(from)?;
        size += 4 + length;
        if size > MAX_REQUEST {
            return Err(too_large());
        }
        let mut arg = vec![0; length];
        from.read_exact(&mut arg)?;
        argv.push(arg);
    }
    Ok(argv)
}

fn read_u32(from: &mut impl Read) -> io::Result<usize> {
    let mut bytes = [0; 4];
    from.read_exact(&mut bytes)?;
    usize::try_from(u32::from_be_bytes(bytes)).map_err(|_| too_large())
}

/// Write a reply frame.
pub fn write_reply(to: &mut impl Write, reply: &Reply) -> io::Result<()> {
    to.write_all(&[reply.exit])?;
    for part in [&reply.stdout, &reply.stderr] {
        to.write_all(&(part.len() as u64).to_be_bytes())?;
        to.write_all(part)?;
    }
    to.flush()
}

/// Read a reply frame; a truncated frame is an error.
pub fn read_reply(from: &mut impl Read) -> io::Result<Reply> {
    let mut exit = [0; 1];
    from.read_exact(&mut exit)?;
    let mut part = || -> io::Result<Vec<u8>> {
        let mut length = [0; 8];
        from.read_exact(&mut length)?;
        let length = u64::from_be_bytes(length);
        let mut bytes = Vec::new();
        // read without trusting the length for an allocation
        let read = from.by_ref().take(length).read_to_end(&mut bytes)?;
        if read as u64 != length {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        Ok(bytes)
    };
    let stdout = part()?;
    let stderr = part()?;
    Ok(Reply {
        exit: exit[0],
        stdout,
        stderr,
    })
}

/// Call the server at `socket` with `argv` and return its reply.
pub fn call(socket: &Path, argv: &[Vec<u8>]) -> io::Result<Reply> {
    let mut stream = UnixStream::connect(socket)?;
    write_request(&mut stream, argv)?;
    stream.shutdown(std::net::Shutdown::Write)?;
    read_reply(&mut stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip_any_bytes() {
        let argv = vec![b"show".to_vec(), vec![0, 0xff, b'\n', b'"'], Vec::new()];
        let mut frame = Vec::new();
        write_request(&mut frame, &argv).unwrap();
        assert_eq!(read_request(&mut frame.as_slice()).unwrap(), argv);

        let reply = Reply {
            exit: 3,
            stdout: vec![0, 1, 2, 0xfe],
            stderr: b"msw-vcs: x\n".to_vec(),
        };
        let mut frame = Vec::new();
        write_reply(&mut frame, &reply).unwrap();
        assert_eq!(read_reply(&mut frame.as_slice()).unwrap(), reply);
    }

    #[test]
    fn truncated_and_oversized_frames_are_errors() {
        let mut frame = Vec::new();
        write_request(&mut frame, &[b"log".to_vec()]).unwrap();
        for end in 0..frame.len() {
            assert!(read_request(&mut &frame[..end]).is_err(), "cut at {end}");
        }
        let mut frame = Vec::new();
        write_reply(&mut frame, &Reply::default()).unwrap();
        for end in 0..frame.len() {
            assert!(read_reply(&mut &frame[..end]).is_err(), "cut at {end}");
        }

        let huge = vec![vec![b'a'; MAX_REQUEST]];
        assert!(write_request(&mut Vec::new(), &huge).is_err());
        let mut frame = 1u32.to_be_bytes().to_vec();
        frame.extend(u32::try_from(MAX_REQUEST).unwrap().to_be_bytes());
        let error = read_request(&mut frame.as_slice()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}
