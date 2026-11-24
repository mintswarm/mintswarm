# Landing while dogfooding

This directory stands in for the control plane's landing: the deterministic landing floor (`floor.py`), the landing stage's judgement (`judgement.py`, which reads the document `.mintswarm/stages/landing.md` asks for) and the decision that combines them (`decision.py`). The session runs them on a PR once review passes. Each script's docstring is the authority on its inputs, rules and outputs; `decision.py`'s gives the full landing sequence. The `*-selftest.py` scripts beside them test them offline.

## The landing sequence

**Collect the facts.** After the round is kept with `keep-round.py`, `collect-facts.py fetch` reads the PR from GitHub and `collect-facts.py assemble` writes `.mintswarm/job/inputs/landing/facts.json`: the checks on the head, the kept gate and review rounds, the steers, the pending re-requested reviews and the plan's approval, with its decision record when one is given.

**Run the floor.** `floor.py` checks the change against its approved plan, with git alone, from a base-ref copy of `.mintswarm/dogfood/`. It writes `.mintswarm/job/outputs/landing-floor.json`; every hit sends the change to a human.

**Run the landing stage.** The landing stage rates the change's auto-approval confidence in a named band, against the approved plan, from the issue, the plan, the diff, the facts and the kept rounds, never from the implementer's reasoning or the floor's verdict. Its confidence rests on plan compliance, passed reviews and care.

**Decide.** `decision.py` combines the floor's verdict with the stage's judgement under `pipeline.landing.mode` at the base, writes `.mintswarm/job/outputs/landing.json` and prints the landing record that the session posts on the PR.

## When the require-human and protected rules hit

`require-human-paths`, `require-human-components` and `protected` ask a human only about what the approved plan did not cover. A changed path is covered when all of these hold:

- **The plan was approved by a human.** The facts' `approval.approver` is a login by `collect-facts.py`'s pattern (`[A-Za-z0-9-]+`, optionally followed by `[bot]`) that does not end in `[bot]`, and `approval.decision` holds the plan approval's decision record, whose `record.tool` is `mintswarm-approval-decision`, whose `record.approval` is `plan`, whose `record.decision` is `human` and whose `record.subject.plan.sha256` is the sha256 of the plan given to the floor. The record is what ties the approval to this plan, so `collect-facts.py assemble` must be given `--decision`. A missing or malformed approval, an approver that is no such login, a missing or malformed decision record, a record of another tool or whose approval is not `plan`, a decision `auto` or any other value, and a record for another plan all count as no human approval: the rules hit, and the input is still usable.
- **The footprint declares the exact path,** in `files` or as a new dependency's `manifest`, compared exactly, as `undeclared-files` compares it. An unusable footprint declares nothing.
- **The path is not a single entry standing in for a protected directory.** A symlink or gitlink at `.github/workflows`, or at a directory above a protected target, always hits `protected`: a footprint lists files, never directories.
- **A gate path has its own `gate_changes` entry.** When the path is `.mintswarm/config.yaml` or a word of a `gates.run` command at the base, some `gate_changes` entry, stripped and compared case-insensitively, starts with the path followed by `:`. An entry that names the path in any other form covers nothing.

A covered path gives no reason in any of the three rules. The issue's own component, when it is in `require_human.components`, stops hitting only when the plan was approved by a human, the footprint is usable and every changed path in that component is covered. Every other match keeps its own per-path reason, and the rule adds why the path is not covered: a declared gate path without its entry, or a plan not approved by a human. `undeclared-files`, `undeclared-new-files`, `size` and the facts rules do not change with the approval.

The approver comes from the session, which passes it to `collect-facts.py assemble`, and is trusted by convention like the rest of the facts. Under a future `judged` mode, the protected targets would rest on that convention too.

## PR approval

PR approval stays with a human in every mode. Nothing merges on the landing decision yet: its record's `acts` is always false, and the maintainer approves the PR and the bot merges it as before.
