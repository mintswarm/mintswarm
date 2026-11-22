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
  reported; a CRLF reply, saved unchanged. Successes name the assistant
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
  [.mintswarm/job/outputs/<artefact>.md].

Prints one line per check and exits 0 only if every check passed.
Standard library only.
"""

import json
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

# The document title each artefact's skeleton carries.
TITLES = {
    "analysis": "# Analysis: <issue title>",
    "assessment": "# Assessment: <issue title>",
    "decomposition": "# Decomposition: <issue title>",
    "gates": "# Gates: <issue title>",
    "handoff": "# Handoff: <issue title>",
    "address": "# Address round <n>: <issue title>",
    "plan": "# Plan: <issue title>",
    "review": "# Review round <n>: <issue title>",
}
# The skeletons each stage's Output section ends with, in order.
SKELETONS = {
    "analyse": ["analysis"],
    "assess": ["assessment"],
    "decompose": ["decomposition"],
    "gates": ["gates"],
    "implement": ["handoff", "address"],
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


class SaveOutput(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="msw-save-output.")
        self.dir = Path(self.tmp.name)
        self.out = self.dir / "outputs" / "review.md"
        self.stages_md = STAGES_MD.read_text(encoding="utf-8")

    def tearDown(self):
        self.tmp.cleanup()

    def run_script(self, *args):
        return subprocess.run(
            [sys.executable, "-I", str(SCRIPT), *args],
            cwd=self.dir, capture_output=True, text=True, encoding="utf-8", check=False,
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

    def assert_saved(self, proc, document=DOCUMENT, message=None):
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertEqual(self.saved(), document)
        self.assertIn("msw-result:", proc.stdout)
        self.assertIn(f"saved {len(document)} characters to {self.out}", proc.stdout)
        if message is None:
            self.assertNotIn("document from", proc.stdout)
        else:
            self.assertIn(f"document from {message}\n", proc.stdout)

    def assert_rejected(self, proc, rule, *fragments, message=None):
        self.assertEqual(proc.returncode, 1, proc.stdout + proc.stderr)
        self.assertFalse(self.out.exists(), "a file was written for a rejected reply")
        self.assertEqual(proc.stdout, "")
        found = BREAKS.search(proc.stderr)
        self.assertIsNotNone(found, proc.stderr)
        self.assertEqual(found.group(1), rule, proc.stderr)
        quoted = proc.stderr[found.end():].rstrip("\n")
        self.assertIn(quoted, self.stages_md, "the quoted rule is not verbatim in .mintswarm/stages.md")
        self.assertTrue(quoted.strip())
        for fragment in fragments:
            self.assertIn(fragment, proc.stderr)
        if message is not None:
            self.assertIn(f"save-output: {message}: ", proc.stderr)

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
        self.assert_rejected(proc, "Result block", "tagged 'msw-result'", message="assistant message 1 of 1 (msg_1)")

    # Further cases.

    def test_result_without_msw_result_key(self):
        reply = broken(REPLY, "```yaml\nmsw-result:\n  status: complete\n", "```yaml\nstatus: complete\n")
        path = self.transcript(record("msg_1", handback(reply)))
        proc = self.run_script(str(path), str(self.out))
        self.assert_rejected(proc, "Result block", "'status: complete'", "'msw-result:'")

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
            ("rule 6", broken(REPLY, "````\n\n```yaml\nmsw", "````\n\nStatus below.\n\n```yaml\nmsw"), "'Status below.'"),
            ("Result block", broken(REPLY, "```yaml\nmsw-result:", "```\nmsw-result:"), "untagged"),
            ("Result block", broken(REPLY, "```yaml\nmsw-result:", "```msw-result"), "tagged 'msw-result'"),
            ("Result block", broken(REPLY, "msw-result:\n", "result:\n"), "'result:'"),
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
        yield tag, len(fence), lines[i + 1:j]
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
        self.assertEqual(blocks[-len(artefacts):], skeletons, f"{stage}: the skeletons must end the Output section")
        titles = {body[0] for tag, _, body in blocks if tag == "markdown" and body}
        for (_, backticks, body), artefact in zip(skeletons, artefacts):
            self.assertEqual(backticks, 5, f"{stage}: the skeleton fence must have five backticks")
            reply = tmp / f"{stage}-{artefact}.txt"
            out = tmp / f"{stage}-{artefact}.md"
            reply.write_text("\n".join(body) + "\n", encoding="utf-8")
            proc = subprocess.run(
                [sys.executable, "-I", str(SCRIPT), "--text", str(reply), str(out)],
                cwd=tmp, capture_output=True, text=True, encoding="utf-8", check=False,
            )
            self.assertEqual(proc.returncode, 0, f"{stage}: {proc.stderr}")
            title = out.read_text(encoding="utf-8").split("\n")[0]
            self.assertEqual(title, TITLES[artefact], f"{stage}: skeleton title")
            self.assertIn(title, titles, f"{stage}: the skeleton title is not the template's title line")
            found = re.search(r"^\s*artefact:\s*(\S+)\s*$", proc.stdout, re.MULTILINE)
            self.assertIsNotNone(found, proc.stdout)
            self.assertEqual(found.group(1), artefact, f"{stage}: artefact")
            outputs = re.search(r"^\s*outputs:\s*(\[.*\])\s*$", proc.stdout, re.MULTILINE)
            self.assertIsNotNone(outputs, proc.stdout)
            self.assertEqual(outputs.group(1), f"[.mintswarm/job/outputs/{artefact}.md]", f"{stage}: outputs")


if __name__ == "__main__":
    unittest.main(verbosity=2)
