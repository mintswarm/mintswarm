"""Domain errors shared by the dogfood tools.

A tool loads this module by path from its own directory with importlib.util,
under the module name `msw_dogfood_errors`, as commit.py loads floor.py; it is
not on any import path. Every message a tool prints is its own: an exception
from the JSON decoder, the operating system or any other library is caught
where it occurs and re-raised as a DomainError `from` it, never shown.

- DomainError: the base class; its message is the text it was given.
- read_phrase and write_phrase: the fixed phrase for an error reading or
  writing a file, the words of `read_text` and `write_failure` in
  crates/runner/src/tool/save_output.rs, chosen by errno.
- decode_position: the line and column where a JSONDecodeError stopped,
  counted from its document and position by this module.
- crash_line: the line a tool prints for an exception it did not expect,
  naming the tool and the exception's class only.

Standard library only.
"""

import errno
import json

NOT_UTF8 = "not UTF-8 text"
NO_SUCH_FILE = "no such file"
PERMISSION_DENIED = "permission denied"
READ_FAILED = "read failed"
PARENT_NOT_A_DIRECTORY = "a parent is not a directory"
DIRECTORY_IN_THE_WAY = "a directory is in the way"
NO_SUCH_DIRECTORY = "no such directory"
WRITE_FAILED = "write failed"

READ_PHRASES = {
    errno.ENOENT: NO_SUCH_FILE,
    errno.EACCES: PERMISSION_DENIED,
    errno.EPERM: PERMISSION_DENIED,
}
WRITE_PHRASES = {
    errno.EACCES: PERMISSION_DENIED,
    errno.EPERM: PERMISSION_DENIED,
    errno.ENOTDIR: PARENT_NOT_A_DIRECTORY,
    errno.EEXIST: PARENT_NOT_A_DIRECTORY,
    errno.EISDIR: DIRECTORY_IN_THE_WAY,
    errno.ENOTEMPTY: DIRECTORY_IN_THE_WAY,
    errno.ENOENT: NO_SUCH_DIRECTORY,
}


class DomainError(Exception):
    """An error whose message is always the tool's own text."""

    def __init__(self, message: str):
        super().__init__(message)
        self.message = message

    def __str__(self) -> str:
        return self.message


def read_phrase(exc: BaseException) -> str:
    """Why a file could not be read: undecodable bytes, or the OSError's errno."""
    if isinstance(exc, UnicodeDecodeError):
        return NOT_UTF8
    if isinstance(exc, OSError):
        return READ_PHRASES.get(exc.errno, READ_FAILED)
    return READ_FAILED


def write_phrase(exc: BaseException) -> str:
    """Why a file could not be written: the OSError's errno; anything else is a failed write."""
    if isinstance(exc, OSError):
        return WRITE_PHRASES.get(exc.errno, WRITE_FAILED)
    return WRITE_FAILED


def decode_position(exc: json.JSONDecodeError) -> tuple:
    """The 1-based (line, column) where decoding stopped, from exc.doc and exc.pos only."""
    before = exc.doc[: exc.pos]
    line = before.count("\n") + 1
    column = exc.pos - (before.rfind("\n") + 1) + 1
    return line, column


def crash_line(tool: str, exc: BaseException) -> str:
    """The line a tool prints for an unexpected exception: the tool and the class, never its text."""
    return f"{tool}: internal error: unexpected {type(exc).__name__}"
