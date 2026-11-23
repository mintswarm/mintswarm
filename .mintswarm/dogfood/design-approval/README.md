# Design approval while dogfooding

This directory describes how the session takes a design document to approval until the runner does it. Design issues run analyse, design, approval and decompose. Design documents are always approved by a human (N6), like decompositions, until a judged mode is designed with #113. No floor, assess or decision runs for a design, and `pipeline.plan_approval.mode` does not apply to it.

## Steps

After the design stage (`.mintswarm/stages/design.md`) returns a design, in this order:

1. **Publish the design** as an issue artifact on the design issue, through `bot-gh.sh api` (see "Session procedure" in `.mintswarm/dogfood/github/README.md`), and note the sha256 of the published document.
2. **Post the approval package** (below) as one comment on the design issue through `bot-gh.sh` (`-F body=@<file>`), with the full URLs GitHub returned (`html_url`).
3. **Assign the issue** with `.mintswarm/dogfood/github/assign-approvers.py assign <issue> plan --base <base commit>`. The script has no `design` reason yet, so design approvals use `plan` until it gains one.
4. **Act on the maintainer's answer**, a comment whose first line is the command:
   - `approve`, which must cite the sha256 of the current design (Q1); an approval of an earlier revision does not count. Release the issue with `assign-approvers.py release <issue> plan`, and place the approved revision at the decompose job's `.mintswarm/job/inputs/parent/design.md`.
   - `changes`: release the issue, apply the comment as a steer under "Decisions and notes" in the job's `issue.md` (with the line `<!-- mintswarm: steer -->`, as in `.mintswarm/dogfood/plan-approval/README.md`), put the previous design at `.mintswarm/job/inputs/artefacts/design.md`, and run the design stage again. The revision goes through these steps from step 1.
   - `reject`: release the issue; nothing more runs for it until the maintainer says otherwise.

## The approval package

One comment on the design issue, holding:

1. the design artifact's full URL and its sha256, which an `approve` must cite;
2. the design's summary line and its goals;
3. the decisions it takes, binding and adopted from the analysis;
4. its open questions, each with its recommendation;
5. for a revision, what changed and the change request it answers;
6. the analysis artifact's full URL, the commands `approve`, `changes` and `reject`, and the statement that design approval is always human, with no floor, assess or decision.

## Records

Design artifacts, approval packages and the maintainer's answers live in issue comments and issue artifacts, never in the repository. `.mintswarm/job/` is not committed.
