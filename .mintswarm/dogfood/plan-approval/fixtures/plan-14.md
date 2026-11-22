# Plan: Bot identity tooling for the dogfooding session

Fixture modelled on the plan of #14 (component `agents`), for selftest.py.

## Approach

Derived from the approved plan of #14, merged as f6b3313, component `agents`. `files` follows the plan's Steps, checked against the merged commit: the ten new files under `.mintswarm/dogfood/github/` and `.mintswarm/config.yaml`. `estimated_lines` is 1504, the `git show --numstat --no-renames f6b3313` total. Two external steps are needed for the change to work: the bot identity include installed before implement, and the one-off `security -i` check against the real Keychain, which decides how the key is stored. `gate_changes` is empty: no gate changes, though the config that holds them does.

## Steps

1. **Shared constants and helpers** — files: `.mintswarm/dogfood/github/common.sh` (new).
2. **Token minting** — files: `.mintswarm/dogfood/github/mint-token.sh` (new).
3. **Creating the App and rotating its key** — files: `.mintswarm/dogfood/github/create-app.html` (new), `.mintswarm/dogfood/github/store-credentials.sh` (new).
4. **Bot commit identity** — files: `.mintswarm/dogfood/github/bot-identity.sh` (new).
5. **Bot push** — files: `.mintswarm/dogfood/github/push.sh` (new).
6. **`gh` as the bot** — files: `.mintswarm/dogfood/github/bot-gh.sh` (new).
7. **Review requests** — files: `.mintswarm/dogfood/github/request-review.py` (new), `.mintswarm/config.yaml`.
8. **Offline self-test** — files: `.mintswarm/dogfood/github/selftest.sh` (new).
9. **README** — files: `.mintswarm/dogfood/github/README.md` (new).

## Footprint

```json
{
  "footprint": {
    "files": [
      ".mintswarm/dogfood/github/common.sh",
      ".mintswarm/dogfood/github/mint-token.sh",
      ".mintswarm/dogfood/github/create-app.html",
      ".mintswarm/dogfood/github/store-credentials.sh",
      ".mintswarm/dogfood/github/bot-identity.sh",
      ".mintswarm/dogfood/github/push.sh",
      ".mintswarm/dogfood/github/bot-gh.sh",
      ".mintswarm/dogfood/github/request-review.py",
      ".mintswarm/config.yaml",
      ".mintswarm/dogfood/github/selftest.sh",
      ".mintswarm/dogfood/github/README.md"
    ],
    "new_files": [
      ".mintswarm/dogfood/github/common.sh",
      ".mintswarm/dogfood/github/mint-token.sh",
      ".mintswarm/dogfood/github/create-app.html",
      ".mintswarm/dogfood/github/store-credentials.sh",
      ".mintswarm/dogfood/github/bot-identity.sh",
      ".mintswarm/dogfood/github/push.sh",
      ".mintswarm/dogfood/github/bot-gh.sh",
      ".mintswarm/dogfood/github/request-review.py",
      ".mintswarm/dogfood/github/selftest.sh",
      ".mintswarm/dogfood/github/README.md"
    ],
    "estimated_lines": 1504,
    "new_dependencies": [],
    "external_steps": [
      "Before implement, the maintainer installs the bot identity include (bot-identity.gitconfig and the includeIf onbranch:mintswarm/ entry).",
      "The maintainer checks security -i once against the real Keychain with a dummy item; the result decides how store-credentials.sh passes the key."
    ],
    "gate_changes": [],
    "overridden_assumptions": []
  }
}
```
