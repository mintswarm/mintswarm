# GitHub identity for the dogfooding session

While mintswarm is dogfooded, the session acting as the runner pushes, opens PRs and writes issues and artifacts as the `mintswarm-dev` GitHub App (id 5874301), whose bot user is `mintswarm-dev[bot]` (user id 359856778). These scripts are how it does that without using the maintainer's SSH key or `gh` login, without ever writing a token to disk, and without writing the App's key to disk unencrypted unless the file key store is chosen explicitly.

The App's private key lives in a key store: the macOS login Keychain, Secret Service, `pass`, `systemd-creds` or a plain file (see [Key store](#key-store)). Every store keeps it under the service `mintswarm-dev-private-key` with the App's client id as account. The public identifiers and the name of the key store in use (`key_store`) live in `~/.config/mintswarm/dev-app.json`. Installation tokens (`ghs_…`, one hour) are minted on demand and only ever held in shell variables and the environment of the process that needs them.

A second App, `mintswarm-dev-steward`, is the landing identity: it has its own key, under its own service, and its own config, `~/.config/mintswarm/steward-app.json` (see [Steward](#steward)).

## Scripts

| Script | What it does | Secrets it touches |
| --- | --- | --- |
| `common.sh` | Sourced by the others. Holds the repository, bot identity and config paths; `msw_app_profile`, the per-App table of config file, key service, key label and confirmation; `msw_token`; `msw_recorded_key_store`; and `msw_git_net`, which runs git with only the repository's config and an inline credential helper fed from `$MSW_TOKEN` that answers only for `https://github.com`. | the token, from the environment |
| `key-store.sh backend\|put\|get\|delete` | Keeps a value in a key store, with the value on stdin and stdout only (see [Key store](#key-store)). `backend` prints the store `put` would use; `put --confirm` makes every later read of a Keychain item ask for confirmation (see [Per-use confirmation](#per-use-confirmation)). `get` refuses to write to a terminal; never run `get` for the App's key in the session, not even into `cat` or a pager: it prints the private key into the transcript. | the key, on stdin or stdout, to and from the store |
| `mint-token.sh` | `jwt` prints an App JWT; `token [PERMS_JSON]` prints an installation token for `mintswarm/mintswarm`, optionally with narrower permissions. Reads the key only from the key store recorded in `dev-app.json`. Used by the other scripts; do not run `token` bare in the session. | the key (key store to `openssl` on a pipe), the JWT (to `curl` on stdin), the token (stdout) |
| `push.sh BRANCH [--force-with-lease=SHA]` | Pushes a `mintswarm/*` branch to `https://github.com/mintswarm/mintswarm.git` as the bot, after checking every commit in `origin/master..BRANCH` is the bot's. | the token, unless `MSW_TOKEN` is set: minted with `contents: write`, plus `workflows: write` when `.github/workflows/` changes from the merge base with `origin/master` to `BRANCH` or, with a lease, from the lease commit to `BRANCH` (a rebase onto a master that gained a workflow change); with a lease, the lease commit must be present locally. A branch that differs from master under `.github/workflows/` asks for it on every push, for as long as it differs |
| `bot-gh.sh ARGS…` | Runs `gh` with a fresh installation token as `GH_TOKEN`, `GH_REPO=mintswarm/mintswarm` and the maintainer's tokens removed from the environment; does not run `gh` at all if minting fails. | the token, as `GH_TOKEN` |
| `request-review.py PR --branch B` | Resolves reviewers for the PR's changed paths from `.mintswarm/config.yaml` and requests them through `bot-gh.sh`. `--dry-run` only prints them. | none directly |
| `assign-approvers.py assign\|release NUMBER REASON` | Assigns an issue or PR to `pipeline.review.reviewers` from `.mintswarm/config.yaml` at `--base` (default `origin/master`) while a human is waited on, and `release` removes only the assignments the bot made, judged from the item's events. Never assigns a `[bot]` login, posts only approvers not already assigned, and exits 2 when no human approver is resolved; REASON is `plan`, `needs_input`, `pr` or `external_steps`. `--dry-run` only prints the logins. | none directly |
| `reclassify.py split\|check-parent` | `split ISSUE --analysis PATH --analysis-url URL` reads the `## Split` json of an implementation issue's analysis and, when it proposes a split, sets the issue's type to the plan type of `tracker.kinds` in `.mintswarm/config.yaml` at `--base` (default `origin/master`), checks that GitHub took it, posts one comment (`<!-- mintswarm: reclassified -->`) naming the finding and the analysis URL, and prints the job kind, the pipeline-state text, the comment URL and the decomposition approval package line; on an issue already of the plan type it changes no type, posts the comment only if no comment holds the marker yet (so a re-run after a failed comment finishes the job), and prints the same lines. The `## Split` heading and its json fence are found outside fenced blocks, as the plan-approval floor finds sections. `check-parent PARENT` exits 1 unless the parent's type is the plan or the design type. `split --dry-run` changes nothing. | none directly |
| `msw-state.py set\|get` | `set ISSUE VALUE` sets the issue's `msw-state` value (see [msw-state](#msw-state)) as `tracker.state` in `.mintswarm/config.yaml` at `--base` (default `origin/master`) chooses: the organisation issue field `msw-state`, by its recorded `field_id`, or exactly one `msw-state:<value>` label, swapped in one `PUT` of the issue's labels. It writes only when the value differs and checks GitHub's answer. Then, unless the value is `none` or `tracker.state.status` is `off`, it sets the issue status the value maps to through GraphQL `setIssueStatus`, whenever the status differs, even if the value was already current. `get ISSUE` prints the value and the status. `set --dry-run` changes nothing. | none directly |
| `bot-identity.sh install\|status\|uninstall` | Makes every commit on a `mintswarm/*` branch authored and committed by `mintswarm-dev[bot]`, through a conditional include in the repository's shared git config. | none |
| `store-credentials.sh create\|rotate` | `create MANIFEST [--unconfirmed-key]` runs the whole manifest flow: it writes a page that posts the manifest to GitHub, catches the redirect on `127.0.0.1` with `manifest-listener.py`, converts the code and stores the key and the identifiers in that App's own key-store item and config. `rotate PEM_PATH [--keep]` stores a new key for the App whose config `MSW_APP_CONFIG` names. | the code, from the listener to `curl` on stdin; the key, from GitHub or the downloaded PEM to `key-store.sh` on stdin |
| `manifests/*.json` | One manifest per App. `mintswarm-dev.json`: `contents`, `pull_requests`, `issues` and `workflows` write; `checks`, `statuses` and `metadata` read. `workflows` is there because agents author the CI files and GitHub refuses a whole push from an App that adds, edits or deletes a file under `.github/workflows/` without it; once it is accepted (Setup, step 2), `bot-gh.sh`'s unnarrowed tokens carry `workflows: write` too. `mintswarm-dev-steward.json`: the same without `issues` and `workflows` (it never pushes). Both private, with no events and no webhook, redirecting to `http://127.0.0.1:8765/`. | none |
| `manifest-listener.py PORT TIMEOUT_SECS PAGE` | Run by `create`. Listens on `127.0.0.1:PORT` only, accepts the first redirect whose `state` (given on stdin) matches, rejects every other request, prints the code on stdout and exits; gives up after the timeout, an absolute bound that no slow client can stretch (each connection gets at most 10 seconds). Logs no request. | the code, on stdout to `create` |
| `selftest.sh` | Offline test of all of the above against stubs of `security`, `secret-tool`, `pass`, `systemd-creds`, `uname`, `curl` and `gh` in a temporary sandbox; the file backend is tested for real inside the sandbox. Reaches no real key store and not GitHub. | throwaway keys and dummy tokens only |
| `linux-check.sh` | Runs `selftest.sh` and the real Linux backends in a throwaway Debian container (see [Linux check](#linux-check)). `linux/` holds its image and in-container script. | throwaway keys generated in the container only |

The scripts need bash (3.2 is enough), `jq`, `openssl`, `curl`, `git`, `gh` and, for `request-review.py`, `assign-approvers.py` and `reclassify.py`, `python3` and `yq`. `selftest.sh` also needs `shellcheck`, which it runs at severity `warning`: info and style notes differ between shellcheck versions (0.10 on Debian, 0.11 on Homebrew), so they do not fail it. Each key store needs its own tool:

| Key store | Needs |
| --- | --- |
| `keychain` | macOS `security` |
| `secret-service` | `secret-tool` (Debian and Ubuntu: `libsecret-tools`), a D-Bus session bus and a Secret Service provider such as GNOME Keyring or KeePassXC |
| `pass` | `pass` and `gnupg`, with an initialised store |
| `systemd-creds` | `systemd-creds`; systemd 256 or later for a non-root user |
| `file` | nothing beyond coreutils and `find` |

`linux-check.sh` needs Docker.

## Key store

`key-store.sh` keeps the key in one of five backends. Each takes the value on stdin and gives it back on stdout, so the key is never in argv or a temporary file.

| `MSW_KEY_STORE` | Where the key is kept | Chosen |
| --- | --- | --- |
| `keychain` | the login Keychain: a generic password with service `mintswarm-dev-private-key`, the client id as account and the label `mintswarm-dev GitHub App private key`, base64-encoded (the steward's: its own service and label, and no trusted application, see [Per-use confirmation](#per-use-confirmation)) | automatically on macOS |
| `secret-service` | the default Secret Service collection: an item with attributes `service` and `account` | automatically when `secret-tool` is on `PATH` and there is a session bus (`DBUS_SESSION_BUS_ADDRESS`, or a socket at `$XDG_RUNTIME_DIR/bus`) |
| `pass` | the entry `mintswarm/mintswarm-dev-private-key/<client id>` in the password store | automatically when `pass` is on `PATH` and `${PASSWORD_STORE_DIR:-~/.password-store}/.gpg-id` exists |
| `systemd-creds` | `~/.config/mintswarm/credstore/mintswarm-dev-private-key@<client id>.cred`, encrypted with the host key (and the TPM2 when there is one; `--user` for a non-root user); read from `$CREDENTIALS_DIRECTORY/mintswarm-dev-private-key@<client id>` first | only when set explicitly, or when `$CREDENTIALS_DIRECTORY` already holds the credential |
| `file` | `~/.config/mintswarm/keys/mintswarm-dev-private-key@<client id>`, **unencrypted**, mode 0600 in a 0700 directory | only when set explicitly |

**Which store is used.** `create` and `rotate` choose the store and record it as `key_store` in `dev-app.json`; after that, `mint-token.sh` reads the key only from the recorded store and ignores `MSW_KEY_STORE`, so it never reads from a different store than the one written to. The choice is `MSW_KEY_STORE` when set, and fails if that store is not available here. Otherwise it is the first available in the order of the table above, without `systemd-creds` unless `$CREDENTIALS_DIRECTORY` holds the credential, and never `file`. When nothing is available the command fails and lists the values of `MSW_KEY_STORE`; it never falls back to the file. A `dev-app.json` without `key_store`, as written before key stores existed, means `keychain` on macOS and is an error elsewhere.

- `key-store.sh backend` prints the store that would be chosen here; `jq -r .key_store ~/.config/mintswarm/dev-app.json` shows the one in use.
- To choose one: `MSW_KEY_STORE=<name> store-credentials.sh rotate PEM_PATH` (or `create`).
- **Moving to another store:** run `rotate` with the new `MSW_KEY_STORE`; once a token mints with the new key, delete the old item, as `rotate` prints: `MSW_KEY_STORE=<old> key-store.sh delete mintswarm-dev-private-key <client id>`. `rotate` leaves the old item where it is.
- **A systemd user service** can take the key without decrypting it itself: `LoadCredentialEncrypted=mintswarm-dev-private-key@<client id>:%h/.config/mintswarm/credstore/mintswarm-dev-private-key@<client id>.cred` in the unit puts it in `$CREDENTIALS_DIRECTORY`, which `key-store.sh` reads first.
- **The file store is the weakest:** the key is plain text on disk, protected only by its mode. It is never picked automatically and every use prints a warning. It refuses the key file or the `keys` directory when either is a symlink, is not yours, or has any group or other permission. It also refuses when `~/.config` or `~/.config/mintswarm` is a symlink, the usual layout of a dotfiles manager, and when the `keys` directory is inside a git work tree, such as a home directory kept in git. It cannot tell whether `~/.config` is synced, for example by a cloud drive; keep `~/.config/mintswarm/keys` out of any sync.
- **`pass` commits** each change when the password store is a git repository; the entry is encrypted, but the commit is pushed wherever that store is pushed.
- **Unlock first.** Reading from a locked keyring needs an unlock prompt: on the desktop a dialogue opens; with no display the prompt may fail at once or wait for an answer that never comes. `key-store.sh` stops every `secret-tool` call after `MSW_KEY_STORE_TIMEOUT` seconds (default 10) and then fails with "the keyring is probably locked; unlock it first"; when the prompt fails at once, `get` fails at once. Either way it never falls back to another store. `pass` may start `pinentry`, which is not bounded the same way. Unlock the keyring or the gpg key before the session starts. Choosing a store never runs the store's tool, so `key-store.sh backend` never prompts.

**What has been verified where.** The Keychain backend on macOS (#14, with `security -i`; and the maintainer's existing item and `dev-app.json` with `key-store.sh`, minting a token and pushing with `push.sh`). Every backend against stubs in `selftest.sh`, including a `secret-tool` that never answers. In the [Linux check](#linux-check), whose runs are recorded on PR #40, against the real tools:

- GNOME Keyring in a D-Bus session: selection, a byte-exact round trip, `delete`, `rotate` then `mint-token.sh jwt`, and failing without the bus with no fallback.
- `pass` with a throwaway gpg key.
- `systemd-creds` as root with the host key, with the null key ruled out.
- The file store with GNU coreutils, including a file owned by another user.
- No key in any traced argv and no plaintext copy on disk.

A locked collection is checked against a stub `secret-tool` that never answers (`get` and `put` fail within the bound and the stub is stopped). Against GNOME Keyring it counts as verified only for a recorded run whose phase B shows `ok` for the locked step; a `skip` there means that run reached no locked collection, and the `note` line shows how `get` failed. Not verified yet, on a real Linux host:

- Secret Service: GNOME Keyring unlocked at login through PAM; KeePassXC's Secret Service integration (access prompts, a locked database); the desktop prompt on a locked collection; an SSH session without the desktop's bus (expected: a clear error, no fallback).
- `pass`: a real `gpg-agent` and `pinentry`, hardware keys, and the commit to a git-backed store.
- `systemd-creds`: `--user` as a non-root user on systemd 256 or later; a real user service reading `$CREDENTIALS_DIRECTORY` through `LoadCredentialEncrypted=`; TPM2 binding on a machine with a TPM, including PCR changes after firmware updates.

### Per-use confirmation

An App's config records `key_confirm`: `keychain-acl` when every use of its key needs the user's confirmation, `none` otherwise. A config without it means `none`, which only `mintswarm-dev` may have; `rotate` refuses a steward config without it.

- `key-store.sh put --confirm SERVICE ACCOUNT [LABEL]` creates the Keychain item with no trusted application (`-T ""` on the stdin of `security -i`, and no `-U`). Not even `security` itself is trusted, so every read of the value asks for confirmation in a dialogue, and a read with no GUI session, such as over SSH, fails. It refuses an item that exists already, rather than update it with an access list it cannot see: delete the item first. The check reads attributes only, so it never asks.
- **Answer "Allow", never "Always Allow".** "Always Allow" adds `security` to the item's trusted applications, after which reads stop asking, silently: nothing here can tell. To undo it, delete the item and store the key again (see [Steward](#steward), Rotation).
- Only the `keychain` store has this. Every other store refuses `put --confirm`. Where the keychain store is available, as on a Mac, the key of an App whose profile asks for confirmation (the steward) always goes there with confirmation: `create` and `rotate` refuse any other `MSW_KEY_STORE`, and `create` refuses `--unconfirmed-key`.
- Where there is no keychain store, as on Linux, `create` refuses such an App unless `--unconfirmed-key` is given, which records `key_confirm` `none`. That is the only way to create the steward there; its key is then as well protected as the store makes it, and no more.
- `rotate` keeps the confirmation: for `keychain-acl` it stores the new key with `put --confirm`, in the keychain only. Where the keychain store is available it does so for the steward even when its config says `none`, and records `keychain-acl`.

Before the first `put --confirm` on a Mac, check once, with a dummy item, that it behaves as described:

```
printf 'abc\n' | key-store.sh put --confirm msw-selftest t
key-store.sh get msw-selftest t | cat          # asks; answer Allow; prints abc
key-store.sh get msw-selftest t | cat          # asks again; answer Deny; get fails
```

Then check that a `get` from an SSH session to the Mac fails, and that Keychain Access lists no application under the item's Access Control. Delete it with `key-store.sh delete msw-selftest t`. `selftest.sh` checks `put --confirm` only against a stub `security`, which records the line it gets; this check is what shows that the real one gives an item that asks.

## Setup

The App already exists and is installed on `mintswarm/mintswarm`. On another machine, copy `dev-app.json` over, give the machine its own key (Rotation, steps 1 and 2) and then follow steps 3 to 5. On Linux the copied file names no key store, so choose one in step 2, for example `MSW_KEY_STORE=secret-service store-credentials.sh rotate ~/Downloads/mintswarm-dev.<date>.private-key.pem`.

1. **Re-creating the App**, only if it is gone: `store-credentials.sh create .mintswarm/dogfood/github/manifests/mintswarm-dev.json` (with `MSW_KEY_STORE` set to choose a key store). `create` refuses to overwrite an existing `dev-app.json`: move it aside first.
   1. Before anything else it checks the manifest, that `dev-app.json` does not exist and that a key store is available.
   2. It writes `~/.config/mintswarm/create-mintswarm-dev.html` and starts listening on `127.0.0.1:8765`; when the port is taken it stops there and removes the page. Once it prints `listening on 127.0.0.1:8765; open … in the browser`, open the page in a browser signed in as an owner of the `mintswarm` organisation; it posts the manifest to GitHub, where you confirm the App's name.
   3. GitHub redirects to `http://127.0.0.1:8765/?code=…&state=…`, where `create` is listening. It accepts only a redirect with its own `state`, then converts the code, stores the key, writes `dev-app.json` and prints the key's fingerprint; compare it with the App's settings page. The client secret and webhook secret are discarded. With no matching redirect within `MSW_MANIFEST_TIMEOUT` seconds (default 600) it gives up; Ctrl-C stops it at once.
2. **Install the App** on `mintswarm/mintswarm` only (organisation settings, GitHub Apps, `mintswarm-dev`, Configure).
   1. **Workflows permission**, the maintainer's step on GitHub, for an App created before its manifest asked for `workflows`, such as App 5874301: in the App's settings (organisation settings, Developer settings, GitHub Apps, `mintswarm-dev`, Edit, Permissions & events, Repository permissions), set *Workflows* to *Read and write* and save. Then an owner of the `mintswarm` organisation accepts the updated permissions on the installation (organisation settings, GitHub Apps, `mintswarm-dev`, Configure: review and accept the requested permissions; owners are also emailed); until then the installation lacks the permission and minting a token that asks for it fails.
   2. Check it, without printing the token: `mint-token.sh token '{"contents":"write","workflows":"write"}' >/dev/null && echo ok`. Do this before the first push that touches `.github/workflows/`; ordinary pushes ask only for `contents: write` and do not depend on it.
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
2. `store-credentials.sh rotate ~/Downloads/mintswarm-dev.<date>.private-key.pem`. It stores the key under the same service and account in the recorded key store (or in `MSW_KEY_STORE` when set), reads it back to compare fingerprints, updates `key_fingerprint`, `key_store` and `key_confirm` in `dev-app.json`, deletes the downloaded file (`--keep` keeps it) and prints the fingerprint.
3. Mint a token with the new key: `bot-gh.sh api repos/{owner}/{repo} --jq .full_name`.
4. Only then delete the old key in the App's settings, matching it by fingerprint.

Before the first `create` or `rotate` on a machine, check once that the key store works, with a dummy item (add `MSW_KEY_STORE=<name>` to each line to check a store other than the one chosen automatically):

```
key-store.sh backend                                   # the store that will be used
printf 'abc\n' | key-store.sh put msw-selftest t
key-store.sh get msw-selftest t | cat                  # prints abc; get refuses a terminal
key-store.sh delete msw-selftest t
```

## Steward

`mintswarm-dev-steward` is the landing identity of #29: the App that merges pull requests into `master`. It is a separate App so that its key, the only one allowed to bypass the `master` ruleset, is never the runner's. No script here uses it yet; `mint-token.sh` and `bot-gh.sh` can act as it with `MSW_APP_CONFIG=~/.config/mintswarm/steward-app.json`.

| | |
| --- | --- |
| Permissions | `contents` and `pull_requests` write; `checks`, `statuses` and `metadata` read; no `issues`; no events; no webhook; private (`manifests/mintswarm-dev-steward.json`) |
| Config | `~/.config/mintswarm/steward-app.json` (`MSW_STEWARD_CONFIG`), never `dev-app.json` |
| Key | service `mintswarm-dev-steward-private-key`, the client id as account, label `mintswarm-dev-steward GitHub App private key`; in the Keychain with no trusted application, so every use asks for confirmation ([Per-use confirmation](#per-use-confirmation)) |

**Creating it**, on the maintainer's Mac, after the dummy-item check under [Per-use confirmation](#per-use-confirmation):

```
.mintswarm/dogfood/github/store-credentials.sh create .mintswarm/dogfood/github/manifests/mintswarm-dev-steward.json
```

It runs as for `mintswarm-dev` (Setup, step 1), with a browser signed in as an owner of the `mintswarm` organisation. It refuses to start when `steward-app.json` exists, when `MSW_STEWARD_CONFIG` points at a `mintswarm-dev` config, and when the key would not go to the Keychain with confirmation: on a Mac it refuses another `MSW_KEY_STORE` and `--unconfirmed-key` (on Linux, see `--unconfirmed-key` above). If GitHub answers for another App, nothing is stored. Whenever it fails after the code was used, the App exists on GitHub without a usable setup here: it prints the App's id and says to delete that App and run `create` again, and, when the key had been stored already (for example when Deny was answered at the read-back), the `key-store.sh delete` command for its item. Reading the key back after storing it asks for confirmation once: answer "Allow", never "Always Allow". Compare the printed fingerprint with the App's settings page.

**Rotation:** generate a new key in the App's settings, delete the item, then rotate:

```
key-store.sh delete mintswarm-dev-steward-private-key <client id>
MSW_APP_CONFIG=~/.config/mintswarm/steward-app.json store-credentials.sh rotate ~/Downloads/mintswarm-dev-steward.<date>.private-key.pem
```

`rotate` refuses while the item exists, and refuses any store other than the keychain for this App wherever the keychain store is available.

**The maintainer's admin steps.** They need the organisation's admin rights, so the scripts never run them:

1. **Install** `mintswarm-dev-steward` on `mintswarm/mintswarm` only: organisation settings, GitHub Apps, `mintswarm-dev-steward`, Configure, "Only select repositories".
2. **Allow it to bypass the ruleset on `master`, for pull requests only.** In the repository settings, Rules, Rulesets, the ruleset that targets `master`: under Bypass list, add `mintswarm-dev-steward` and set it to "For pull requests only". Through the API this is one entry in the ruleset's `bypass_actors`, with the steward's App id from `steward-app.json`:

   ```
   {"actor_type": "Integration", "actor_id": <app id>, "bypass_mode": "pull_request"}
   ```

3. **Never add it to `mintswarm-factory-branches`**, the ruleset on `mintswarm/**`: the steward lands pull requests and never writes factory branches.

**Kill switch:** remove the steward from the `master` ruleset's bypass list to stop it landing anything at once; uninstall the App from `mintswarm/mintswarm` as the hard stop. Neither needs its key.

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
4. **Ready for review:** this step runs whenever the PR becomes ready: right after step 3 for a PR opened without `-F draft=true`, or, for a draft, after `bot-gh.sh pr ready <number>` and `request-review.py` run again. Post one comment on the PR asking for approval, with the full URLs of the plan artifact, the `Plan assessment` artifact, the approval package comment (or the auto-approval comment), in `shadow` mode of `pipeline.plan_approval` the plan's shadow-record comment, and, in `shadow` mode of `pipeline.landing`, the landing shadow-record comment. The landing sequence that posts it once review has passed (keep the round, collect the facts, run the landing floor, the landing stage and the landing decision, post the comment) is in the docstring of `.mintswarm/dogfood/landing/decision.py`. Then assign the PR: `assign-approvers.py assign <number> pr --base <base commit>`. A draft PR is not assigned until it is marked ready.
5. **Issues, comments and artifacts:** `bot-gh.sh api …` or `bot-gh.sh issue …`. `gh` 2.83 has no `gh issue artifact`, so artifacts go through `bot-gh.sh api`.
6. **Merged or closed:** `assign-approvers.py release <number> pr`. An escalation after the last review round needs no new assignment, because the PR has been assigned since it became ready (step 4); the escalation comment gives the full URL of the last review round's artefact.
7. **`needs_input`:** post one comment on the issue with the blocking questions and the full URL of the artefact that raised them, then `assign-approvers.py assign <issue> needs_input --base <base commit>`. When the maintainer has answered and the session resumes the job, run `assign-approvers.py release <issue> needs_input`.
8. **External steps:** when the plan's footprint has a non-empty `external_steps`, the PR body links the issue with `Refs #<issue>` instead of `Closes #<issue>`, so the merge leaves the issue open and it shows under "Assigned to me". After the merge, post one comment on the issue listing the steps, with the full URLs of the merged PR and the plan artifact, then `assign-approvers.py assign <issue> external_steps --base <base commit>`. When the maintainer reports the steps done, run `assign-approvers.py release <issue> external_steps`, then `bot-gh.sh issue close <issue>`.
9. **Plan approval:** the issue is assigned when the plan is published and released when the maintainer decides, as `.mintswarm/dogfood/plan-approval/README.md` describes.
10. **Address round answering a human review:** when an address round that answers a human reviewer's `CHANGES_REQUESTED` review has been pushed with `push.sh`, re-request review from that reviewer: `request-review.py <number> --branch mintswarm/<id>-<slug>`, and check that the printed logins include them. `request-review.py` never skips a reviewer who has already reviewed. For a reviewer who is not configured, use instead:

    ```
    bot-gh.sh api --method POST repos/{owner}/{repo}/pulls/<number>/requested_reviewers -f 'reviewers[]=<login>'
    ```

    Then post one comment on the PR with the round's summary and the full URL of the address artefact, and assign the PR with `assign-approvers.py assign <number> pr --base <base commit>`, as in step 4. This step follows `CHANGES_REQUESTED` only, not a `COMMENTED` review. While a re-requested reviewer is still in the PR's `requested_reviewers`, their earlier approval or change request is not current: the PR is not merged, and the landing floor's `review-requests` rule is a hit. A first request, from step 3, is not a re-request and does not make it a hit.

11. **Decomposition:** an issue is split into children in this order.
    1. When the analysis of an implementation issue is published, run `reclassify.py split <issue> --analysis <path> --analysis-url <artifact html_url> --base <base commit>`, with the analysis as published. An exit of 1 or 2 is handled as `needs_input` (step 7). `kind: implementation` means no split: the issue goes on to plan.
    2. Update the `<!-- mintswarm: state -->` comment to the printed `state:` text, when one is printed.
    3. Dispatch decompose as a new job whose `issue.md` has the printed `kind:`. The analyse job's inputs are never edited.
    4. The comment that asks the maintainer to approve the decomposition (the decomposition approval package) starts with the printed `package:` line, when there is one.
    5. Before filing each child, run `reclassify.py check-parent <issue> --base <base commit>`. On a non-zero exit, file nothing more and stop the job for a human as in step 7: post the refusal and run `assign-approvers.py assign <issue> needs_input --base <base commit>`.

12. **`msw-state`:** whenever the session writes a new Status line to an issue's Pipeline state artifact or updates its `<!-- mintswarm: state -->` comment, it runs `msw-state.py set <issue> <value> --base <base commit>`, with the value the tables in [msw-state](#msw-state) map the state to. It does the same when a job stops for `needs_input` (step 7) or waits on external steps (step 8), both `escalated`, and runs `msw-state.py set <issue> none --base <base commit>` when a job is dropped. It also runs `set` again with the current value after every `assign-approvers.py assign` (steps 4, 7, 8 and 9) and after opening the PR (step 2): GitHub sets Todo when an issue is assigned and In review when a PR is linked, and the re-run puts the mapped status back. `msw-state.py get <issue> --base <base commit>` shows the current value and status.

A full URL is the `https://github.com/…` URL GitHub returned for the artifact or comment (its `html_url`), never `#N` or a relative link. `<base commit>` is the job's base commit, the one plan approval reads the config at; without `--base`, `assign` reads the local `origin/master`, which the session does not fetch. `release` removes every assignment the bot made on the item, whatever its reason, and leaves those a person made.

Each Bash call in the session is a fresh shell, so the scripts mint a token per call; that stays far inside the rate limits below.

## msw-state

Every open `mintswarm` issue the session works on carries one `msw-state` value, its coarse phase in the pipeline, so issues can be filtered by where they are. The session sets it with `msw-state.py set` (Session procedure, step 12), and the value changes only when the phase changes.

`tracker.state.source` in `.mintswarm/config.yaml` chooses where the value lives:

- `field`: the organisation-level single-select issue field `msw-state`, whose options are the values below, in that order. An organisation admin defines it once (it needs `admin:org`, which the bot does not have and does not get); the bot only sets values on issues. Listing the organisation's fields needs `read:org`, which the bot lacks too, so the config records the field's id (`field_id`) and `msw-state.py` sets the option by name.
- `labels`, where issue fields are unavailable (a personal account, or the feature off): one `msw-state:<value>` label (`tracker.state.label`), for example `msw-state:implementing`. `msw-state.py` creates a missing label and swaps the old one for the new in a single `PUT` of the issue's labels, so every issue it has touched carries exactly one; an issue it has never touched carries none, which reads as `none`.

### Values

| Value | When it applies |
| --- | --- |
| `none` | The default: the issue is not a mintswarm job, or its job was rejected, cancelled or dropped. |
| `queued` | The issue is picked up and waits for a runner. |
| `blocked` | The issue waits on other issues: a queued job whose dependencies are open, or a parent waiting on its children. |
| `analysing` | The analyse stage runs, or the job's worktree is being prepared for it. |
| `designing` | The design stage runs. |
| `planning` | Re-classification, decomposition, filing children, the plan stage, the plan floor, the assessment or the plan-approval decision runs. |
| `awaiting approval` | A plan, decomposition or design waits for a human's approval. |
| `implementing` | The implement stage, its commit, the gates, their summary or the push runs. |
| `in review` | The PR is under review: review and address rounds, and the landing floor, judge and decision. |
| `awaiting merge` | The PR is ready and waits to be merged, or is being merged and released. |
| `escalated` | A human is needed: a `needs_input` stop, external steps after the merge, or an escalation. |
| `done` | The issue's work is merged and closed. |

### Runner states

Each fine state of the runner (`#152` design, §6) maps to one value.

| Runner state | msw-state |
| --- | --- |
| `queued`, with the blocked flag | `blocked` |
| `queued`, without it | `queued` |
| `preparing`, `analysing` | `analysing` |
| `designing` | `designing` |
| `reclassifying`, `decomposing`, `filing_children`, `planning`, `plan_floor`, `assessing`, `deciding` | `planning` |
| `awaiting_design_approval`, `awaiting_decomposition_approval`, `awaiting_plan_approval` | `awaiting approval` |
| `awaiting_children` | `blocked` |
| `implementing`, `committing`, `gating`, `summarising_gates`, `pushing` | `implementing` |
| `reviewing`, `addressing`, `landing_floor`, `landing_judging`, `landing_deciding` | `in review` |
| `ready`, `merging`, `releasing` | `awaiting merge` |
| `awaiting_input`, `awaiting_external_steps`, `escalated` | `escalated` |
| `done` | `done` |
| `rejected`, `cancelled` | `none` |
| `paused` | the value of the state it pauses |

### Pipeline states

Each Status wording the session writes to the Pipeline state artifact or the `<!-- mintswarm: state -->` comment maps to one value.

| Pipeline state | msw-state |
| --- | --- |
| Analysing | `analysing` |
| Planning | `planning` |
| Decomposing (re-classified as plan) | `planning` |
| Awaiting plan approval | `awaiting approval` |
| Awaiting approval, for a plan, a decomposition or a design | `awaiting approval` |
| Implementing | `implementing` |
| Reviewing, round 1 (PR #M) | `in review` |
| Reviewing, round N (PR #M) | `in review` |
| Awaiting approval, PR #N | `awaiting merge` |
| Awaiting approval, for a PR | `awaiting merge` |
| Decomposed into #a, #b, … (tracked through its children) | `blocked` |
| Done | `done` |
| Done (merged and closed) | `done` |
| Stopped for `needs_input` (Session procedure, step 7) | `escalated` |
| Waiting on external steps after the merge (Session procedure, step 8) | `escalated` |

### Issue status

With `tracker.state.status: issue_status`, `msw-state.py set` also sets GitHub's issue status from the value, whatever the source. The status names are resolved to ids through the repository's `planningStatuses`; a repository without one of these names makes `set` exit 1 until `tracker.state.status` is `off`.

| msw-state | Issue status |
| --- | --- |
| `none` | left alone |
| `queued` | Todo |
| `blocked` | Todo |
| `analysing` | In progress |
| `designing` | In progress |
| `planning` | In progress |
| `awaiting approval` | In review |
| `implementing` | In progress |
| `in review` | In review |
| `awaiting merge` | In review |
| `escalated` | In review |
| `done` | Done |

GitHub's own automations move statuses too: assigning someone sets Todo, and linking a PR sets In review. `set` therefore compares and repairs the status even when the value is already current, and the session runs it after each of its assignments and after opening the PR. An assignment made by a person is repaired only at the next `set`.

## Pitfalls

- **Never push to `origin` or any SSH remote.** In this repository `origin` is `git@github.com:mintswarm/mintswarm.git`. A push to it authenticates with the maintainer's SSH key, so GitHub records the push as the maintainer's, whatever the commits say. `push.sh` always pushes to the HTTPS URL and never through a named remote.
- **Never put a token in a remote URL** (`https://x-access-token:…@github.com/…`): it ends up in `.git/config` and the transcript.
- **Never run `gh` without `GH_TOKEN`**, that is, never plain `gh`: it falls back to the maintainer's stored login. Use `bot-gh.sh`.
- **Never let `gh pr create` push the branch:** it may push through `origin`. Push with `push.sh`, open the PR with `bot-gh.sh api …/pulls`.
- **Never print a token or the key:** no bare `mint-token.sh token`, no `key-store.sh get mintswarm-dev-private-key …` (it refuses a terminal, but not a pipe into `cat`, and the session's output is a pipe), no `set -x`, no `curl -v`, no `GIT_TRACE_REDACT=0`, no `GIT_CURL_VERBOSE`. The session transcript is kept on disk.
- **Never store a token** in a git or `gh` credential store; `push.sh` uses an inline helper, which answers only for `https://github.com`, and `GIT_CONFIG_GLOBAL=/dev/null`, so `osxkeychain`, `libsecret` and `store` never see it. To check after a push: on macOS `security find-internet-password -s github.com -a x-access-token` finds nothing; on Linux with `git-credential-libsecret`, `secret-tool search protocol https server github.com user x-access-token` finds nothing; and `~/.git-credentials` has no `x-access-token` line.
- **Do not set `url.*`, `credential.*`, `http.*` or `remote.<URL>.*` in the repository's git config;** `push.sh` refuses to push while they are set. A remote section named after a URL, such as `remote.https://github.com/mintswarm/mintswarm.git.pushurl`, applies to a push to that URL and would redirect it. Independently, `push.sh` lets git use only the https transport, so no redirect can reach ssh and the maintainer's key, its credential helper gives the token only to `github.com`, and it runs no hooks, so none sees the token. `GIT_CONFIG`, `GIT_SSL_NO_VERIFY`, `GIT_SSL_CAINFO` and `GIT_SSL_CAPATH` are removed from git's environment.
- **Do not count on `~/.curlrc`:** `mint-token.sh` and `store-credentials.sh` run `curl -q`, so a curlrc with `--trace` or `-v` cannot record the JWT, the token or the private key. `GIT_AUTHOR_*` or `GIT_COMMITTER_*` in the environment override the bot identity; `push.sh` refuses the resulting commits.
- **The manifest code reaches `curl` only on stdin** (`-K -`), never in argv, a file or the output; `manifest-listener.py` logs no request, so the code is not in the transcript either. It still stays in the browser's history: it works once and expires within an hour, so this matters only when `create` did not convert it.
- **Do not rely on the token's length;** it varies (390 characters has been seen). Only the `ghs_` prefix is checked.
- **`@me` and `gh pr status` do not work** with an installation token (`GET /user` is 403). Name logins explicitly.

## Limits

- GitHub's secondary rate limits for content-creating requests: 80 per minute and 500 per hour.
- An installation token lasts one hour and reaches only `mintswarm/mintswarm`.

## Linux check

```
.mintswarm/dogfood/github/linux-check.sh
```

Builds a Debian image from `linux/Dockerfile` (`debian:trixie-slim` unless `MSW_LINUX_BASE` is set; the build needs the network) and prints the base image's digest, the commit and, from inside the container, the package versions. It then streams the tracked files under `.mintswarm/` into a container and runs `linux/check.sh` as root:

- **A:** the whole `selftest.sh` as a non-root user.
- **B:** `secret-service` against GNOME Keyring in a D-Bus session: selection, a byte-exact round trip, `rotate` then `mint-token.sh jwt`; without the bus `get` fails. Then the keyring daemon is restarted without unlocking, so the login collection loads locked, which is confirmed over D-Bus (`Locked` is true; a bounded `secret-tool lock` is the fallback). `get` of the stored key must fail within 20 seconds, however the prompt behaves, with nothing falling back to the file; a `note` line records the error. Restarted unlocked, `get` must return the key again, which shows the failure came from the lock. When no locked collection can be reached, the step says `skip` with the reason.
- **C:** `pass` with a throwaway gpg key: round trip, `rotate` then `jwt`.
- **D:** the file store: modes, the warning, refusing a symlink, a wide mode and a file owned by another user; `rotate` then `jwt`.
- **E:** `systemd-creds` as root with the host key, which is then moved aside to show that `get` fails, so the null key was not used.
- **F:** no key line in any `execve` argv traced with `strace` during B to E, every `TMPDIR` empty, and no key line in any file under `/home`, `/root`, `/tmp`, `/run`, `/dev/shm` or `/var` except the file store's own.

It prints one `ok`, `FAIL` or `skip` line per check and the totals, and exits with the container's status. `linux/check.sh` runs only in a container with the marker `/etc/mintswarm-linux-check`, which only `linux/Dockerfile` writes, and its per-user steps only as that image's `msw` user, so it never touches the keyring or files of a toolbox or distrobox that shares the host's home. The container runs with `--network none` and gets no volume, no variable, no socket and no SSH agent from the host, so the Keychain, `~/.config/mintswarm`, `~/.gnupg` and `~/.password-store` never reach it; every key it uses is generated inside it. `yq` in the image is a stand-in (`linux/yq`, on PyYAML) that accepts only `yq -o=json . FILE`, the form `request-review.py`, `assign-approvers.py` and `reclassify.py` (with `FILE` `/dev/stdin`) use. It is a recorded manual check, not a gate: run it after changing `key-store.sh` and post its output on the issue or PR. The items listed under "What has been verified where" still need a real Linux host.

## Not covered

- Verified (signed) commits: bot commits are unsigned; squash merges are signed by GitHub.
- The `master` and `mintswarm/*` rulesets: both exist on GitHub (#18) and are not managed by these scripts. Adding the steward to the `master` ruleset's bypass list, and installing it, are the maintainer's steps ([Steward](#steward)).
