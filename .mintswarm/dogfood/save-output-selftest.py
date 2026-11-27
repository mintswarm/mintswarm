#!/usr/bin/env python3
"""Offline self-test of save-output.py.

    save-output-selftest.py [unittest options]

Builds hand-made Claude Code transcripts (JSONL) and plain-text replies in a
fresh temporary directory and runs save-output.py on them as a subprocess,
from inside that directory, so exit codes, stdout and stderr are seen as the
dispatching session sees them. Needs no network and writes nothing outside
the temporary directory.

Checks:

- transcripts: a well-formed reply whose message spans several records that
  share a message.id; the document in an earlier message than the final
  hand-back; a missing document block; a result block in a ```msw-result
  fence; a ```yaml result block without `msw-result:`; a document block that
  is never closed; an api-request-blob copy holding a valid block, which is
  ignored; a broken final reply after a good earlier one, whose fault is
  reported; a final message that is an unfenced document after a good
  earlier draft, which is rejected rather than replaced by the draft; a CRLF reply, saved unchanged. Successes name the assistant
  message used (K of M) and save exactly the document; failures exit 1,
  name the message checked, and write no file;
- --text mode, which saves the same document and names no message;
- every fault save-output.py reports, each naming its rule, and every rule
  text it prints appearing verbatim in .mintswarm/stages.md;
- every stage definition in .mintswarm/stages/ (README.md excepted) ending
  its Output section with its reply skeleton, a five-backtick `text` fence
  (two for implement: handoff, then address) that save-output.py --text
  accepts, whose title is the stage template's title line, and whose
  `artefact` matches that title, with `outputs` equal to
  [.mintswarm/job/outputs/<artefact>.md];
- every save, which also writes <name>.result.json with the expected
  record, and every rejection, which leaves no *.result.json;
- result blocks with questions: every kind with every shape it allows;
  each of the 16 violations of StageOutcome::check with its exact
  message, and several at once in the Rust order; the status rule both
  ways; each structural violation (unknown, duplicate and missing keys,
  an unknown nested key, a bad enum, a wrong type, an old string
  question, a malformed or unclosed array, a closing ] at three spaces, a
  tab-indented or blank line inside the array, text after ], single
  quotes, an escape that is not JSON, `reason: null`), each with its
  line; the comment and null forms that are allowed; `commit` and
  `pr_title`; the record's name without a .md suffix; and the Questions
  example in .mintswarm/stages.md, one question carrying every field;
- every case under crates/domain/tests/fixtures/result-blocks/: its
  block.yaml, wrapped in a reply, gives exactly its result.json, byte for
  byte.

MSW_IMPL picks the implementation under test. Unset or empty, the checks run
save-output.py; `rust`, they run `$MSW_RUST_BIN tool save-output`, the
mintswarm-runner binary, whose path is resolved against the starting
directory. Any other value, or `rust` without an executable MSW_RUST_BIN,
exits 2 before any check runs. Under Rust the check that imports
save-output.py as a module is skipped; the unit test of that name in
crates/runner/src/tool/save_output.rs covers it.

Prints one line per check and exits 0 only if every check passed.
Standard library only.
"""

import importlib.util
import json
import os
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
SCRIPT = HERE / "save-output.py"
STAGES_MD = ROOT / ".mintswarm" / "stages.md"
STAGE_DIR = ROOT / ".mintswarm" / "stages"
BREAKS = re.compile(r"^breaks (rule [1-6]|Result block) of \.mintswarm/stages\.md: ", re.MULTILINE)

DOCUMENT = "# Review round 1: An issue\n\nPass, no findings.\n\n## Findings\n\n```yaml\nfindings: []\n```\n"
RESULT = (
    "```yaml\n"
    "msw-result:\n"
    "  status: complete\n"
    "  artefact: review\n"
    "  outputs: [.mintswarm/job/outputs/review.md]\n"
    "  questions: []\n"
    '  reason: ""\n'
    "```\n"
)
REPLY = "Review done.\n\n````markdown\n" + DOCUMENT + "````\n\n" + RESULT
SUMMARY = "The review is complete: no findings, verdict pass."
RECORD = {
    "status": "complete",
    "artefact": "review",
    "outputs": [".mintswarm/job/outputs/review.md"],
    "questions": [],
    "reason": "",
    "commit": None,
    "pr_title": None,
}
FIXTURES = ROOT / "crates" / "domain" / "tests" / "fixtures" / "result-blocks"


def usage_error(message):
    print(f"save-output-selftest: {message}", file=sys.stderr)
    sys.exit(2)


def resolve_command():
    """The argv prefix that runs the save-output tool, chosen by MSW_IMPL and MSW_RUST_BIN."""
    impl = os.environ.get("MSW_IMPL", "")
    if impl == "":
        return [sys.executable, "-I", str(SCRIPT)]
    if impl != "rust":
        usage_error(f"MSW_IMPL must be unset, empty or rust, not {impl!r}")
    binary = os.environ.get("MSW_RUST_BIN", "")
    if binary == "":
        usage_error("MSW_IMPL=rust needs MSW_RUST_BIN, the path of the mintswarm-runner binary")
    path = Path(binary).resolve()
    if not path.is_file() or not os.access(path, os.X_OK):
        usage_error(f"MSW_RUST_BIN is not an executable file: {path}")
    return [str(path), "tool", "save-output"]


RUST = os.environ.get("MSW_IMPL", "") == "rust"
COMMAND = resolve_command()


def tool_command():
    """The argv prefix that runs the save-output tool under test."""
    return list(COMMAND)


# The document title each artefact's skeleton carries.
TITLES = {
    "analysis": "# Analysis: <issue title>",
    "assessment": "# Assessment: <issue title>",
    "decomposition": "# Decomposition: <issue title>",
    "design": "# Design: <issue title>",
    "gates": "# Gates: <issue title>",
    "handoff": "# Handoff: <issue title>",
    "landing": "# Landing: <issue title>",
    "address": "# Address round <n>: <issue title>",
    "plan": "# Plan: <issue title>",
    "review": "# Review round <n>: <issue title>",
}
# The skeletons each stage's Output section ends with, in order.
SKELETONS = {
    "analyse": ["analysis"],
    "assess": ["assessment"],
    "assess-decomposition": ["assessment"],
    "decompose": ["decomposition"],
    "design": ["design"],
    "gates": ["gates"],
    "implement": ["handoff", "address"],
    "landing": ["landing"],
    "plan": ["plan"],
    "review": ["review"],
}


def record(mid, *content, kind="assistant"):
    message = {"role": "assistant", "content": list(content)}
    if mid is not None:
        message = {"id": mid, "type": "message", **message}
    return {"type": kind, "uuid": f"u-{mid}-{len(content)}", "message": message}


def user(text):
    return {"type": "user", "message": {"role": "user", "content": [{"type": "tool_result", "content": text}]}}


def thinking(text="Considering the inputs."):
    return {"type": "thinking", "thinking": text, "signature": "sig"}


def text(value):
    return {"type": "text", "text": value}


def tool(name, **inputs):
    return {"type": "tool_use", "id": f"toolu_{name}", "name": name, "input": inputs}


def handback(message):
    return tool("SubagentHandback", message=message)


def broken(reply, old, new):
    assert old in reply
    return reply.replace(old, new, 1)


class Harness(unittest.TestCase):
    """Runs save-output.py in a fresh temporary directory and checks what it saved or rejected."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="msw-save-output.")
        self.dir = Path(self.tmp.name)
        self.out = self.dir / "outputs" / "review.md"
        self.stages_md = STAGES_MD.read_text(encoding="utf-8")

    def tearDown(self):
        self.tmp.cleanup()

    def run_script(self, *args):
        return subprocess.run(
            [*tool_command(), *args],
            cwd=self.dir,
            capture_output=True,
            text=True,
            encoding="utf-8",
            check=False,
        )

    def transcript(self, *records):
        path = self.dir / "agent.jsonl"
        with path.open("w", encoding="utf-8", newline="") as f:
            for rec in records:
                f.write(json.dumps(rec) + "\n")
        return path

    def reply_file(self, reply, name="reply.txt"):
        path = self.dir / name
        with path.open("w", encoding="utf-8", newline="") as f:
            f.write(reply)
        return path

    def saved(self):
        with self.out.open(encoding="utf-8", newline="") as f:
            return f.read()

    def assert_saved(self, proc, document=DOCUMENT, message=None, record=None):
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertEqual(self.saved(), document)
        expected = json.dumps(RECORD if record is None else record, indent=2, ensure_ascii=False) + "\n"
        self.assertEqual(self.out.with_name("review.result.json").read_text(encoding="utf-8"), expected)
        self.assertIn("msw-result:", proc.stdout)
        self.assertIn(f"saved {len(document)} characters to {self.out}", proc.stdout)
        if message is None:
            self.assertNotIn("document from", proc.stdout)
        else:
            self.assertIn(f"document from {message}\n", proc.stdout)

    def assert_rejected(self, proc, rule, *fragments, message=None):
        self.assertEqual(proc.returncode, 1, proc.stdout + proc.stderr)
        self.assertFalse(self.out.exists(), "a file was written for a rejected reply")
        self.assertEqual(list(self.dir.rglob("*.result.json")), [], "a record was written for a rejected reply")
        self.assertEqual(proc.stdout, "")
        found = BREAKS.search(proc.stderr)
        if found is None:
            self.fail(f"no 'breaks <rule>' line in stderr: {proc.stderr}")
        self.assertEqual(found.group(1), rule, proc.stderr)
        quoted = proc.stderr[found.end() :].rstrip("\n")
        self.assertIn(quoted, self.stages_md, "the quoted rule is not verbatim in .mintswarm/stages.md")
        self.assertTrue(quoted.strip())
        for fragment in fragments:
            self.assertIn(fragment, proc.stderr)
        if message is not None:
            self.assertIn(f"save-output: {message}: ", proc.stderr)


class SaveOutput(Harness):
    # The four cases the issue requires.

    def test_well_formed_reply(self):
        path = self.transcript(
            record("msg_a", thinking()),
            record("msg_a", text("Reading the inputs.")),
            record("msg_a", tool("Read", file_path="issue.md")),
            user("the issue"),
            record("msg_b", thinking()),
            record("msg_b", handback(REPLY)),
        )
        self.assert_saved(self.run_script(str(path), str(self.out)), message="assistant message 2 of 2 (msg_b)")

    def test_block_in_earlier_message(self):
        path = self.transcript(
            record("msg_1", thinking()),
            record("msg_1", text(REPLY)),
            record("msg_1", tool("Bash", command="git status")),
            user("clean"),
            record("msg_2", handback(SUMMARY)),
        )
        self.assert_saved(self.run_script(str(path), str(self.out)), message="assistant message 1 of 2 (msg_1)")

    def test_missing_block(self):
        plain = "# Review round 1: An issue\n\nPass, no findings.\n\n## Verdict\npass\n"
        path = self.transcript(record("msg_1", thinking()), record("msg_1", handback(plain)))
        proc = self.run_script(str(path), str(self.out))
        self.assert_rejected(proc, "rule 1", "no document block", message="assistant message 1 of 1 (msg_1)")

    def test_result_in_non_yaml_fence(self):
        reply = broken(REPLY, "```yaml\nmsw-result:\n", "```msw-result\n")
        path = self.transcript(record("msg_1", handback(reply)))
        proc = self.run_script(str(path), str(self.out))
        self.assert_rejected(proc, "Result block", 'tagged "msw-result"', message="assistant message 1 of 1 (msg_1)")

    # Further cases.

    def test_result_without_msw_result_key(self):
        reply = broken(REPLY, "```yaml\nmsw-result:\n  status: complete\n", "```yaml\nstatus: complete\n")
        path = self.transcript(record("msg_1", handback(reply)))
        proc = self.run_script(str(path), str(self.out))
        self.assert_rejected(proc, "Result block", '"status: complete"', "'msw-result:'")

    def test_document_never_closed(self):
        reply = "````markdown\n" + DOCUMENT + "\n" + RESULT
        path = self.transcript(record("msg_1", handback(reply)))
        proc = self.run_script(str(path), str(self.out))
        self.assert_rejected(proc, "rule 2", "never closed")

    def test_api_request_blob_ignored(self):
        path = self.transcript(
            record("msg_1", handback(SUMMARY)),
            record(None, text(REPLY), kind="api-request-blob"),
            record("msg_blob", text(REPLY), kind="api-request-blob"),
        )
        proc = self.run_script(str(path), str(self.out))
        self.assert_rejected(proc, "rule 1", "no document block", message="assistant message 1 of 1 (msg_1)")

    def test_broken_final_reply_reported(self):
        path = self.transcript(
            record("msg_1", text(REPLY)),
            user("please fix"),
            record("msg_2", handback(REPLY.split("```yaml\nmsw")[0])),
        )
        proc = self.run_script(str(path), str(self.out))
        self.assert_rejected(proc, "rule 6", "no msw-result block", message="assistant message 2 of 2 (msg_2)")

    def test_unfenced_final_document_not_replaced(self):
        draft = REPLY.replace("Pass, no findings.", "DRAFT")
        path = self.transcript(
            record("msg_1", text(draft)),
            user("please revise"),
            record("msg_2", handback("# Review round 1: An issue\n\nFINAL revised, changes_required\n")),
        )
        proc = self.run_script(str(path), str(self.out))
        self.assert_rejected(
            proc,
            "rule 1",
            "no document block",
            "'# ' title",
            "no earlier block is used",
            message="assistant message 2 of 2 (msg_2)",
        )

    def test_record_without_id_is_own_message(self):
        path = self.transcript(record(None, text(REPLY)), record(None, text(SUMMARY)))
        self.assert_saved(self.run_script(str(path), str(self.out)), message="assistant message 1 of 2 (no id)")

    def test_text_mode(self):
        path = self.reply_file(REPLY)
        self.assert_saved(self.run_script("--text", str(path), str(self.out)))

    def test_crlf_reply_saved_unchanged(self):
        reply = REPLY.replace("\n", "\r\n")
        path = self.transcript(record("msg_1", handback(reply)))
        document = DOCUMENT.replace("\n", "\r\n")
        self.assert_saved(self.run_script(str(path), str(self.out)), document, "assistant message 1 of 1 (msg_1)")

    def test_every_fault_names_its_rule(self):
        cases = [
            ("rule 1", SUMMARY + "\n", "no document block"),
            ("rule 2", "````markdown\n" + DOCUMENT, "never closed"),
            ("rule 3", broken(REPLY, "```yaml\nfindings", "````yaml\nfindings"), "a run of 4 backticks"),
            ("rule 4", broken(REPLY, "# Review round 1", "Review round 1"), "'# ' title"),
            ("rule 5", "````markdown\n" + DOCUMENT + "````\n\nAnd again:\n\n" + REPLY, "2 document blocks"),
            ("rule 6", REPLY.split("```yaml\nmsw")[0], "no msw-result block"),
            (
                "rule 6",
                broken(REPLY, "````\n\n```yaml\nmsw", "````\n\nStatus below.\n\n```yaml\nmsw"),
                '"Status below."',
            ),
            ("Result block", broken(REPLY, "```yaml\nmsw-result:", "```\nmsw-result:"), "untagged"),
            ("Result block", broken(REPLY, "```yaml\nmsw-result:", "```msw-result"), 'tagged "msw-result"'),
            ("Result block", broken(REPLY, "msw-result:\n", "result:\n"), '"result:"'),
            ("Result block", REPLY.rsplit("```", 1)[0], "result block is never closed"),
        ]
        for rule, reply, fragment in cases:
            with self.subTest(rule=rule, fragment=fragment):
                path = self.reply_file(reply)
                self.assert_rejected(self.run_script("--text", str(path), str(self.out)), rule, fragment)


def fenced_blocks(lines):
    """Yield (tag, backticks, body) for each top-level fenced block in lines."""
    i = 0
    while i < len(lines):
        m = re.fullmatch(r"(`{3,})(\S*)[ \t]*", lines[i])
        if not m:
            i += 1
            continue
        fence, tag = m.group(1), m.group(2)
        closer = re.compile(re.escape(fence) + r"[ \t]*")
        for j in range(i + 1, len(lines)):
            if closer.fullmatch(lines[j]):
                break
        else:
            raise AssertionError(f"block opened with {fence}{tag} is never closed")
        yield tag, len(fence), lines[i + 1 : j]
        i = j + 1


def output_section(lines):
    """The lines of the `## Output` section, ignoring headings inside fences."""
    start = lines.index("## Output") + 1
    fence = None
    for k in range(start, len(lines)):
        line = lines[k]
        m = re.fullmatch(r"(`{3,})(\S*)[ \t]*", line)
        if fence is None and m:
            fence = m.group(1)
        elif fence is not None and re.fullmatch(re.escape(fence) + r"[ \t]*", line):
            fence = None
        elif fence is None and line.startswith("## "):
            return lines[start:k]
    return lines[start:]


class Skeletons(unittest.TestCase):
    def test_stages_are_known(self):
        sources = sorted(p.stem for p in STAGE_DIR.glob("*.md") if p.name != "README.md")
        self.assertEqual(sources, sorted(SKELETONS))

    def test_skeletons(self):
        with tempfile.TemporaryDirectory(prefix="msw-save-output.") as tmp:
            for stage, artefacts in SKELETONS.items():
                with self.subTest(stage=stage):
                    self.check_stage(Path(tmp), stage, artefacts)

    def check_stage(self, tmp, stage, artefacts):
        source = (STAGE_DIR / f"{stage}.md").read_text(encoding="utf-8")
        blocks = list(fenced_blocks(output_section(source.split("\n"))))
        skeletons = [b for b in blocks if b[0] == "text"]
        self.assertEqual(len(skeletons), len(artefacts), f"{stage}: expected {len(artefacts)} skeletons")
        self.assertEqual(blocks[-len(artefacts) :], skeletons, f"{stage}: the skeletons must end the Output section")
        titles = {body[0] for tag, _, body in blocks if tag == "markdown" and body}
        for (_, backticks, body), artefact in zip(skeletons, artefacts):
            self.assertEqual(backticks, 5, f"{stage}: the skeleton fence must have five backticks")
            reply = tmp / f"{stage}-{artefact}.txt"
            out = tmp / f"{stage}-{artefact}.md"
            reply.write_text("\n".join(body) + "\n", encoding="utf-8")
            proc = subprocess.run(
                [*tool_command(), "--text", str(reply), str(out)],
                cwd=tmp,
                capture_output=True,
                text=True,
                encoding="utf-8",
                check=False,
            )
            self.assertEqual(proc.returncode, 0, f"{stage}: {proc.stderr}")
            title = out.read_text(encoding="utf-8").split("\n")[0]
            self.assertEqual(title, TITLES[artefact], f"{stage}: skeleton title")
            self.assertIn(title, titles, f"{stage}: the skeleton title is not the template's title line")
            found = re.search(r"^\s*artefact:\s*(\S+)\s*$", proc.stdout, re.MULTILINE)
            if found is None:
                self.fail(f"{stage}: no artefact line in the result block: {proc.stdout}")
            self.assertEqual(found.group(1), artefact, f"{stage}: artefact")
            outputs = re.search(r"^\s*outputs:\s*(\[.*\])\s*$", proc.stdout, re.MULTILINE)
            if outputs is None:
                self.fail(f"{stage}: no outputs line in the result block: {proc.stdout}")
            self.assertEqual(outputs.group(1), f"[.mintswarm/job/outputs/{artefact}.md]", f"{stage}: outputs")


def result_reply(*lines):
    """A reply holding DOCUMENT and a result block made of lines, `msw-result:` first."""
    return "````markdown\n" + DOCUMENT + "````\n\n```yaml\n" + "\n".join(lines) + "\n```\n"


# The reply line of `msw-result:` in result_reply(): the opener, DOCUMENT, the closer, a blank line, the fence.
BLOCK_LINE = DOCUMENT.count("\n") + 5
OUTPUTS_LINE = "  outputs: [.mintswarm/job/outputs/review.md]"


def array(*questions):
    """questions as a multi-line JSON array, one object per line, closed by `]` at four spaces."""
    if not questions:
        return ["  questions: []"]
    items = [json.dumps(q, ensure_ascii=False) for q in questions]
    return ["  questions: ["] + [f"    {item}," for item in items[:-1]] + [f"    {items[-1]}", "    ]"]


def block(*questions, status="complete", reason='""', extra=()):
    return [
        "msw-result:",
        f"  status: {status}",
        "  artefact: review",
        OUTPUTS_LINE,
        *array(*questions),
        f"  reason: {reason}",
        *extra,
    ]


def option(oid, **more):
    return {"id": oid, "label": f"Option {oid}", "consequence": f"Leads to {oid}.", **more}


def question(qid="q1", kind="must_decide", shape="one", **more):
    q = {"id": qid, "kind": kind, "question": f"Which {qid}?", "why": "It matters.", "shape": shape}
    if shape in ("one", "several"):
        q["options"] = [option("a"), option("b")]
    q.update((k, v) for k, v in more.items() if v is not ...)
    return q


def serde(q):
    """A question as the record holds it: every key, in serde order, defaults applied."""
    rec = q.get("recommendation")
    return {
        "id": q["id"],
        "kind": q["kind"],
        "question": q["question"],
        "why": q["why"],
        "shape": q["shape"],
        "options": [{**o, "trade_off": o.get("trade_off")} for o in q.get("options") or []],
        "recommendation": rec
        and {"choice": rec.get("choice") or [], "answer": rec.get("answer"), "reason": rec["reason"]},
        "references": [{**r, "lines": r.get("lines")} for r in q.get("references") or []],
    }


def record_of(*questions, status="complete"):
    return {**RECORD, "status": status, "questions": [serde(q) for q in questions]}


def listed(stderr):
    """The violations save-output.py listed, in order."""
    return [line[2:] for line in stderr.split("\n") if line.startswith("- ")]


class Questions(Harness):
    def run_block(self, lines):
        return self.run_script("--text", str(self.reply_file(result_reply(*lines))), str(self.out))

    def assert_accepted(self, lines, record):
        self.assert_saved(self.run_block(lines), record=record)

    def assert_violations(self, lines, *expected, exact=True):
        proc = self.run_block(lines)
        self.assert_rejected(proc, "Result block", "the result block has")
        found = listed(proc.stderr)
        if exact:
            self.assertEqual(found, list(expected), proc.stderr)
        else:
            self.assertEqual(len(found), len(expected), proc.stderr)
            for line, fragment in zip(found, expected):
                self.assertIn(fragment, line, proc.stderr)

    def test_every_kind_and_shape(self):
        yes_no = [option("yes"), option("no")]
        recommendations = {
            "one": {"choice": ["a"], "reason": "Simpler."},
            "several": {"choice": ["a"], "reason": "Enough."},
            "yes_no": {"choice": ["yes"], "reason": "Safer."},
            "open": {"choice": [], "answer": "result_block", "reason": "Matches the wording."},
        }
        for kind in ("must_decide", "recommended", "assumption"):
            for shape, options in (("one", ...), ("several", ...), ("yes_no", None), ("yes_no", yes_no), ("open", ...)):
                if kind == "assumption" and shape == "open":
                    continue
                with self.subTest(kind=kind, shape=shape, options=options):
                    rec = recommendations[shape] if kind != "must_decide" or shape == "open" else ...
                    q = question(kind=kind, shape=shape, options=options, recommendation=rec)
                    status = "complete" if kind == "assumption" else "needs_input"
                    self.assert_accepted(block(q, status=status), record_of(q, status=status))

    def test_each_rust_violation(self):
        two = [option("a"), option("b")]
        cases = [
            (question(kind="recommended"), "q1: a recommended or assumption question needs a recommendation"),
            (
                question(kind="assumption", shape="open", recommendation={"answer": "x", "reason": "r"}),
                "q1: an assumption cannot have shape open",
            ),
            (
                question(kind="assumption", shape="several", recommendation={"choice": ["a", "b"], "reason": "r"}),
                "q1: an assumption needs an option besides those it chooses",
            ),
            (question(question=" \t"), "q1: question is empty"),
            (question(question="\u3000\u2028\x85\xa0"), "q1: question is empty"),
            (question(why=""), "q1: why is empty"),
            (question(options=[option("a")]), "q1: shapes one and several need at least two options"),
            (
                question(shape="yes_no", options=[option("yes"), option("maybe")]),
                "q1: yes_no options must be exactly yes and no",
            ),
            (question(shape="open", options=two), "q1: an open question has no options"),
            (question(options=[option("a"), option("a")]), 'q1: option id "a" is not unique'),
            (question(recommendation={"choice": ["a", "b"], "reason": "r"}), "q1: 2 choices do not fit shape one"),
            (question(recommendation={"choice": ["z"], "reason": "r"}), 'q1: choice "z" is not an option id'),
            (
                question(shape="several", recommendation={"choice": ["a", "a"], "reason": "r"}),
                'q1: choice "a" is chosen more than once',
            ),
            (
                question(recommendation={"choice": ["a"], "answer": "x", "reason": "r"}),
                "q1: only an open question's recommendation has an answer",
            ),
        ]
        for q, message in cases:
            with self.subTest(message=message):
                status = "complete" if q["kind"] == "assumption" else "needs_input"
                self.assert_violations(block(q, status=status), message)
        with self.subTest(message="DuplicateId"):
            self.assert_violations(block(question(), question(), status="needs_input"), "q1: the id is not unique")
        assumed = question(kind="assumption", recommendation={"choice": ["a"], "reason": "r"})
        with self.subTest(message="NeedsInputWithoutBlocking"):
            self.assert_violations(
                block(assumed, status="needs_input"), "needs_input needs a must_decide or recommended question"
            )
        for status in ("complete", "failed"):
            with self.subTest(message="BlockingNotNeedsInput", status=status):
                self.assert_violations(
                    block(assumed, question("q2"), status=status), f"q2: a {status} outcome may hold assumptions only"
                )

    def test_several_violations_in_rust_order(self):
        self.assert_violations(
            block(
                question(kind="recommended", why=" "),
                question("q2", options=[option("a"), option("a")], recommendation={"choice": ["c"], "reason": "r"}),
                question("q1", kind="assumption", shape="yes_no", recommendation={"choice": ["yes"], "reason": "r"}),
                status="complete",
            ),
            "q1: a recommended or assumption question needs a recommendation",
            "q1: why is empty",
            'q2: option id "a" is not unique',
            'q2: choice "c" is not an option id',
            "q1: the id is not unique",
            "q1: a complete outcome may hold assumptions only",
            "q2: a complete outcome may hold assumptions only",
        )

    def test_structural_violations(self):
        good = question()
        lines = block(good, status="needs_input")
        at = {line: n for n, line in enumerate(lines, start=BLOCK_LINE)}
        three = [line if not line.startswith("    ]") else "   ]" for line in lines]
        cases = [
            ("unknown key", lines + ["  verdict: pass"], [f'line {len(lines) + BLOCK_LINE}: unknown key "verdict"']),
            (
                "unknown nested key",
                block(question(options=[option("a", cost=1), option("b")]), status="needs_input"),
                ['q1: options[0]: unknown key "cost"'],
            ),
            ("duplicate key", lines + ['  reason: ""'], [f'line {len(lines) + BLOCK_LINE}: duplicate key "reason"']),
            (
                "duplicate question key",
                [line.replace('"id": "q1", ', '"id": "q1", "id": "q9", ') for line in lines],
                ['q1: duplicate key "id"'],
            ),
            (
                "duplicate nested key",
                [line.replace('"id": "a", ', '"id": "a", "label": "x", ') for line in lines],
                ['q1: options[0]: duplicate key "label"'],
            ),
            (
                "unpaired surrogate in a question",
                [line.replace('"Which q1?"', '"Which q1\\ud800?"') for line in lines],
                ["q1: question holds an unpaired surrogate"],
            ),
            (
                "unpaired surrogate in an id",
                [line.replace('"id": "q1"', '"id": "q\\udc00"') for line in lines],
                ["questions[0]: id holds an unpaired surrogate"],
            ),
            (
                "unpaired surrogate in an optional string",
                block(question(options=[option("a", trade_off="T@"), option("b")]), status="needs_input"),
                ["q1: options[0].trade_off holds an unpaired surrogate"],
            ),
            (
                "unpaired surrogate in a scalar",
                block(good, status="needs_input", reason='"\\ud800"'),
                [f"line {at[lines[-1]]}: reason: holds an unpaired surrogate"],
            ),
            (
                "outputs item starting with a single quote",
                [line.replace("[.mintswarm/job/outputs/review.md]", "['x']") for line in lines],
                [
                    f"line {at[OUTPUTS_LINE]}: outputs item \"'x'\" is not a plain string: "
                    "starts with '\\''; quote it with double quotes"
                ],
            ),
            (
                "outputs item starting with an indicator",
                [line.replace("[.mintswarm/job/outputs/review.md]", "[&x]") for line in lines],
                [
                    f'line {at[OUTPUTS_LINE]}: outputs item "&x" is not a plain string: '
                    "starts with '&'; quote it with double quotes"
                ],
            ),
            ("missing key", [line for line in lines if "artefact" not in line], ['missing key "artefact"']),
            (
                "bad enum",
                block(question(kind="maybe"), status="needs_input"),
                ['q1: kind must be one of must_decide, recommended, assumption, not "maybe"'],
            ),
            ("wrong type", block(question(why=3), status="needs_input"), ["q1: why must be a string, not a number"]),
            (
                "string question",
                block("Which store keeps drafts?", status="needs_input"),
                ["questions[0] must be a question object, not a string"],
            ),
            (
                "bad status",
                block(good, status="done"),
                [f'line {BLOCK_LINE + 1}: status must be one of complete, needs_input, failed, not "done"'],
            ),
            (
                "single quotes",
                block(good, status="needs_input", reason="''"),
                [f"line {at[lines[-1]]}: reason: single-quoted values are not supported; use double quotes"],
            ),
            (
                "invalid escape",
                block(good, status="needs_input", reason='"\\x41"'),
                [f"line {at[lines[-1]]}: reason: a double-quoted value may use JSON escapes only"],
            ),
            (
                "null reason",
                block(good, status="needs_input", reason="null"),
                [f'line {at[lines[-1]]}: reason must be a string but empty or null; quote it, for example ""'],
            ),
            (
                "three-space closing bracket",
                three,
                [f"line {at['    ]']}: a continuation line of questions is indented by fewer than four spaces"],
            ),
            (
                "tab-indented line",
                [("\t" + line[4:]) if line.startswith("    {") else line for line in lines],
                [f"line {at['    ]'] - 1}: a continuation line of questions is indented with a tab; use four spaces"],
            ),
            (
                "blank line",
                lines[: at["    ]"] - BLOCK_LINE] + [""] + lines[at["    ]"] - BLOCK_LINE :],
                [f"line {at['    ]']}: a blank line inside the questions array"],
            ),
            (
                "text after ]",
                [line + " extra" if line == "    ]" else line for line in lines],
                [f"line {at['    ]']}: text after the questions array's closing ]: \"extra\""],
            ),
            (
                "malformed array",
                [line.replace('"q1", ', '"q1" ') for line in lines],
                [
                    f"line {at['  questions: [']}: the questions array is not valid JSON; "
                    "write it as a JSON array of question objects"
                ],
            ),
            (
                "unclosed array",
                [line for line in lines if line != "    ]"],
                [f"line {at['  questions: [']}: the questions array is not closed by ]"],
            ),
        ]
        for name, case, expected in cases:
            with self.subTest(case=name):
                self.assert_violations([line.replace("T@", "T\\ud800") for line in case], *expected)

    def test_trim_is_rusts(self):
        q = question(question="\x1c", why="\u200b")
        self.assert_accepted(block(q, status="needs_input"), record_of(q, status="needs_input"))

    @unittest.skipIf(
        RUST,
        "imports save-output.py; under Rust, write_both_cleans_up_on_any_exception in crates/runner/src/tool/save_output.rs covers it",
    )
    def test_write_both_cleans_up_on_any_exception(self):
        spec = importlib.util.spec_from_file_location("save_output", SCRIPT)
        module = importlib.util.module_from_spec(spec)
        dont_write, sys.dont_write_bytecode = sys.dont_write_bytecode, True
        try:
            spec.loader.exec_module(module)
        finally:
            sys.dont_write_bytecode = dont_write
        target = self.dir / "outputs"
        with self.assertRaises(UnicodeEncodeError):
            module.write_both([(target / "review.md", "ok\n"), (target / "review.result.json", "\ud800")])
        self.assertEqual(list(target.iterdir()), [], "write_both left files behind")

    def test_allowed_comment_and_null_forms(self):
        q = question(
            "q1",
            kind="assumption",
            why="See #339 §10.2 # not a comment",
            options=[option("a", trade_off=None), option("b", trade_off="Slower.")],
            recommendation={"choice": ["a"], "answer": None, "reason": "r"},
            references=[{"kind": "design", "target": "#339", "lines": None}, {"kind": "url", "target": "https://x"}],
        )
        empty = question("q2", kind="assumption", shape="yes_no", options=None, references=None)
        empty["recommendation"] = {"choice": ["no"], "reason": "r"}
        lines = block(q, empty)
        lines[0] = "msw-result:   # the stage's result"
        lines[1] = '  status: "complete"   # quoted'
        lines.insert(2, "  # a comment line between keys")
        lines.insert(3, "")
        lines[lines.index("    ]")] = "    ]   # end of questions"
        lines += ["  commit: null", "  pr_title: ~"]
        self.assert_accepted(lines, record_of(q, empty))

    def test_commit_and_pr_title(self):
        lines = block(extra=['  commit: "feat(agents): a \\"quoted\\" title"', "  pr_title: feat(agents) — plain"])
        self.assert_accepted(
            lines, {**RECORD, "commit": 'feat(agents): a "quoted" title', "pr_title": "feat(agents) — plain"}
        )

    def test_record_name_without_md(self):
        self.out = self.dir / "outputs" / "review"
        path = self.reply_file(REPLY)
        proc = self.run_script("--text", str(path), str(self.out))
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertEqual(json.loads((self.dir / "outputs" / "review.result.json").read_text()), RECORD)

    def test_stages_md_example(self):
        text = STAGES_MD.read_text(encoding="utf-8").split("\n### Questions\n", 1)[1]
        examples = [body for tag, _, body in fenced_blocks(text.split("\n")) if tag == "yaml"]
        self.assertEqual(len(examples), 1, "the Questions subsection has one yaml example")
        example = examples[0]
        proc = self.run_script("--text", str(self.reply_file(result_reply(*example))), str(self.out))
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertTrue(example[-2].startswith("    ]"), "the example's closing ] is indented by four spaces")
        first = next(i for i, line in enumerate(example) if line.startswith("  questions: ["))
        last = next(i for i, line in enumerate(example) if i > first and line.startswith("    ]"))
        array = "\n".join(example[first : last + 1]).removeprefix("  questions:")
        questions = json.loads(array)
        self.assertIsInstance(questions, list)
        self.assertEqual(len(questions), 1, "the example holds exactly one question")
        question = questions[0]
        self.assertEqual(
            set(question), {"id", "kind", "question", "why", "shape", "options", "recommendation"}, "question keys"
        )
        self.assertGreaterEqual(len(question["options"]), 2, "the example has at least two options")
        for option in question["options"]:
            self.assertEqual(set(option), {"id", "label", "consequence", "trade_off"}, "option keys")
            for value in option.values():
                self.assertIsInstance(value, str)
                self.assertTrue(value, "an option field is empty")
        recommendation = question["recommendation"]
        self.assertEqual(set(recommendation), {"choice", "reason"}, "recommendation keys")
        self.assertIsInstance(recommendation["choice"], list)
        self.assertTrue(recommendation["choice"], "the recommendation's choice is empty")
        self.assertIsInstance(recommendation["reason"], str)
        self.assertTrue(recommendation["reason"], "the recommendation's reason is empty")
        record = json.loads(self.out.with_name("review.result.json").read_text(encoding="utf-8"))
        self.assertEqual(record["status"], "needs_input")
        self.assertEqual(len(record["questions"]), 1)


class Fixtures(unittest.TestCase):
    """Each crates/domain result-block fixture's block.yaml gives exactly its result.json."""

    def test_fixtures(self):
        cases = sorted(p for p in FIXTURES.iterdir() if p.is_dir())
        self.assertGreaterEqual(len(cases), 3, "too few result-block fixtures")
        with tempfile.TemporaryDirectory(prefix="msw-save-output.") as tmp:
            for case in cases:
                with self.subTest(case=case.name):
                    self.assertTrue((case / "block.yaml").is_file(), f"{case.name}: no block.yaml")
                    self.assertTrue((case / "result.json").is_file(), f"{case.name}: no result.json")
                    lines = (case / "block.yaml").read_text(encoding="utf-8").rstrip("\n").split("\n")
                    reply = Path(tmp) / f"{case.name}.txt"
                    reply.write_text(result_reply(*lines), encoding="utf-8")
                    out = Path(tmp) / case.name / "document.md"
                    proc = subprocess.run(
                        [*tool_command(), "--text", str(reply), str(out)],
                        cwd=tmp,
                        capture_output=True,
                        text=True,
                        encoding="utf-8",
                        check=False,
                    )
                    self.assertEqual(proc.returncode, 0, f"{case.name}: {proc.stderr}")
                    self.assertEqual(
                        out.with_name("document.result.json").read_bytes(), (case / "result.json").read_bytes()
                    )


if __name__ == "__main__":
    unittest.main(verbosity=2)
