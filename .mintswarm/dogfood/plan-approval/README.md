# Plan approval while dogfooding

This directory stands in for the control plane's plan approval: the deterministic floor (`floor.py`) and the decision that combines it with the assess stage's judgement (`decision.py`). Until the control plane exists, the session runs them by hand for every implementation plan, as described here. `size.py` reads the size an analysis or a plan states (see Sizes below). `selftest.py` tests the three scripts offline; run it with `python3 -I .mintswarm/dogfood/plan-approval/selftest.py`.

The mode is `pipeline.plan_approval.mode` in `.mintswarm/config.yaml` at the job's base commit, never the worktree's copy:

- `always`: every plan goes to a human.
- `shadow`: floor, assess and the decision run and are recorded, and every plan still goes to a human. The maintainer's own decisions become the comparison for the threshold review.
- `judged`: a plan is approved without a human only when the floor is clean, the assessment names no hard trigger, its confidence is at least `min_confidence`, and assess ran on the configured harness and model.
- `never`: every plan is approved without a human.

The threshold is `pipeline.plan_approval.min_confidence` at the same base commit: an integer from 90 to 100, 90 when absent. `floor.py` and `decision.py` reject any other value as unusable input. Like the rest of the config, it is changed only through a reviewed PR.

The size limits are `pipeline.plan_approval.limits` at the same base commit, with `files`, `lines`, `new_files` and `new_dependencies`. `pipeline.plan_approval.limits_by_component` may give a component some of those keys, which replace the flat value key by key for plans of that component; a component without an entry uses `limits`. `pipeline.plan_approval.non_production` lists the globs of paths whose lines are not production lines (tests, fixtures, lockfiles and generated files); when it is absent the floor uses its own generic defaults. The `lines` limit counts the footprint's `production_lines` when a path in `files` matches one of those globs, and `estimated_lines` otherwise, and the landing floor checks the diff's production lines against `production_lines` with the same globs. The `new_dependencies` limit counts distinct packages per ecosystem, leaving out those already in the base's root `Cargo.lock`, its root `[workspace.dependencies]` or the nearest base `pnpm-lock.yaml`.

Assess rates each plan by an auto-approval confidence from 0 to 100 in one of five named bands, defined in `.mintswarm/stages/assess.md`: `blocked` (Blocked, 0, a hard trigger), `uncertain` (Uncertain, 1 to 49), `needs-review` (Needs review, 50 to 79), `likely-safe` (Likely safe, 80 to 94) and `safe` (Safe, 95 to 100). It returns them in one JSON envelope with the hard triggers, the decisive reason and its doubts; an envelope that breaks the stage's rules makes the judgement unusable, so a human decides.

Both layers run in every mode, so a disagreement between them, or with the maintainer, is always visible.

## Steps

After the plan stage returns a plan, in this order:

1. **Post the TL;DR.** Once the plan artifact is published, post one comment on the implementation issue through `bot-gh.sh` (`-F body=@<file>`). This is done in every mode and for every published plan, revisions included. The comment holds a heading line naming the plan's TL;DR, the section's bullets unchanged, and the full `html_url` GitHub returned for the plan artifact. The section is the lines after the plan's first `## TL;DR` line up to the next line starting with `## `. It is usable only when all of these hold (rule 11 of `.mintswarm/stages/plan.md`):
   - the `## TL;DR` line comes right after the title and summary line, before any other `## ` heading;
   - ignoring blank lines, it has three to six lines, and each starts with `- `; anything else, such as prose, an indented continuation line, a nested bullet, a heading or a fence line, makes it unusable, and nothing is dropped or repaired;
   - every line has an even number of backticks, which pair up into inline code spans;
   - outside those code spans, no line holds `@`, `<`, `>`, `](`, a bare GitHub reference (`#` followed by a digit, `owner/repo#N`, `GH-` followed by a digit) or a run of 40 hexadecimal characters; text inside a code span is exempt from these checks;
   - no line holds a URL (`://` or `www.`), inside a code span or not.

   Otherwise the comment says instead that the plan has no usable TL;DR and links only the plan artifact; the plan's text is then not posted.
2. **Lay out the assess job.** Put the plan at the assess job's `.mintswarm/job/inputs/artefacts/plan.md`, beside `.mintswarm/job/inputs/issue.md` and `.mintswarm/job/inputs/artefacts/analysis.md`. Nothing else goes under `inputs/`: no parent artefacts, no floor verdict, no planner transcript.
3. **Run the floor** in that job's worktree, on that file and the job's issue document and analysis, with `--base` at the job's base commit:

   ```
   python3 -I .mintswarm/dogfood/plan-approval/floor.py --plan .mintswarm/job/inputs/artefacts/plan.md \
     --component <component> --base <base commit> \
     --issue-document .mintswarm/job/inputs/issue.md --analysis .mintswarm/job/inputs/artefacts/analysis.md
   ```

   It writes `.mintswarm/job/outputs/floor.json`. Besides the footprint, it checks for blocking questions in the analysis and the plan, for a Verification table that does not quote the issue's acceptance criteria, and for the first job of its kind (no earlier commit scoped `(<component>)` on the base). A blocking question stops being a hit only when the analysis is rerun with the answer applied to the issue.
4. **Run assess** (`msw-assess`) in a fresh subagent that receives only `issue.md`, `analysis.md` and `plan.md`: no summary of the plan stage, no floor verdict, no other context. Run it on the model `agents.stages.assess` names in the base config, which differs from the plan stage's model.
5. **Save the assessment** from the subagent's reply with `.mintswarm/dogfood/save-output.py` to `.mintswarm/job/outputs/assessment.md`.
6. **Publish the assessment as an issue artifact named exactly `Plan assessment`** on the implementation issue, through `bot-gh.sh api` (see "Session procedure" in `.mintswarm/dogfood/github/README.md`). This is done on every plan, whatever `pipeline.plan_approval.mode` is, for traceability.
7. **Run the decision** with the same plan file, issue document and analysis, the assessment, the assessment's artifact, the same base, and the harness and model assess actually ran on:

   ```
   python3 -I .mintswarm/dogfood/plan-approval/decision.py --issue <number> --component <component> \
     --plan .mintswarm/job/inputs/artefacts/plan.md \
     --issue-document .mintswarm/job/inputs/issue.md --analysis .mintswarm/job/inputs/artefacts/analysis.md \
     --assessment .mintswarm/job/outputs/assessment.md \
     --assessment-artifact <artifact URL> --base <base commit> --harness <harness> --model <model>
   ```

   It writes `.mintswarm/job/outputs/decision.json` (version 2: the threshold, and the judgement's confidence, band, hard triggers, doubts and reason) and exits 0 for `auto` and 1 for `human`. For `auto` it prints the auto-approval comment to stdout, and in `shadow` mode the shadow-record comment; otherwise stdout is empty. Keep stdout in a file for the comment.
8. **Publish `decision.json` as an issue artifact** on the same issue, in every mode, for traceability.
9. **Record the decision in the session's plan artifact**: the mode, the decision and `judged_decision`, the floor result, the band and confidence against `min_confidence`, any hard triggers, the reason, and links to the assessment and decision artifacts.

## Sizes

The analyse and plan stages state a size in a `## Size` section (`.mintswarm/stages/analyse.md` and rule 12 of `.mintswarm/stages/plan.md`). `size.py` reads it:

- After every analyse job, of any kind, whose base includes `size.py`, the session runs `python3 -I .mintswarm/dogfood/plan-approval/size.py analysis <analysis>` on the saved analysis before publishing it. Exit 0 prints `size: <value>`. Exit 1 means the section is missing or malformed: the analysis is unusable, and the analyse stage reruns. Exit 2 stops the job until the cause is fixed.
- For every plan that goes to the maintainer, the session runs `python3 -I .mintswarm/dogfood/plan-approval/size.py plan .mintswarm/job/inputs/artefacts/plan.md` in the assess job's worktree. It prints the size, raised to the lines floor of the footprint's `production_lines` when the judged size is below it, with the line `size raised from <judged> to <floor> by planned lines`, or the lines floor and a line saying the plan states no usable size. A missing or malformed section in a plan is also an assessment finding.

## The plan's dependency screen

For every plan the plan stage returns, revisions included and in every mode, the session screens the packages the plan declares, right after laying out the assess job (step 2) and before the floor (step 3). It runs, at the root of the assess job's worktree:

```
python3 -I .mintswarm/dogfood/dependencies/screen.py plan --plan .mintswarm/job/inputs/artefacts/plan.md \
  --root . --out .mintswarm/job/outputs/screen.json
```

It runs with network access to `crates.io`, `index.crates.io`, `static.crates.io`, `registry.npmjs.org` and `api.osv.dev` only, over HTTPS, with no registry or other credential in its environment, and with the system CA bundle readable so that the default verified TLS context works. It reads the `new_dependencies` of the plan's `## Footprint` block (an empty list gives a record with no packages) and writes the versioned screen record to the assess job's `.mintswarm/job/outputs/screen.json`, which, like `floor.json`, is not committed. A package it cannot screen in full is recorded as `unscreened` with its reason, never as clean, and the script still exits 0. An exit of 2 (bad arguments, an unreadable plan or footprint, an unreadable `popular-names.json`, or a record it cannot write) means there is no usable record: the session stops the plan's approval until the cause is fixed and the screen reruns. Nothing in the assess job's `inputs/` gets the record.

## Outcomes

- **Exit 2 or no record** counts as `human`. Exit 0 counts as `auto` only together with a `decision.json` whose `decision` is `auto`.
- **`shadow`:** post the shadow-record comment that `decision.py` printed on the issue, through `bot-gh.sh` (`-F body=@<file>`), unchanged except for one line the session appends with the URLs GitHub returned for the plan, `Plan assessment` and decision artifacts. Never write the body by hand: the script sets model and session text (the reason, the doubts, the harness, model, artifact, paths and ref) on one line, without backticks, at most 200 characters, in code spans, so mentions, links and Markdown in it do nothing. The body carries what shadow records 1 to 3 carried by hand:
  - the plan: its path, sha256 and the footprint in brief;
  - assess: the stage, the harness and model, whether they are as configured, the band's name and the confidence (for example `Likely safe · 92`) with the threshold, the decisive reason, the hard triggers, one row per doubt and the artifact;
  - the floor: the base commit, `--base`, the result, the exit code, and the hit and clean rules;
  - the would-be decision, `judged_decision`, and the decision;
  - the maintainer decision, pending.

  The plan then goes to the maintainer as before, with the approval package below; once it is posted, the session assigns the issue with `.mintswarm/dogfood/github/assign-approvers.py assign <issue> plan --base <base commit>`. When the maintainer decides, record that decision beside the judgement, in the shadow-record comment and in the plan artifact: approved or rejected, whether it agrees with `judged_decision`, and any observation for the threshold review, such as a rule that should have hit or a path that should require a human.
- **`auto`:** post the printed comment on the issue through `.mintswarm/dogfood/github/bot-gh.sh` (`-F body=@<file>`), unchanged. Implement then starts without the maintainer, and nobody is assigned, because nobody is waited on. The PR description of an auto-approved item says that the plan was auto-approved and why: the mode, the floor clean on all 16 rules, the band and confidence against `min_confidence`, the reason and the doubts, and links to the assessment and decision artifacts.
- **`human`** (under `always`, `shadow`, or `judged` with a hit, a hard trigger, a confidence below `min_confidence`, an unusable layer, or assess run on a harness or model other than the configured one): the plan goes to the maintainer with the approval package; once it is posted, the session assigns the issue with `.mintswarm/dogfood/github/assign-approvers.py assign <issue> plan --base <base commit>`.

## The approval package

For every plan that goes to the maintainer, the session posts the approval package as one comment on the implementation issue through `bot-gh.sh` (`-F body=@<file>`). It gives the full URLs GitHub returned (`html_url`) of the plan artifact, the TL;DR comment for that plan (step 1), the `Plan assessment` artifact, the decision artifact and, in `shadow` mode, the shadow-record comment, and holds:

- the plan artifact, with its approach and footprint in brief;
- the size: the lines `size.py plan` printed, unchanged, or, when it exits 2, a line saying the size could not be read;
- the floor verdict: hit rules with their reasons, or clean;
- the band and confidence with the threshold, the hard triggers, the reason and the doubts, and the `Plan assessment` artifact;
- the mode and the decision, with `judged_decision` and the decision artifact;
- the open questions of the plan, if any.

The maintainer approves, asks for changes or rejects; nothing starts implement until they approve. Whichever they decide, the session then releases the issue with `.mintswarm/dogfood/github/assign-approvers.py release <issue> plan`. A revised plan is assigned again when it is published, and so is a plan that a veto returns to awaiting plan approval.

## Veto

A veto stops an auto-approved plan. It is either:

- an issue comment from an author in `tracker.ingest.trusted.authors` of the base config whose first line is `veto`; or
- the maintainer telling the session.

The session checks for one before implement starts, before it records implement as finished, and before it asks for PR approval.

- **Until implement finishes**, a veto cancels the job and returns the item to awaiting plan approval: the plan goes to the maintainer with the approval package, as for `human`.
- **After implement finishes**, a veto is a rejection at PR approval: the PR is not approved, and the item follows what the maintainer says there.

## Decisions while a job is in flight

A maintainer decision taken while a job is in flight, for example one that changes what a criterion asks, is not left only in a comment, because later stages read the job's inputs, not the comments:

1. Apply it to the issue itself: amend the acceptance criteria or the body, with a note that points to the decision.
2. The decision comment carries the line `<!-- mintswarm: steer -->`.
3. Refresh the job's `.mintswarm/job/inputs/issue.md` from the issue before the next stage runs.
4. If the plan was already through the floor, rerun the floor (step 3) and the decision (step 7) on the refreshed `issue.md`: both bind the documents they read, so a verdict from before the refresh no longer counts. A `floor.json` of version 1, from before the floor read the issue document and the analysis, also counts as `human` until the floor reruns.

## The review job's mechanical check

When the approved plan has a `## Mechanical check` section (rule 10 of `.mintswarm/stages/plan.md`), the session writes `.mintswarm/job/inputs/mechanical-check.md` in every review round's job:

- The first line is `Base: <commit>`, the commit the review job's `diff.patch` is made against (the merge base of the head with `origin/master` when the session writes the review inputs), followed by a blank line. This can differ from the job's plan-approval base, for example after a rebase before the first push; the check must compare against the same commit as `diff.patch`.
- Then comes the plan's section, from its `## Mechanical check` line up to, not including, the next `## ` heading outside a fenced block, copied unchanged.
- The section is taken from the plan the maintainer approved, or that was approved without a human, never from a later revision.

Nothing else from the plan goes into the review job. Without the section, the file is absent.

## The review job's dependency inputs

In every review round, the session runs `python3 -I .mintswarm/dogfood/dependencies/new-packages.py --base <diff.patch base> --head HEAD --out .mintswarm/job/inputs/dependencies` at the worktree root, with the same base as `diff.patch`. The script reads `pipeline.dependencies.review` from that base's config, and writes `packages.json` only when the option is `agent` and the change touches a dependency file. When it writes no file, the review job gets no dependency inputs. An exit other than 0 stops the round until the cause is fixed.

Right after it, in every round, the session runs `python3 -I .mintswarm/dogfood/dependencies/prefetch.py --dir .mintswarm/job/inputs/dependencies` at the worktree root, with network access to `crates.io`, `index.crates.io`, `static.crates.io` and `registry.npmjs.org` only and no registry credential in its environment. The script first removes the previous round's prefetch output (`prefetch.json`, `cargo/`, `npm/`); with no `packages.json` it prints `no packages.json` and writes nothing. Otherwise it writes each package's trimmed registry metadata and extracted manifest and build or install code beside `packages.json`, executing nothing from the package. A package it refuses or fails to fetch is recorded in `prefetch.json` with its reason, and the review rejects it as unverifiable. An exit other than 0 stops the round until the cause is fixed.

## PR approval

PR approval stays with a human in every mode, including `never`. Plan approval only decides whether implement may start without the maintainer.

## Records

Shadow records, `Plan assessment` artifacts and decision records live in issue comments and issue artifacts, never in the repository. `.mintswarm/job/` is not committed.
