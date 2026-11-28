//! The fixed phrases for a file that cannot be read or written, chosen by its
//! [`io::ErrorKind`], so a tool reports a file error in its own words and never the operating
//! system's.
//!
//! The Python counterpart is `.mintswarm/dogfood/errors.py`, whose `read_phrase` and
//! `write_phrase` choose the same phrases by errno.

use std::io;

/// Why a file's bytes are not text: they are not UTF-8.
pub const NOT_UTF8: &str = "not UTF-8 text";

/// Why a file could not be read.
pub fn read_phrase(kind: io::ErrorKind) -> &'static str {
    match kind {
        io::ErrorKind::NotFound => "no such file",
        io::ErrorKind::PermissionDenied => "permission denied",
        _ => "read failed",
    }
}

/// Why a file could not be written.
pub fn write_phrase(kind: io::ErrorKind) -> &'static str {
    match kind {
        io::ErrorKind::PermissionDenied => "permission denied",
        io::ErrorKind::NotADirectory | io::ErrorKind::AlreadyExists => {
            "a parent is not a directory"
        }
        io::ErrorKind::IsADirectory | io::ErrorKind::DirectoryNotEmpty => {
            "a directory is in the way"
        }
        io::ErrorKind::NotFound => "no such directory",
        _ => "write failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_phrases_cover_every_arm() {
        let cases = [
            (io::ErrorKind::NotFound, "no such file"),
            (io::ErrorKind::PermissionDenied, "permission denied"),
            (io::ErrorKind::IsADirectory, "read failed"),
            (io::ErrorKind::Other, "read failed"),
        ];
        for (kind, phrase) in cases {
            assert_eq!(read_phrase(kind), phrase, "{kind:?}");
        }
    }

    #[test]
    fn write_phrases_cover_every_arm() {
        let cases = [
            (io::ErrorKind::PermissionDenied, "permission denied"),
            (io::ErrorKind::NotADirectory, "a parent is not a directory"),
            (io::ErrorKind::AlreadyExists, "a parent is not a directory"),
            (io::ErrorKind::IsADirectory, "a directory is in the way"),
            (
                io::ErrorKind::DirectoryNotEmpty,
                "a directory is in the way",
            ),
            (io::ErrorKind::NotFound, "no such directory"),
            (io::ErrorKind::Other, "write failed"),
        ];
        for (kind, phrase) in cases {
            assert_eq!(write_phrase(kind), phrase, "{kind:?}");
        }
    }

    #[test]
    fn not_utf8_is_the_fixed_phrase() {
        assert_eq!(NOT_UTF8, "not UTF-8 text");
    }
}
