# GitHub identity for the dogfooding session

While mintswarm is dogfooded, the session acting as the runner pushes, opens PRs and writes issues and artifacts as the `mintswarm-dev` GitHub App (id 5874301), whose bot user is `mintswarm-dev[bot]` (user id 359856778). These scripts are how it does that without using the maintainer's SSH key or `gh` login, and without writing the App's key or a token to disk.

The App's private key lives in the maintainer's login Keychain, in a generic password with service `mintswarm-dev-private-key` and the App's client id as account. The public identifiers live in `~/.config/mintswarm/dev-app.json`. Installation tokens (`ghs_…`, one hour) are minted on demand and only ever held in shell variables and the environment of the process that needs them.

## Scripts

| Script | What it does | Secrets it touches |
| --- | --- | --- |
| `common.sh` | Sourced by the others. Holds the repository, bot identity and config paths; `msw_token`, and `msw_git_net`, which runs git with only the repository's config and an inline credential helper fed from `$MSW_TOKEN`. | the token, from the environment |
| `mint-token.sh` | `jwt` prints an App JWT; `token [PERMS_JSON]` prints an installation token for `mintswarm/mintswarm`, optionally with narrower permissions. Used by the other scripts; do not run `token` bare in the session. | the key (Keychain to `openssl` on a pipe), the JWT (to `curl` on stdin), the token (stdout) |
| `push.sh BRANCH [--force-with-lease=SHA]` | Pushes a `mintswarm/*` branch to `https://github.com/mintswarm/mintswarm.git` as the bot, after checking every commit in `origin/master..BRANCH` is the bot's. | the token, minted with `contents: write` unless `MSW_TOKEN` is set |
| `bot-gh.sh ARGS…` | Runs `gh` with a fresh installation token as `GH_TOKEN`, `GH_REPO=mintswarm/mintswarm` and the maintainer's tokens removed from the environment; does not run `gh` at all if minting fails. | the token, as `GH_TOKEN` |
| `request-review.py PR --branch B` | Resolves reviewers for the PR's changed paths from `.mintswarm/config.yaml` and requests them through `bot-gh.sh`. `--dry-run` only prints them. | none directly |
| `bot-identity.sh install\|status\|uninstall` | Makes every commit on a `mintswarm/*` branch authored and committed by `mintswarm-dev[bot]`, through a conditional include in the repository's shared git config. | none |
| `store-credentials.sh prepare\|convert\|rotate` | Creates the App from `create-app.html` and stores its key, or imports a new key when rotating. | the key, from GitHub or the downloaded PEM to the Keychain on the stdin of `security -i` |
| `create-app.html` | The App's manifest: `contents`, `pull_requests` and `issues` write; `checks`, `statuses` and `metadata` read; no events. `prepare` fills in its `state`. | none |
| `selftest.sh` | Offline test of all of the above against stubs of `security`, `curl` and `gh` in a temporary sandbox. Reaches neither the Keychain nor GitHub. | throwaway keys and dummy tokens only |

The scripts need bash (3.2 is enough), `jq`, `openssl`, `curl`, `git`, `gh` and, for `request-review.py`, `python3` and `yq`. `selftest.sh` also needs `shellcheck`.

## Setup

The App already exists and is installed on `mintswarm/mintswarm`. On another machine, give it its own key (Rotation, steps 1 and 2, with `dev-app.json` copied over first) and then follow steps 3 to 5.

1. **Re-creating the App**, only if it is gone:
   1. `store-credentials.sh prepare` writes `~/.config/mintswarm/create-app.html` with a random `state` and prints its path.
   2. Open that file in a browser signed in as an owner of the `mintswarm` organisation and confirm. GitHub redirects to `http://localhost:8765/?code=CODE&state=STATE`; nothing needs to listen there, copy both values from the address bar.
   3. Within an hour, `store-credentials.sh convert CODE STATE`. It checks `STATE`, imports the key into the Keychain, writes `dev-app.json` and prints the key's fingerprint; compare it with the App's settings page. The client secret and webhook secret are discarded.
2. **Install the App** on `mintswarm/mintswarm` only (organisation settings, GitHub Apps, `mintswarm-dev`, Configure).
3. **Bot commit identity:** from any worktree of the repository, `bot-identity.sh install`. Check with `bot-identity.sh status` on a `mintswarm/*` branch.
4. **Check the token works:** `bot-gh.sh api repos/{owner}/{repo} --jq .full_name` prints `mintswarm/mintswarm`.
5. **Run the self-test** after changing any script: `selftest.sh`.

The include from step 3 makes every commit on a checked-out `mintswarm/*` branch the bot's, including the maintainer's own: those branches are reserved for the factory. It does not apply where `HEAD` is detached: on a detached checkout, and in commands run by a separate git process during a rebase, such as `rebase --exec` or `rebase --continue` after a conflict. The commits the rebase itself rewrites are the bot's. For the other cases, give the identity explicitly:

```
git -c user.name='mintswarm-dev[bot]' -c user.email='359856778+mintswarm-dev[bot]@users.noreply.github.com' rebase --continue
```

`push.sh` refuses any commit that is not the bot's, so a slip is caught before it leaves the machine.

## Rotation

1. In the App's settings on GitHub, generate a new private key. The browser downloads `mintswarm-dev.<date>.private-key.pem`.
2. `store-credentials.sh rotate ~/Downloads/mintswarm-dev.<date>.private-key.pem`. It imports the key into the same Keychain item, reads it back to compare fingerprints, updates `key_fingerprint` in `dev-app.json`, deletes the downloaded file (`--keep` keeps it) and prints the fingerprint.
3. Mint a token with the new key: `bot-gh.sh api repos/{owner}/{repo} --jq .full_name`.
4. Only then delete the old key in the App's settings, matching it by fingerprint.

Before the first `convert` or `rotate` on a machine, check once that `security -i` accepts commands on stdin, with a dummy item:

```
printf 'add-generic-password -s msw-selftest -a t -w abc\n' | security -i
security find-generic-password -s msw-selftest -w     # prints abc
security delete-generic-password -s msw-selftest
```

## Session procedure

Every push, PR, issue and artifact the session makes as the runner goes through these scripts; plain `git push` and plain `gh` are never used for factory work.

1. **Push:** `.mintswarm/dogfood/github/push.sh mintswarm/<id>-<slug>`. After a rebase, `push.sh BRANCH --force-with-lease=<sha the branch has on GitHub>`.
2. **Open the PR** through the REST API, never with `gh pr create`:

   ```
   bot-gh.sh api repos/{owner}/{repo}/pulls -f head=mintswarm/<id>-<slug> -f base=master \
     -f title='feat(<component>): …' -F body=@<file> --jq .number
   ```

   Add `-F draft=true` for a draft.
3. **Request review:** `request-review.py <number> --branch mintswarm/<id>-<slug>`. Reviewers come from `components.<name>.reviewers` for the components whose paths the PR changes, otherwise from `pipeline.review.reviewers` in `.mintswarm/config.yaml`. GitHub requests code owners itself.
4. **Marked ready:** after `bot-gh.sh pr ready <number>`, run `request-review.py` again.
5. **Issues, comments and artifacts:** `bot-gh.sh api …` or `bot-gh.sh issue …`. `gh` 2.83 has no `gh issue artifact`, so artifacts go through `bot-gh.sh api`.

Each Bash call in the session is a fresh shell, so the scripts mint a token per call; that stays far inside the rate limits below.

## Pitfalls

- **Never push to `origin` or any SSH remote.** In this repository `origin` is `git@github.com:mintswarm/mintswarm.git`. A push to it authenticates with the maintainer's SSH key, so GitHub records the push as the maintainer's, whatever the commits say. `push.sh` always pushes to the HTTPS URL and never through a named remote.
- **Never put a token in a remote URL** (`https://x-access-token:…@github.com/…`): it ends up in `.git/config` and the transcript.
- **Never run `gh` without `GH_TOKEN`**, that is, never plain `gh`: it falls back to the maintainer's stored login. Use `bot-gh.sh`.
- **Never let `gh pr create` push the branch:** it may push through `origin`. Push with `push.sh`, open the PR with `bot-gh.sh api …/pulls`.
- **Never print a token or the key:** no bare `mint-token.sh token`, no `set -x`, no `curl -v`, no `GIT_TRACE_REDACT=0`, no `GIT_CURL_VERBOSE`. The session transcript is kept on disk.
- **Never store a token** in a git or `gh` credential store; `push.sh` uses an inline helper and `GIT_CONFIG_GLOBAL=/dev/null`, so `osxkeychain` and `store` never see it. To check after a push: `security find-internet-password -s github.com -a x-access-token` finds nothing, and `~/.git-credentials` has no `x-access-token` line.
- **Do not set `url.*`, `credential.*` or `http.*.extraheader` in the repository's git config;** `push.sh` refuses to push while they are set. `GIT_AUTHOR_*` or `GIT_COMMITTER_*` in the environment override the bot identity; `push.sh` refuses the resulting commits.
- **Do not rely on the token's length;** it varies (390 characters has been seen). Only the `ghs_` prefix is checked.
- **`@me` and `gh pr status` do not work** with an installation token (`GET /user` is 403). Name logins explicitly.

## Limits

- GitHub's secondary rate limits for content-creating requests: 80 per minute and 500 per hour.
- An installation token lasts one hour and reaches only `mintswarm/mintswarm`.

## Not covered

- Verified (signed) commits: bot commits are unsigned; squash merges are signed by GitHub.
- The `master` ruleset, which is the maintainer's to configure.
