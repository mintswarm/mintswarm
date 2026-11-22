# Plan approval while dogfooding

This directory stands in for the control plane's plan approval: the deterministic floor (`floor.py`) and the decision that combines it with the assess stage's judgement (`decision.py`). Until the control plane exists, the session runs them by hand for every implementation plan, as described here. `selftest.py` tests both scripts offline; run it with `python3 -I .mintswarm/dogfood/plan-approval/selftest.py`.

The mode is `pipeline.plan_approval.mode` in `.mintswarm/config.yaml` at the job's base commit, never the worktree's copy:

- `always`: every plan goes to a human.
- `shadow`: floor, assess and the decision run and are recorded, and every plan still goes to a human. The maintainer's own decisions become the comparison for the threshold review.
- `judged`: a plan is approved without a human only when the floor is clean and the risk is `low`.
- `never`: every plan is approved without a human.

Both layers run in every mode, so a disagreement between them, or with the maintainer, is always visible.

## Steps

After the plan stage returns a plan, in this order:

1. **Lay out the assess job.** Put the plan at the assess job's `.mintswarm/job/inputs/artefacts/plan.md`, beside `.mintswarm/job/inputs/issue.md` and `.mintswarm/job/inputs/artefacts/analysis.md`. Nothing else goes under `inputs/`: no parent artefacts, no floor verdict, no planner transcript.
2. **Run the floor** in that job's worktree, on that file, with `--base` at the job's base commit:

   ```
   python3 -I .mintswarm/dogfood/plan-approval/floor.py --plan .mintswarm/job/inputs/artefacts/plan.md \
     --component <component> --base <base commit>
   ```

   It writes `.mintswarm/job/outputs/floor.json`.
3. **Run assess** (`msw-assess`) in a fresh subagent that receives only `issue.md`, `analysis.md` and `plan.md`: no summary of the plan stage, no floor verdict, no other context. Run it on the model `agents.stages.assess` names in the base config, which differs from the plan stage's model.
4. **Save the assessment** from the subagent's reply with `.mintswarm/dogfood/save-output.py` to `.mintswarm/job/outputs/assessment.md`.
5. **Publish the assessment as an issue artifact** on the implementation issue, through `bot-gh.sh api` (see "Session procedure" in `.mintswarm/dogfood/github/README.md`).
6. **Run the decision** with the same plan file, the assessment, the assessment's artifact, the same base, and the harness and model assess actually ran on:

   ```
   python3 -I .mintswarm/dogfood/plan-approval/decision.py --issue <number> --component <component> \
     --plan .mintswarm/job/inputs/artefacts/plan.md --assessment .mintswarm/job/outputs/assessment.md \
     --assessment-artifact <artifact URL> --base <base commit> --harness <harness> --model <model>
   ```

   It writes `.mintswarm/job/outputs/decision.json`, exits 0 for `auto` and 1 for `human`, and for `auto` prints the issue comment to stdout. Keep stdout in a file for the comment.
7. **Publish `decision.json` as an issue artifact** on the same issue.
8. **Record the decision in the session's plan artifact**: the mode, the decision and `judged_decision`, the floor result, the risk and its reason, and links to the assessment and decision artifacts.

## Outcomes

- **Exit 2 or no record** counts as `human`. Exit 0 counts as `auto` only together with a `decision.json` whose `decision` is `auto`.
- **`shadow`:** post a shadow-record comment on the issue, through `bot-gh.sh`. From the record, it carries what shadow records 1 to 3 carried by hand:
  - the plan: its artifact and the footprint in brief;
  - assess: the stage, the harness and model, the risk, the decisive reason and the artifact;
  - the floor: the script, the commit it ran at, `--base`, the verdict, the exit code, and the hit and clean rules;
  - the would-be decision, `judged_decision`;
  - the decision artifact.

  The plan then goes to the maintainer as before, with the approval package below. When the maintainer decides, record that decision beside the judgement, in the shadow-record comment and in the plan artifact: approved or rejected, whether it agrees with `judged_decision`, and any observation for the threshold review, such as a rule that should have hit or a path that should require a human.
- **`auto`:** post the printed comment on the issue through `.mintswarm/dogfood/github/bot-gh.sh` (`-F body=@<file>`), unchanged. Implement then starts without the maintainer. The PR description of an auto-approved item says that the plan was auto-approved and why: the mode, the floor clean on all 13 rules, the risk and its reason, and links to the assessment and decision artifacts.
- **`human`** (under `always`, `shadow`, or `judged` with a hit, a risk above `low` or an unusable layer): the plan goes to the maintainer with the approval package.

## The approval package

For a `human` decision, the maintainer gets, in one message:

- the plan artifact, with its approach and footprint in brief;
- the floor verdict: hit rules with their reasons, or clean;
- the risk with its reason, and the assessment artifact;
- the mode and the decision, with `judged_decision` and the decision artifact;
- the open questions of the plan, if any.

The maintainer approves, asks for changes or rejects; nothing starts implement until they approve.

## Veto

A veto stops an auto-approved plan. It is either:

- an issue comment from an author in `tracker.ingest.trusted.authors` of the base config whose first line is `veto`; or
- the maintainer telling the session.

The session checks for one before implement starts, before it records implement as finished, and before it asks for PR approval.

- **Until implement finishes**, a veto cancels the job and returns the item to awaiting plan approval: the plan goes to the maintainer with the approval package, as for `human`.
- **After implement finishes**, a veto is a rejection at PR approval: the PR is not approved, and the item follows what the maintainer says there.

## PR approval

PR approval stays with a human in every mode, including `never`. Plan approval only decides whether implement may start without the maintainer.

## Records

Shadow records and decision records live in issue comments and issue artifacts, never in the repository. `.mintswarm/job/` is not committed.
