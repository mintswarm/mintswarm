# mintswarm

mintswarm turns tickets into reviewed pull requests. You shape an idea into a design; runners on your own machines analyse it and decompose it into tickets, then carry each implementation ticket through analyse, plan and implement, and run its gates; each plan is approved by you or, where you allow it, by mintswarm, always or only when its confidence that the plan is safe to approve reaches the project's threshold (`pipeline.plan_approval.min_confidence`); an independent reviewer checks the result; you approve the PR.

It is built first for a developer or small team running several projects on one AI subscription:

- **Schedules against the subscription.** One account budget shared fairly by every project and runner, with throttle-aware pausing.
- **Runs on a fleet you own.** Linux and macOS runners, matched to work by extensible capability probes (Xcode, sanitisers, game engines, …).
- **Harness-neutral.** Claude Code first, Codex next, behind one adapter contract.
- **Reviews independently.** A fresh session sees the ticket, acceptance criteria and diff, never the implementer's transcript.
- **Never lets the model declare success.** Gates are run by the runner, outside the agent's shell; agents commit, the runner pushes.
- **Puts you in the loop from your own agent.** A console MCP to shape tickets, answer questions, steer and interrupt running work.

## Status

Design phase. Nothing here is usable yet; the first milestone is a proof of concept with the control plane running locally and runners connecting to it.

## Licence

mintswarm is **fair source**, licensed under the [Functional Source License, Version 1.1, ALv2 Future License](LICENSE.md) (FSL-1.1-ALv2). You may use, modify and self-host it, commercially included, for any purpose other than offering a competing product or service. Each release becomes available under the Apache License 2.0 two years after it is published.

## Contributing

Contributions need a signed Contributor Licence Agreement. How to branch, commit and open a pull request is set out in [CONTRIBUTING.md](CONTRIBUTING.md).
