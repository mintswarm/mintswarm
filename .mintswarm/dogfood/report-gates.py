#!/usr/bin/env python3
"""Write a job's gate results in the report formats the repository asks for.

    report-gates.py [--check] [--config PATH]

Dogfooding stand-in for the runner's report step. run-gates.py writes the
canonical result, .mintswarm/job/outputs/gates.ctrf.json; every other format
comes from a converter. The configuration is .mintswarm/config.yaml at the
root of the checkout holding this script (or --config), read with
`yq -o=json`; outputs are under .mintswarm/job/outputs/ relative to the
working directory, which is the worktree root. Agents never read this setting.

    gates:
      report:
        formats: [ctrf, junit]       # ctrf is always written; each other entry names a converter
        converters:                  # optional; a name here overrides a built-in of the same name
          sarif: {command: [python3, .mintswarm/reporters/ctrf-to-sarif.py], extension: sarif, timeout: 300}

Without `report`, formats is [ctrf].

Converter contract:

- A format name matches [a-z0-9][a-z0-9-]*; `ctrf` is reserved and cannot be
  a converter. `command` is a non-empty list of strings (argv, no shell),
  `extension` matches [a-z0-9]+, `timeout` is a positive integer in seconds
  (default 300). No other keys are allowed.
- Converters are resolved from the repository's `converters` first, then the
  built-ins: `junit` is {command: [python3, reporters/ctrf-to-junit.py],
  extension: xml}, its path resolved from this script's directory.
- The converter runs from the worktree root with stdin closed and the
  environment inherited, as `COMMAND... CTRF_IN OUT_PATH`. CTRF_IN is an
  absolute path to a read-only copy of gates.ctrf.json in a temporary
  directory; OUT_PATH is an absolute path to a temporary file next to the
  final one. The converter writes only OUT_PATH (documented, not enforced).
  Its stdout and stderr go to this script's stderr.
- The final file is .mintswarm/job/outputs/gates.<format>.<extension>, for
  example gates.junit.xml. It is deleted before the converter runs, and the
  temporary file is renamed onto it only on success: exit 0 within the
  timeout, with OUT_PATH a non-empty regular file.
- On any other outcome (non-zero exit, timeout, missing or empty output, a
  command that cannot start) the temporary file is deleted,
  `warning: report <format>: <reason>` is printed to stderr and the next
  converter runs. The runner records the warning for the job; while
  dogfooding, in the PR description.
- A format with no converter, or an invalid declaration, is a config error:
  --check reports it and exits 2, and a plain run exits 2 before running any
  converter. A config error or a converter failure never changes the gate
  verdict, which is taken from the CTRF report before any converter runs.

Runner sequence (the dogfooding session follows it):

1. report-gates.py --check; stop the job on exit 2.
2. Write inputs/gates.json from gates.run.
3. Dispatch the gates stage and save gates.md.
4. run-gates.py --verdict for the verdict.
5. report-gates.py, noting any warnings.

Exits 0 when every requested format was written or none was requested, 1 when
any converter failed, 2 on a config error, a missing yq or CTRF report, or bad
arguments. Standard library only, apart from calling yq.
"""

import argparse
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
DEFAULT_CONFIG = ROOT / ".mintswarm" / "config.yaml"
OUTPUTS = Path(".mintswarm/job/outputs")
REPORT = OUTPUTS / "gates.ctrf.json"
FORMAT_NAME = re.compile(r"[a-z0-9][a-z0-9-]*")
EXTENSION = re.compile(r"[a-z0-9]+")
CONVERTER_KEYS = {"command", "extension", "timeout"}
REPORT_KEYS = {"formats", "converters"}
DEFAULT_TIMEOUT_S = 300
BUILT_INS = {
    "junit": {
        "command": ["python3", str(HERE / "reporters" / "ctrf-to-junit.py")],
        "extension": "xml",
        "timeout": DEFAULT_TIMEOUT_S,
    },
}


class ConfigError(Exception):
    pass


def load_config(path: Path) -> dict:
    try:
        proc = subprocess.run(["yq", "-o=json", ".", str(path)], capture_output=True, text=True)
    except OSError as exc:
        raise ConfigError(f"cannot run yq: {exc}") from exc
    if proc.returncode != 0:
        raise ConfigError(f"yq failed on {path}: {proc.stderr.strip()}")
    try:
        config = json.loads(proc.stdout or "null")
    except ValueError as exc:
        raise ConfigError(f"yq gave no JSON for {path}: {exc}") from exc
    if not isinstance(config, dict):
        raise ConfigError(f"{path} is not a mapping")
    return config


def converter(name: str, value) -> dict:
    where = f"gates.report.converters.{name}"
    if not isinstance(name, str) or not FORMAT_NAME.fullmatch(name):
        raise ConfigError(f"{where}: a format name must match [a-z0-9][a-z0-9-]*")
    if name == "ctrf":
        raise ConfigError(f"{where}: ctrf is reserved and cannot be a converter")
    if not isinstance(value, dict):
        raise ConfigError(f"{where} must be a mapping")
    unknown = sorted(set(value) - CONVERTER_KEYS)
    if unknown:
        raise ConfigError(f"{where}: unknown keys: {', '.join(unknown)}")
    command = value.get("command")
    if not isinstance(command, list) or not command or not all(isinstance(a, str) for a in command):
        raise ConfigError(f"{where}.command must be a non-empty list of strings")
    extension = value.get("extension")
    if not isinstance(extension, str) or not EXTENSION.fullmatch(extension):
        raise ConfigError(f"{where}.extension must match [a-z0-9]+")
    timeout = value.get("timeout", DEFAULT_TIMEOUT_S)
    if not isinstance(timeout, int) or isinstance(timeout, bool) or timeout <= 0:
        raise ConfigError(f"{where}.timeout must be a positive integer (seconds)")
    return {"command": command, "extension": extension, "timeout": timeout}


def resolve(config: dict) -> list:
    """Return [(format, converter)] for each requested format other than ctrf."""
    gates = config.get("gates")
    if gates is None:
        gates = {}
    if not isinstance(gates, dict):
        raise ConfigError("gates must be a mapping")
    report = gates.get("report")
    if report is None:
        return []
    if not isinstance(report, dict):
        raise ConfigError("gates.report must be a mapping")
    unknown = sorted(set(report) - REPORT_KEYS)
    if unknown:
        raise ConfigError(f"gates.report: unknown keys: {', '.join(unknown)}")
    formats = report.get("formats", ["ctrf"])
    if not isinstance(formats, list) or not all(isinstance(f, str) for f in formats):
        raise ConfigError("gates.report.formats must be a list of format names")
    declared = report.get("converters") or {}
    if not isinstance(declared, dict):
        raise ConfigError("gates.report.converters must be a mapping")
    converters = dict(BUILT_INS)
    converters.update({name: converter(name, value) for name, value in declared.items()})

    resolved = []
    seen = set()
    for name in formats:
        if not FORMAT_NAME.fullmatch(name):
            raise ConfigError(f"gates.report.formats: {name!r} must match [a-z0-9][a-z0-9-]*")
        if name in seen:
            raise ConfigError(f"gates.report.formats: {name} is listed twice")
        seen.add(name)
        if name == "ctrf":
            continue
        if name not in converters:
            raise ConfigError(f"gates.report.formats: no converter for {name}")
        resolved.append((name, converters[name]))
    return resolved


def remove(path: Path) -> None:
    if path.is_dir() and not path.is_symlink():
        shutil.rmtree(path, ignore_errors=True)
    else:
        path.unlink(missing_ok=True)


def convert(name: str, spec: dict) -> bool:
    final = OUTPUTS / f"gates.{name}.{spec['extension']}"
    temporary = final.with_name(final.name + ".tmp")
    remove(final)
    remove(temporary)
    reason = None
    with tempfile.TemporaryDirectory(prefix="report-gates-") as scratch:
        ctrf_in = Path(scratch) / "gates.ctrf.json"
        shutil.copyfile(REPORT, ctrf_in)
        os.chmod(ctrf_in, 0o444)
        argv = spec["command"] + [str(ctrf_in), str(temporary.resolve())]
        sys.stderr.flush()
        try:
            proc = subprocess.run(
                argv,
                stdin=subprocess.DEVNULL,
                stdout=sys.stderr.fileno(),
                stderr=sys.stderr.fileno(),
                timeout=spec["timeout"],
            )
        except subprocess.TimeoutExpired:
            reason = f"timed out after {spec['timeout']} s"
        except OSError as exc:
            reason = f"could not start {spec['command'][0]}: {exc.strerror or exc}"
        else:
            if proc.returncode < 0:
                reason = f"killed by signal {-proc.returncode}"
            elif proc.returncode != 0:
                reason = f"exit code {proc.returncode}"
    if reason is None:
        try:
            info = os.lstat(temporary)
        except FileNotFoundError:
            reason = "no output written"
        else:
            if not stat.S_ISREG(info.st_mode):
                reason = "the output is not a regular file"
            elif info.st_size == 0:
                reason = "empty output"
    if reason is None:
        try:
            os.replace(temporary, final)
        except OSError as exc:
            reason = f"cannot rename the output: {exc}"
    if reason is not None:
        remove(temporary)
        print(f"warning: report {name}: {reason}", file=sys.stderr)
        return False
    print(f"wrote {final}")
    return True


def main(argv: list) -> int:
    parser = argparse.ArgumentParser(prog="report-gates.py", description="Write gate results in extra report formats.")
    parser.add_argument("--check", action="store_true", help="validate the report settings and run nothing")
    parser.add_argument("--config", type=Path, default=DEFAULT_CONFIG, help="repository config (default: %(default)s)")
    args = parser.parse_args(argv)
    try:
        formats = resolve(load_config(args.config))
    except ConfigError as exc:
        print(f"report-gates: {exc}", file=sys.stderr)
        return 2
    if args.check:
        print("report formats: " + ", ".join(["ctrf"] + [name for name, _ in formats]))
        return 0
    if not REPORT.is_file():
        print(f"report-gates: no CTRF report at {REPORT}", file=sys.stderr)
        return 2
    ok = True
    for name, spec in formats:
        ok = convert(name, spec) and ok
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
