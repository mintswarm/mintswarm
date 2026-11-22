#!/usr/bin/env python3
"""Catch the redirect of a GitHub App manifest flow on 127.0.0.1 and print its code.

    printf '%s\\n' "$state" | python3 -I manifest-listener.py PORT TIMEOUT_SECS PAGE

Run by store-credentials.sh create, which holds the state and gives it on stdin
as one line of 32 hex digits; PAGE is only named in the message that says where
to go. The listener binds 127.0.0.1:PORT, and only that address, without
SO_REUSEADDR, so a port that any other socket holds (a wildcard listener
included) is refused. After binding it writes

    listening on 127.0.0.1:PORT; open PAGE in the browser

to stderr. It then serves one connection at a time until TIMEOUT_SECS have
passed since it started. Both bounds are absolute: a connection is dropped 10
seconds after it was accepted, or at the overall deadline if that comes first,
however slowly its client sends, because every read waits only for the time
left. A client that holds a connection open can delay the redirect by at most
those 10 seconds, and never keeps the listener past TIMEOUT_SECS.

It accepts the first request that is a GET of the path / with a single Host
header of exactly 127.0.0.1:PORT or localhost:PORT, and with exactly one code
and one state parameter and nothing else, whose state equals the expected one
(compared in constant time) and whose code matches [A-Za-z0-9._-]{1,256}. That
request gets a 200 plain-text page saying to return to the terminal, with
Cache-Control: no-store, Referrer-Policy: no-referrer and Connection: close; the
code is written to stdout, followed by a newline, and the listener exits.

Every other request gets a 400 (or the status the HTTP parser chose) with the
fixed body "rejected", and one line on stderr naming only the reason; the
listener keeps waiting. Nothing it writes, to stderr or in a response, holds
the code, the state or any part of a request: there is no request log, the
parser's error messages are dropped and an error in a handler prints one fixed
line without a traceback.

Exits 0 with the code on stdout; 1 on timeout ("no matching redirect within N
seconds"), when the port cannot be bound or when stdin cannot be read, with
nothing on stdout; 2 on bad usage or a malformed state; 130 when interrupted.
Standard library only; Python 3.9 or later.
"""

import hmac
import io
import re
import socket
import socketserver
import sys
import time
import typing
from http.server import BaseHTTPRequestHandler
from urllib.parse import parse_qsl

# Used with fullmatch: "$" would also match before a trailing newline.
CODE = re.compile(r"[A-Za-z0-9._-]{1,256}")
STATE = re.compile(r"[0-9a-f]{32}")
REQUEST_TIMEOUT = 10
REJECTED = b"rejected\n"
RECEIVED = b"received; return to the terminal\n"


def say(message):
    sys.stderr.write("manifest-listener.py: %s\n" % message)
    sys.stderr.flush()


class UsageError(Exception):
    pass


class DeadlineReader(io.RawIOBase):
    """The connection's bytes, where every read waits only until an absolute deadline."""

    def __init__(self, sock, deadline):
        io.RawIOBase.__init__(self)
        self.sock = sock
        self.deadline = deadline

    def readable(self):
        return True

    def readinto(self, buffer):
        left = self.deadline - time.monotonic()
        if left <= 0:
            raise socket.timeout("connection deadline passed")
        self.sock.settimeout(left)
        return self.sock.recv_into(buffer)


class Handler(BaseHTTPRequestHandler):
    server_version = "manifest-listener"
    sys_version = ""
    timeout = REQUEST_TIMEOUT

    def setup(self):
        BaseHTTPRequestHandler.setup(self)
        server = typing.cast("Server", self.server)
        deadline = min(time.monotonic() + REQUEST_TIMEOUT, server.deadline)
        # The parser's reads go through this, so no client can stretch the bound
        # by sending a byte at a time; socket.timeout ends the request silently.
        self.rfile.close()
        self.rfile = io.BufferedReader(DeadlineReader(self.connection, deadline))

    def log_message(self, format, *args):  # no request log: the request line holds the code
        pass

    def send_error(self, code, message=None, explain=None):
        # The parser's messages can echo the request line; send a fixed body instead.
        say("rejected a malformed or unsupported request")
        self.respond(code, REJECTED)

    def respond(self, status, body):
        self.close_connection = True
        self.send_response(status)
        self.send_header("Content-Type", "text/plain; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.send_header("Referrer-Policy", "no-referrer")
        self.send_header("Connection", "close")
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(body)

    def reject(self, reason):
        say("rejected a request: %s" % reason)
        self.respond(400, REJECTED)

    def do_GET(self):
        server = typing.cast("Server", self.server)
        port = server.port
        target = self.path
        path, _, query = target.partition("?")
        if path != "/":
            return self.reject("wrong path")
        hosts = self.headers.get_all("Host") or []
        if len(hosts) != 1 or hosts[0] not in ("127.0.0.1:%d" % port, "localhost:%d" % port):
            return self.reject("unexpected Host")
        try:
            params = parse_qsl(query, keep_blank_values=True, strict_parsing=True)
        except ValueError:
            return self.reject("unexpected parameters")
        if sorted(name for name, _ in params) != ["code", "state"]:
            return self.reject("unexpected parameters")
        values = dict(params)
        state = values["state"].encode("utf-8")
        if not hmac.compare_digest(state, server.expected):
            return self.reject("wrong state")
        code = values["code"]
        if not CODE.fullmatch(code):
            return self.reject("malformed code")
        self.respond(200, RECEIVED)
        server.code = code


class Server(socketserver.TCPServer):
    allow_reuse_address = False

    def __init__(self, port, expected, deadline):
        self.port = port
        self.expected = expected
        self.deadline = deadline
        self.code = None  # type: typing.Optional[str]
        socketserver.TCPServer.__init__(self, ("127.0.0.1", port), Handler)

    def handle_error(self, request, client_address):
        say("a request failed; still listening")


def number(text, what, low, high):
    if not text.isdigit() or not (low <= int(text) <= high):
        raise UsageError("%s must be a whole number from %d to %d" % (what, low, high))
    return int(text)


def main(argv):
    if len(argv) != 3 or not argv[2]:
        raise UsageError("usage: manifest-listener.py PORT TIMEOUT_SECS PAGE (the state on stdin)")
    port = number(argv[0], "PORT", 1, 65535)
    limit = number(argv[1], "TIMEOUT_SECS", 1, 86400)
    page = argv[2]
    try:
        line = sys.stdin.readline()
    except (OSError, ValueError):
        say("cannot read the state on stdin")
        return 1
    expected = line.rstrip("\n")
    if not STATE.fullmatch(expected):
        raise UsageError("the state on stdin must be one line of 32 hex digits")

    deadline = time.monotonic() + limit
    try:
        server = Server(port, expected.encode("ascii"), deadline)
    except OSError:
        say("cannot listen on 127.0.0.1:%d: is it in use? After an earlier run, wait a minute and retry" % port)
        return 1
    try:
        host, bound = server.socket.getsockname()[:2]
        say("listening on %s:%d; open %s in the browser" % (host, bound, page))
        while server.code is None:
            left = deadline - time.monotonic()
            if left <= 0:
                say("no matching redirect within %d seconds" % limit)
                return 1
            server.timeout = left
            server.handle_request()
        sys.stdout.write(server.code + "\n")
        sys.stdout.flush()
        server.code = None
        return 0
    finally:
        server.server_close()


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv[1:]))
    except UsageError as e:
        say(str(e))
        sys.exit(2)
    except KeyboardInterrupt:
        say("interrupted")
        sys.exit(130)
