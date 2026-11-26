# GitHub identity for the dogfooding session

While mintswarm is dogfooded, the session acting as the runner pushes, opens PRs and writes issues and artifacts as the `mintswarm-dev` GitHub App (id 5874301), whose bot user is `mintswarm-dev[bot]` (user id 359856778). These scripts are how it does that without using the maintainer's SSH key or `gh` login, without ever writing a token to disk, and without writing the App's key to disk unencrypted unless the file key store is chosen explicitly.

The App's private key lives in a key store: the macOS login Keychain, Secret Service, `pass`, `systemd-creds` or a plain file (see [Key store](#key-store)). Every store keeps it under the service `mintswarm-dev-private-key` with the App's client id as account. The public identifiers and the name of the key store in use (`key_store`) live in `~/.config/mintswarm/dev-app.json`. Installation tokens (`ghs_…`, one hour) are minted on demand and only ever held in shell variables and the environment of the process that needs them.

A second App, `mintswarm-dev-steward`, is the landing identity: it has its own key, under its own service, and its own config, `~/.config/mintswarm/steward-app.json` (see [Steward](#steward)).

## Scripts

| Script | What it does | Secrets it touches |
| --- | --- | --- |
| `common.sh` | Sourced by the others. Holds the repository, bot identity and config paths; `msw_app_profile`, the per-App table of config file, key service and key label; `msw_token`; `msw_recorded_key_store`; and `msw_git_net`, which runs git with only the repository's config and an inline credential helper fed from `$MSW_TOKEN` that answers only for `https://github.com`. | the token, from the environment |
| `key-store.sh backend\|put\|get\|delete` | Keeps a value in a key store, with the value on stdin and stdout only (see [Key store](#key-store)). `backend` prints the store `put` would use; `put --confirm` makes every later read of a Keychain item ask for confirmation, a capability no App uses (see [Per-use confirmation](#per-use-confirmation)). `get` refuses to write to a terminal; never run `get` for the App's key in the session, not even into `cat` or a pager: it prints the private key into the transcript. | the key, on stdin or stdout, to and from the store |
| `mint-token.sh` | `jwt` prints an App JWT; `token [PERMS_JSON]` prints an installation token for `mintswarm/mintswarm`, optionally with narrower permissions. Reads the key only from the key store recorded in `dev-app.json`. Used by the other scripts; do not run `token` bare in the session. | the key (key store to `openssl` on a pipe), the JWT (to `curl` on stdin), the token (stdout) |
| `push.sh BRANCH [--force-with-lease=SHA]` | Pushes a `mintswarm/*` branch to `https://github.com/mintswarm/mintswarm.git` as the bot, after checking every commit in `origin/master..BRANCH` is the bot's. | the token, unless `MSW_TOKEN` is set: minted with `contents: write`, plus `workflows: write` when `.github/workflows/` changes from the merge base with `origin/master` to `BRANCH` or, with a lease, from the lease commit to `BRANCH` (a rebase onto a master that gained a workflow change); with a lease, the lease commit must be present locally. A branch that differs from master under `.github/workflows/` asks for it on every push, for as long as it differs |
| `bot-gh.sh ARGS…` | Runs `gh` with a fresh installation token as `GH_TOKEN`, `GH_REPO=mintswarm/mintswarm` and the maintainer's tokens removed from the environment; does not run `gh` at all if minting fails. | the token, as `GH_TOKEN` |
| `land.py PR --sha SHA` | The only script that acts as `mintswarm-dev-steward`: mints one steward token from `steward-app.json`, checks the PR with it (open, not a draft, a `mintswarm/*` head in `mintswarm/mintswarm` at `SHA`, into `master`; a `gates-matrix` run by `github-actions` whose check suite is on the commit and holds at least one gate run, since the gates workflow creates the gate runs only once `gates-matrix` has completed; every check suite with check runs, every check run and every status completed with conclusion `success`, so a `skipped`, `neutral` or any other conclusion refuses too; no unresolved review thread; mergeable with no conflict; no `hold` label; an approval steer naming `SHA` that no steer withdraws: no steer, any comment, inline review comment or review by a person or a `mintswarm-dev` one carrying the steer marker, created or submitted after it, and no comment or inline review comment edited after it), reads the PR, its check runs, check suites and statuses, its review threads and its steers again right before the merge, refusing if anything checked changed, and squash-merges at `SHA`, printing the merge commit and `merged_by`. Any failed check refuses, merging nothing, exit 1. `--dry-run` only checks. Needs no GUI session. | the steward's token, as `MSW_TOKEN` in `bot-gh.sh`'s environment only |
| `request-review.py PR --branch B\|--files F… [--base B]` | Requests review through `bot-gh.sh` from a minimal covering set of the user code owners of the PR's changed paths, read from CODEOWNERS at `--base` (default `origin/master`): one owner for each approval group, as `assign-approvers.py owners` chooses them. Team code owners are left to GitHub, which requests them when the PR opens. Exits 2 when a path has no code owner or no user is left to request. `--dry-run` only prints them. | none directly |
| `assign-approvers.py assign\|release NUMBER REASON`, `assign-approvers.py owners` | Assigns an issue or PR while a human is waited on, and `release` removes only the assignments the bot made, judged from the item's events. REASON is `plan`, `needs_input`, `pr` or `external_steps`. `pr` takes `--paths P…` or `--branch B` and assigns the same minimal covering set of user code owners that `request-review.py` requests, from the first of `.github/CODEOWNERS`, `CODEOWNERS` and `docs/CODEOWNERS` at `--base` (default `origin/master`), never the worktree's or the branch's copy; the last matching rule decides each path, and team and e-mail owners are noted, not assigned. `plan`, `needs_input` and `external_steps` assign the item's owner: the issue's author, or, when a bot filed it, the author of the nearest sub-issue ancestor a person filed. `owners --base B --paths P…\|--branch B` prints the covering set and calls no GitHub API. Never assigns a `[bot]` login and posts only logins not already assigned. Exits 2 when a path has no code owner, no user code owner is left, CODEOWNERS uses syntax it does not support, or no person-filed ancestor is found. `--dry-run` only prints the logins. | none directly |
| `reclassify.py split\|check-parent` | `split ISSUE --analysis PATH --analysis-url URL` reads the `## Split` json of an implementation issue's analysis and, when it proposes a split, sets the issue's type to the plan type of `tracker.kinds` in `.mintswarm/config.yaml` at `--base` (default `origin/master`), checks that GitHub took it, posts one comment (`<!-- mintswarm: reclassified -->`) naming the finding and the analysis URL, and prints the job kind, the pipeline-state text, the comment URL and the decomposition approval package line; on an issue already of the plan type it changes no type, posts the comment only if no comment holds the marker yet (so a re-run after a failed comment finishes the job), and prints the same lines. The `## Split` heading and its json fence are found outside fenced blocks, as the plan-approval floor finds sections. `check-parent PARENT` exits 1 unless the parent's type is the plan or the design type. `split --dry-run` changes nothing. | none directly |
| `msw-state.py set\|get` | `set ISSUE VALUE` sets the issue's `msw-state` value (see [msw-state](#msw-state)) as `tracker.state` in `.mintswarm/config.yaml` at `--base` (default `origin/master`) chooses: the organisation issue field `msw-state`, found by name with its id resolved, not recorded (see [Field ids](#field-ids)), or exactly one `msw-state:<value>` label, swapped in one `PUT` of the issue's labels. It writes only when the value differs and checks GitHub's answer. Then, unless the value has no status or `tracker.state.status` is `off`, it sets the issue status the value maps to through GraphQL `setIssueStatus`, whenever the status differs, even if the value was already current. `get ISSUE` prints the value and the status. `set --dry-run` changes nothing. | none directly |
| `msw-state.py short-title read\|set\|get\|name` | The item's short name (see [Short titles](#short-titles)): `read ANALYSIS` prints the name in the analysis's `## Short title` section, exit 1 when it is missing or malformed; `set ISSUE --analysis PATH` writes that name to the `msw-short-title` field, or one `msw-short-title:<name>` label, as `tracker.short_title` at `--base` chooses, only when the issue holds none; `get ISSUE` prints the display name; `name --title TITLE [--value VALUE]` prints it offline. `set --dry-run` changes nothing. | none directly |
| `msw-state.py verdict set\|get`, `msw-state.py fields resolve` | `verdict set NUMBER plan\|decomp\|pr BAND` records an approval stage's band (see [Verdicts](#verdicts)): `plan` and `decomp` as `tracker.plan_verdict` and `tracker.decomp_verdict` at `--base` choose, the `msw-plan` or `msw-decomp` issue field or one `msw-plan:<band>` or `msw-decomp:<band>` label; `pr` exactly one `msw-verdict:<band>` label on the PR. It writes only when the band differs and never touches the issue status. `verdict get NUMBER KIND` prints the band. `fields resolve [ISSUE…]` learns the ids of the configured fields into the per-account cache (see [Field ids](#field-ids)). | none directly |
| `watch-plan-approvals.py ISSUE…\|--commands CONTEXT` | Reports plan approvals and other commands given on the tracker (`.mintswarm/dogfood/plan-approval/README.md`, "Approval from the tracker"): each unanswered `/approve-plan` comment and each hand-set `plan approved` msw-state on the issues given, one line each, `approve`, `refuse`, `hand-set` or `set-back`, with the reason for a refusal; a `react` line before each command it acts on; and a `help` line for each unanswered `/help` and a `hint` line, with a suggested command, for any other unanswered command. `--commands plan\|decomposition\|design\|none` prints the Commands section the packages and the replies end with, and makes no GitHub call. The one trusted approver of an issue is its owner, as `assign-approvers.py` finds it for `plan`, when GitHub reports write, maintain or admin access for them; the state settings are `tracker.state` from `.mintswarm/config.yaml` at `--base` (default `origin/master`). It only reads; the session acts on its lines (Session procedure, step 14). | none directly |
| `bot-identity.sh install\|status\|uninstall` | Makes every commit on a `mintswarm/*` branch authored and committed by `mintswarm-dev[bot]`, through a conditional include in the repository's shared git config. | none |
| `store-credentials.sh create\|rotate` | `create MANIFEST` runs the whole manifest flow: it writes a page that posts the manifest to GitHub, catches the redirect on `127.0.0.1` with `manifest-listener.py`, converts the code and stores the key and the identifiers in that App's own key-store item and config. `rotate PEM_PATH [--keep]` stores a new key for the App whose config `MSW_APP_CONFIG` names. | the code, from the listener to `curl` on stdin; the key, from GitHub or the downloaded PEM to `key-store.sh` on stdin |
| `manifests/*.json` | One manifest per App. `mintswarm-dev.json`: `contents`, `pull_requests`, `issues` and `workflows` write; `checks`, `statuses` and `metadata` read. `workflows` is there because agents author the CI files and GitHub refuses a whole push from an App that adds, edits or deletes a file under `.github/workflows/` without it; once it is accepted (Setup, step 2), `bot-gh.sh`'s unnarrowed tokens carry `workflows: write` too. `mintswarm-dev-steward.json`: the same without `issues` and `workflows` (it never pushes). Both private, with no events and no webhook, redirecting to `http://127.0.0.1:8765/`. | none |
| `manifest-listener.py PORT TIMEOUT_SECS PAGE` | Run by `create`. Listens on `127.0.0.1:PORT` only, accepts the first redirect whose `state` (given on stdin) matches, rejects every other request, prints the code on stdout and exits; gives up after the timeout, an absolute bound that no slow client can stretch (each connection gets at most 10 seconds). Logs no request. | the code, on stdout to `create` |
| `selftest.sh` | Offline test of all of the above against stubs of `security`, `secret-tool`, `pass`, `systemd-creds`, `uname`, `curl` and `gh` in a temporary sandbox; the file backend is tested for real inside the sandbox. Reaches no real key store and not GitHub. | throwaway keys and dummy tokens only |
| `linux-check.sh` | Runs `selftest.sh` and the real Linux backends in a throwaway Debian container (see [Linux check](#linux-check)). `linux/` holds its image and in-container script. | throwaway keys generated in the container only |

The scripts need bash (3.2 is enough), `jq`, `openssl`, `curl`, `git`, `gh` and, for `request-review.py`, `assign-approvers.py`, `reclassify.py`, `msw-state.py`, `watch-plan-approvals.py` and `land.py`, `python3`, with `yq` for `reclassify.py`, `msw-state.py` and `watch-plan-approvals.py`. `selftest.sh` also needs `shellcheck`, which it runs at severity `warning`: info and style notes differ between shellcheck versions (0.10 on Debian, 0.11 on Homebrew), so they do not fail it. Each key store needs its own tool:

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
| `keychain` | the login Keychain: a generic password with service `mintswarm-dev-private-key`, the client id as account and the label `mintswarm-dev GitHub App private key`, base64-encoded (the steward's: its own service and label) | automatically on macOS |
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

No App's key needs per-use confirmation: the steward, like `mintswarm-dev`, runs on a remote runner where nobody is at a dialogue to answer it. `create` and `rotate` store every key with plain `put`, without `--confirm`, and record `key_confirm` `none` in the App's config, on every platform and in every key store. A config without `key_confirm` means `none`; `rotate` refuses any other value, such as `keychain-acl`, storing nothing: delete the key-store item and set `key_confirm` to `none`, then rotate again.

`key-store.sh put --confirm SERVICE ACCOUNT [LABEL]` remains a capability of the keychain store that no App uses. It creates the Keychain item with no trusted application (`-T ""` on the stdin of `security -i`, and no `-U`), so every read of the value asks for confirmation in a dialogue, and a read with no GUI session, such as over SSH, fails. It refuses an item that exists already, rather than update it with an access list it cannot see; the check reads attributes only, so it never asks. Every other store refuses `put --confirm`. Answer such a dialogue with "Allow", never "Always Allow", which silently adds `security` to the item's trusted applications. Anyone who relies on it can check once, with a dummy item, that the real `security` behaves as described; `selftest.sh` checks it only against a stub:

```
printf 'abc\n' | key-store.sh put --confirm msw-selftest t
key-store.sh get msw-selftest t | cat          # asks; answer Allow; prints abc
key-store.sh get msw-selftest t | cat          # asks again; answer Deny; get fails
key-store.sh delete msw-selftest t
```

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

`mintswarm-dev-steward` is the landing identity of #29: the App that merges pull requests into `master`. It is a separate App so that its key, the only one allowed to bypass the `master` ruleset, is never the runner's. `land.py` is the only script that acts as it (Session procedure, step 13); `mint-token.sh` and `bot-gh.sh` can act as it with `MSW_APP_CONFIG=~/.config/mintswarm/steward-app.json`.

| | |
| --- | --- |
| Permissions | `contents` and `pull_requests` write; `checks`, `statuses` and `metadata` read; no `issues`; no events; no webhook; private (`manifests/mintswarm-dev-steward.json`) |
| Config | `~/.config/mintswarm/steward-app.json` (`MSW_STEWARD_CONFIG`), never `dev-app.json` |
| Key | service `mintswarm-dev-steward-private-key`, the client id as account, label `mintswarm-dev-steward GitHub App private key`; stored without per-use confirmation (`key_confirm` `none`), in the same key stores as `mintswarm-dev`'s but under its own service, and never in `mintswarm-dev`'s item or config |

**Creating it**, on the maintainer's machine:

```
.mintswarm/dogfood/github/store-credentials.sh create .mintswarm/dogfood/github/manifests/mintswarm-dev-steward.json
```

It runs as for `mintswarm-dev` (Setup, step 1), with a browser signed in as an owner of the `mintswarm` organisation, and stores the key in `MSW_KEY_STORE` when set, otherwise in the first available store. It refuses to start when `steward-app.json` exists and when `MSW_STEWARD_CONFIG` points at a `mintswarm-dev` config. If GitHub answers for another App, nothing is stored. Whenever it fails after the code was used, the App exists on GitHub without a usable setup here: it prints the App's id and says to delete that App and run `create` again, and, when the key had been stored already, the `key-store.sh delete` command for its item. Compare the printed fingerprint with the App's settings page.

**Rotation:** generate a new key in the App's settings, then rotate; the item is replaced in place:

```
MSW_APP_CONFIG=~/.config/mintswarm/steward-app.json store-credentials.sh rotate ~/Downloads/mintswarm-dev-steward.<date>.private-key.pem
```

**The maintainer's admin steps,** needed before the first landing in the session. They need the organisation's admin rights, so the scripts never run them:

1. **Create** the App, as above.
2. **Install** `mintswarm-dev-steward` on `mintswarm/mintswarm` only: organisation settings, GitHub Apps, `mintswarm-dev-steward`, Configure, "Only select repositories".
3. **Allow it to bypass the ruleset on `master`, for pull requests only.** In the repository settings, Rules, Rulesets, the ruleset that targets `master`: under Bypass list, add `mintswarm-dev-steward` and set it to "For pull requests only". Through the API this is one entry in the ruleset's `bypass_actors`, with the steward's App id from `steward-app.json`:

   ```
   {"actor_type": "Integration", "actor_id": <app id>, "bypass_mode": "pull_request"}
   ```

**Never add it to `mintswarm-factory-branches`**, the ruleset on `mintswarm/**`: the steward lands pull requests and never writes factory branches.

**Kill switch:** remove the steward from the `master` ruleset's bypass list to stop it landing anything at once: GitHub then refuses `land.py`'s merge call, so it exits 1 and merges nothing. Uninstall the App from `mintswarm/mintswarm` as the hard stop. Neither needs its key.

## Session procedure

Every push, PR, issue and artifact the session makes as the runner goes through these scripts; plain `git push` and plain `gh` are never used for factory work.

1. **Push:** `.mintswarm/dogfood/github/push.sh mintswarm/<id>-<slug>`. After a rebase, `push.sh BRANCH --force-with-lease=<sha the branch has on GitHub>`.
2. **Open the PR** through the REST API, never with `gh pr create`:

   ```
   bot-gh.sh api repos/{owner}/{repo}/pulls -f head=mintswarm/<id>-<slug> -f base=master \
     -f title='feat(<components>): …' -F body=@<file> --jq .number
   ```

   `<components>` is the issue's components, comma-separated in the issue's order with no spaces, for example `feat(domain,agents): …`.

   Add `-F draft=true` for a draft.
3. **Request review:** `request-review.py <number> --branch mintswarm/<id>-<slug> --base <base commit>`. The reviewers are the code owners it chooses: one user code owner for each approval group of the paths the PR changes, as few as cover them all, from CODEOWNERS at the base. GitHub requests team code owners itself.
4. **Ready for review:** this step runs whenever the PR becomes ready: right after step 3 for a PR opened without `-F draft=true`, or, for a draft, after `bot-gh.sh pr ready <number>` and `request-review.py` run again. Post one comment on the PR asking for approval, with the full URLs of the plan artifact, the `Plan assessment` artifact, the approval package comment (or the auto-approval comment), in `shadow` mode of `pipeline.plan_approval` the plan's shadow-record comment, and, in `shadow` mode of `pipeline.landing`, the landing shadow-record comment. The landing sequence that posts it once review has passed (keep the round, collect the facts, run the landing floor, the landing stage and the landing decision, post the comment) is in the docstring of `.mintswarm/dogfood/landing/decision.py`. Then assign the PR to the same code owners: `assign-approvers.py assign <number> pr --base <base commit> --branch mintswarm/<id>-<slug>`. A draft PR is not assigned until it is marked ready.
5. **Issues, comments and artifacts:** `bot-gh.sh api …` or `bot-gh.sh issue …`. `gh` 2.83 has no `gh issue artifact`, so artifacts go through `bot-gh.sh api`.
6. **Merged or closed:** `assign-approvers.py release <number> pr`. An escalation after the last review round needs no new assignment, because the PR has been assigned since it became ready (step 4); the escalation comment gives the full URL of the last review round's artefact.
7. **`needs_input`:** post one comment on the issue with the blocking questions and the full URL of the artefact that raised them, then `assign-approvers.py assign <issue> needs_input`, which assigns the issue's owner: its author, or the author of the nearest ancestor a person filed when a bot filed it. When the maintainer has answered and the session resumes the job, run `assign-approvers.py release <issue> needs_input`.
8. **External steps:** when the plan's footprint has a non-empty `external_steps`, the PR body links the issue with `Refs #<issue>` instead of `Closes #<issue>`, so the merge leaves the issue open and it shows under "Assigned to me". After the merge, post one comment on the issue listing the steps, with the full URLs of the merged PR and the plan artifact, then `assign-approvers.py assign <issue> external_steps`, which assigns the issue's owner, as in step 7. When the maintainer reports the steps done, run `assign-approvers.py release <issue> external_steps`, then `bot-gh.sh issue close <issue>`.
9. **Plan approval:** the issue's owner, as in step 7, is assigned when the plan is published and released when the maintainer decides, as `.mintswarm/dogfood/plan-approval/README.md` describes. The maintainer may decide in the session or on the issue; approvals on the issue are picked up as step 14 describes.
10. **Address round answering a human review:** when an address round that answers a human reviewer's `CHANGES_REQUESTED` review has been pushed with `push.sh`, re-request review from that reviewer: `request-review.py <number> --branch mintswarm/<id>-<slug> --base <base commit>`, and check that the printed logins include them. `request-review.py` never skips a reviewer who has already reviewed. For a reviewer it did not choose, such as one who is not a code owner, use instead:

    ```
    bot-gh.sh api --method POST repos/{owner}/{repo}/pulls/<number>/requested_reviewers -f 'reviewers[]=<login>'
    ```

    Then post one comment on the PR with the round's summary and the full URL of the address artefact, and assign the PR with `assign-approvers.py assign <number> pr --base <base commit> --branch mintswarm/<id>-<slug>`, as in step 4. This step follows `CHANGES_REQUESTED` only, not a `COMMENTED` review. While a re-requested reviewer is still in the PR's `requested_reviewers`, their earlier approval or change request is not current: the PR is not merged, and the landing floor's `review-requests` rule is a hit. A first request, from step 3, is not a re-request and does not make it a hit.

11. **Decomposition:** an issue is split into children in this order.
    1. When the analysis of an implementation issue is published, run `reclassify.py split <issue> --analysis <path> --analysis-url <artifact html_url> --base <base commit>`, with the analysis as published. An exit of 1 or 2 is handled as `needs_input` (step 7). `kind: implementation` means no split: the issue goes on to plan.
    2. Update the `<!-- mintswarm: state -->` comment to the printed `state:` text, when one is printed.
    3. Dispatch decompose as a new job whose `issue.md` has the printed `kind:`. The analyse job's inputs are never edited.
    4. The comment that asks the maintainer to approve the decomposition (the decomposition approval package) opens with the line `<!-- mintswarm: decomposition approval package -->`, then the printed `package:` line, when there is one, and ends with the output of `watch-plan-approvals.py --commands decomposition`. Once the decomposition's assessment is saved and published, and before the package is posted, run `msw-state.py verdict set <issue> decomp <band> --base <base commit>` with the band of the assessment's envelope (step 16).
    5. Decompositions are always approved by a human. The maintainer answers the package with a comment whose first line is the command:
       - `approve`: release the issue with `assign-approvers.py release <issue> plan` and file the children, as in the next item.
       - `changes`: release the issue, apply the comment as a steer under "Decisions and notes" in a new decompose job's `issue.md` (with the line `<!-- mintswarm: steer -->`, as in `.mintswarm/dogfood/plan-approval/README.md`), put the previous decomposition at `.mintswarm/job/inputs/artefacts/decomposition.md`, and run decompose again. The revision gets a new package, from item 4.
       - `reject`: release the issue; nothing more runs for it until the maintainer says otherwise.
    6. Before filing each child, run `reclassify.py check-parent <issue> --base <base commit>`. On a non-zero exit, file nothing more and stop the job for a human as in step 7: post the refusal and run `assign-approvers.py assign <issue> needs_input`, which assigns the issue's owner. Each implementation child is filed with one `component:<name>` label per name in its `components` list.

12. **`msw-state`:** whenever the session writes a new Status line to an issue's Pipeline state artifact or updates its `<!-- mintswarm: state -->` comment, it runs `msw-state.py set <issue> <value> --base <base commit>`, with the value the tables in [msw-state](#msw-state) map the state to. It does the same when a job stops for `needs_input` (step 7) or waits on external steps (step 8), both `escalated`, and runs `msw-state.py set <issue> none --base <base commit>` when a job is dropped. Once a plan is approved, in the session or from the tracker (step 14), it posts the approval record of step 14 and sets `plan approved` before `implementing`; without that record on the issue, `watch-plan-approvals.py` reports the value as set by hand. It sets `queued` when it picks up an issue whose job it has not yet started, and `blocked` when that job waits on dependencies that are still open. It also runs `set` again with the current value after every `assign-approvers.py assign` on the issue (steps 7, 8 and 9) and after opening the PR (step 2): GitHub sets Todo when an issue is assigned and In review when a PR is linked, and the re-run puts the mapped status back. `msw-state.py get <issue> --base <base commit>` shows the current value and status. An exit 1 that names `fields resolve` is handled as in [Field ids](#field-ids): reported to the operator, and the job goes on. A job whose base commit predates `tracker.state` or `tracker.state.options` in `.mintswarm/config.yaml` has no settings or no values there, and one whose `tracker.state.label` still holds `{value}` rather than `{option}` has settings `msw-state.py` rejects, so in each case `msw-state.py set`, `msw-state.py get`, `msw-state.py fields resolve` and `watch-plan-approvals.py` (step 14) would exit 2: for such a job, run `git fetch origin master` and pass `--base origin/master` to each instead. The analysis save step in `.mintswarm/dogfood/plan-approval/README.md` ("Sizes") sets `msw-short-title`, so no separate session step writes it.
13. **Approval in the session:** when the maintainer approves a PR in the session ("I approve PR #N"), land it with the steward. Approvals given on GitHub keep working as before, and this step is not needed for them.
    1. Post the approval steer on the PR as `mintswarm-dev`, with `bot-gh.sh api repos/{owner}/{repo}/issues/<number>/comments -F body=@<file>`. The body opens with exactly these three lines, `<sha>` being the PR's head commit when they approved it, followed by the time and the maintainer's words quoted:

       ```
       <!-- mintswarm: steer -->
       <!-- mintswarm: steer approval -->
       Approved head: <sha>
       ```

       `land.py` counts steers as `.mintswarm/dogfood/landing/collect-facts.py` counts one (any comment, inline review comment or submitted review by a person, or a `mintswarm-dev` one with a line that is exactly `<!-- mintswarm: steer -->`), so any later steer, relayed or the maintainer's own on GitHub, a "Request changes" or comment review or an inline review comment included, withdraws the approval, and so does an earlier comment or inline review comment edited after it; an edited review body is not seen, since reviews have no edit time: post the approval steer again right before running `land.py`. It reads the check runs, check suites, statuses and steers again right before the merge. The three approval lines count only at the top of a `mintswarm-dev` comment. The session writes the first line of every comment it posts itself and never starts one with agent or stage text, so no relayed output can open with these lines.
    2. Run `land.py <number> --sha <sha>`. It refuses, merging nothing, unless every check holds (Scripts); on a refusal, report the printed reasons to the maintainer.
    3. Then follow step 6, and step 8 when external steps are pending.
    4. GitHub deletes the head branch after the merge (a repository setting, `CONTRIBUTING.md`). Only if it is still there, delete it as `mintswarm-dev`, as a fallback: `bot-gh.sh api --method DELETE repos/{owner}/{repo}/git/refs/heads/mintswarm/<id>-<slug>`.
14. **Plan approval from the tracker:** every polling interval, the session's monitor runs `watch-plan-approvals.py <issues> --base <base commit>` once for each distinct base commit among the jobs it watches, with the issues of the jobs on that base, so each issue is judged by its own owner and the `tracker.state` of its own job's base and a `/approve-plan` comment or a hand-set `plan approved` is picked up within one interval. A base that predates `tracker.state.options` makes the script exit 2, as step 12 says: run it for those issues with `--base origin/master` instead, after `git fetch origin master`. The watched issues are every open issue whose `msw-state` the session has set to a value other than `none`, so a command on an issue that is not awaiting approval, at any phase, is still answered with the reason. A command on an issue outside that set (one the session has never given a value, or has set to `none`) gets no answer until the control plane's webhook handler exists. A command is the first line of a comment that holds a `/` token alone, such as `/approve-plan` or `/help`, at the start of the line and outside `>` quotes (a quote's lazy continuation included), fenced blocks and indented code; the comment's other lines are a note. The script only reads; the session acts on each line it prints:
    - `react #N comment ID`: add a +1 reaction to the comment, `bot-gh.sh api repos/{owner}/{repo}/issues/comments/ID/reactions -f content=+1`, before acting on the lines that follow for it.
    - `approve #N comment ID by LOGIN package PKG sha256 SHA` and `hand-set #N by LOGIN package PKG sha256 SHA`: post the approval record on the issue. Its first lines are `<!-- mintswarm: plan approved -->` and `<!-- mintswarm: steer -->`, then, for a command, `<!-- mintswarm: approve-plan ID -->`, followed by the approver, the time, the package URL and the plan's sha256, and the command's note quoted. Record the maintainer decision in the shadow-record comment and the plan artifact as for an approval given in the session, run `msw-state.py set <issue> "plan approved" --base <base commit>` and `assign-approvers.py release <issue> plan`, then start implement.
    - `refuse #N comment ID by LOGIN: REASON`: reply on the issue with a comment whose first line is `<!-- mintswarm: approve-plan ID -->`, giving the reason. Nothing is approved.
    - `set-back #N by LOGIN: REASON`: run `msw-state.py set <issue> "awaiting approval" --base <base commit>`, then post the reason on the issue. Nothing is approved.
    - `help #N comment ID by LOGIN commands CONTEXT`: reply on the issue with a comment whose first line is `<!-- mintswarm: command-reply ID -->`, followed by the output of `watch-plan-approvals.py --commands CONTEXT`.
    - `hint #N comment ID by LOGIN command /X suggest /Y commands CONTEXT`: the same reply, saying "did you mean `/Y`?" before the Commands section, or, when `/Y` is `none`, that `/X` is not a command. `/Y` is a command valid in CONTEXT, so it may be one without a `/`, such as `approve` for a decomposition or a design. Nothing else is done for the comment.

    The answer marker is what marks a command as handled, so each command is reported once; the `command-reply` marker does the same for a help or a hint. An approval given in the session also gets the approval record (without the answer marker) before `plan approved` is set.
15. **Commits and merges:** the session makes its own commits on a job branch, including a stage's work that the permission classifier refused to commit, with `.mintswarm/dogfood/commit.py -m <msg> -- <paths>`, never with plain `git commit`. It concludes a merge of `master` into a job branch the same way: it stages the conflict resolutions by name and runs `commit.py` without `-m`, which keeps git's merge message. The helper runs the `gates.format` formatters whose `paths` match a staged path, re-stages only the staged paths and commits them formatted; it refuses a partially staged or unmerged path and stops when a formatter fails, committing nothing. Any other file a formatter changed is put back and named on stderr.
16. **Verdicts:** the session records each approval stage's band with `msw-state.py verdict set` (see [Verdicts](#verdicts)):
    - the plan assessment: `verdict set <issue> plan <band> --base <base commit>`, after the plan-approval decision is recorded (`.mintswarm/dogfood/plan-approval/README.md`, step 10), and again after a re-assessment;
    - the decomposition assessment: `verdict set <issue> decomp <band> --base <base commit>`, as step 11, item 4 says;
    - the landing decision: `verdict set <PR> pr <band> --base <base commit>`, after the landing decision is recorded (`.mintswarm/dogfood/landing/README.md`, "Record the verdict").

    The band is the band of the usable judgement in the decision record (`plan-approval/decision.py` or `landing/decision.py`), or of the decomposition assessment's envelope; an unusable judgement writes nothing. A job whose base commit predates the verdict blocks, or whose verdict labels still hold `{value}` rather than `{option}`, gets exit 2 for `plan` and `decomp`: as in step 12, run `git fetch origin master` and pass `--base origin/master` instead. An exit 1 that names `fields resolve` is reported to the operator, who does the step in [Field ids](#field-ids); the job goes on.

A full URL is the `https://github.com/…` URL GitHub returned for the artifact or comment (its `html_url`), never `#N` or a relative link. `<base commit>` is the job's base commit, the one plan approval reads the config at; without `--base`, `assign` reads the local `origin/master`, which the session does not fetch. `release` removes every assignment the bot made on the item, whatever its reason, and leaves those a person made.

Each Bash call in the session is a fresh shell, so the scripts mint a token per call; that stays far inside the rate limits below.

## msw-state

Every open `mintswarm` issue the session works on carries one `msw-state` value, its coarse phase in the pipeline, so issues can be filtered by where they are. The session sets it with `msw-state.py set` (Session procedure, step 12), and the value changes only when the phase changes.

`tracker.state.source` in `.mintswarm/config.yaml` chooses where the value lives:

- `field`: the organisation-level single-select issue field `msw-state`, whose options are the names in `tracker.state.options`, matched by exact name, not by position. An organisation admin defines it once (it needs `admin:org`, which the bot does not have and does not get); the bot only sets values on issues. The field must hold every value in `tracker.state.options`, `plan approved` included: when a value is added there, an organisation admin adds its option to the field, an external step of the change that adds it, and until then `msw-state.py set` with that value exits 1, since GitHub does not report the value back. The config names the field (`field`, read in this mode only); `msw-state.py` resolves its id (see [Field ids](#field-ids)) and sets the option by name.
- `labels`, where issue fields are unavailable (a personal account, or the feature off): one `msw-state:<value>` label (`tracker.state.label`), for example `msw-state:implementing`; `field` may be left out. Label names are compared case-insensitively, as GitHub compares them. `msw-state.py` creates a missing label and swaps the old one for the new in a single `PUT` of the issue's labels, so every issue it has touched carries exactly one; an issue it has never touched carries none, which reads as `none`.

### Values

The values, what each one means and the issue status each one sets are `tracker.state.options` in `.mintswarm/config.yaml`, in lifecycle order, one entry per value with its meaning as a comment. `msw-state.py` reads them at `--base` and refuses any other value. `none`, `queued`, `blocked` and `done` must be listed, since the session sets them itself.

### Runner states

Each fine state of the runner ([#152](https://github.com/mintswarm/mintswarm/issues/152) design, §6) maps to one value.

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
| Awaiting plan approval, or awaiting approval for a decomposition or a design | `awaiting approval` |
| Plan approved (from the tracker or the session) | `plan approved` |
| Implementing | `implementing` |
| Reviewing, round N (PR #M) | `in review` |
| Awaiting approval, PR #N | `awaiting merge` |
| Decomposed into #a, #b, … (tracked through its children) | `blocked` |
| Done (merged and closed) | `done` |
| Stopped for `needs_input` (Session procedure, step 7) | `escalated` |
| Waiting on external steps after the merge (Session procedure, step 8) | `escalated` |

### Issue status

With `tracker.state.status: issue_status`, `msw-state.py set` also sets GitHub's issue status from the value, whatever the source: the `status` of the value's entry in `tracker.state.options`. A value without `status`, such as `none`, leaves the issue status alone. The status names are resolved to ids through the repository's `planningStatuses`; a repository without one of the configured names makes `set` exit 1 until `tracker.state.status` is `off`.

GitHub's own automations move statuses too: assigning someone sets Todo, and linking a PR sets In review. `set` therefore compares and repairs the status even when the value is already current, and the session runs it after each of its assignments and after opening the PR. An assignment made by a person is repaired only at the next `set`.

### Field ids

Configuration names issue fields and never holds their ids: an id belongs to an account, and a field that is deleted and created again gets a new one. `msw-state.py` resolves the id from the name when it writes, in this order:

1. the issue's own value for the field, from `issues/N/issue-field-values`, whose `issue_field_id` is authoritative;
2. the per-account cache, `~/.config/mintswarm/issue-fields.json` (or `MSW_FIELD_CACHE`), outside the repository, keyed by the repository owner's login and then by field name; only `msw-state.py` writes it;
3. the organisation's fields, `orgs/{owner}/issue-fields`, which the bot normally cannot read, because it lacks `read:org`.

When none yields an id, `set` exits 1 and writes nothing. The operator step, once for each account and again after a field is recreated: set each field (`msw-state`, `msw-plan`, `msw-decomp`, `msw-short-title`) to any option or, for a text field, any value on one issue in GitHub, then run `msw-state.py fields resolve <issue> --base <base commit>` (with `--base origin/master`, after `git fetch origin master`, when the base commit predates `tracker.state.options`, as Session procedure step 12 says), which learns the ids from that issue into the cache and prints one line per field, `unresolved` for a field it could not find. An issue that already holds a field resolves it on first contact. This holds for every `msw-state.py` write, `set`, `verdict set` and `short-title set` alike: an exit 1 that names `fields resolve` is reported to the operator, who does this step; the job goes on, and the session runs the write again once the step is done.

A cached id is resolved again when GitHub refuses a write over the id (HTTP 422 or 404): the entry is dropped and the organisation's fields are read, and when that is refused too, `set` exits 1 naming the operator step. Any other failure, such as a 5xx or a rate limit, exits 1 and keeps the cache. When GitHub's answer lists the written id under another name, the field was renamed: the entry is dropped and `set` exits 1; rename the field back or change `field` in the config. The control plane will later own this mapping per account and send it to runners.

## Verdicts

The approval stages' bands are recorded on the tracker, so issues and PRs can be filtered by verdict (Session procedure, step 16). The values are the five band identifiers, `blocked`, `uncertain`, `needs-review`, `likely-safe` and `safe`.

- **Plan and decomposition**, on the issue: `tracker.plan_verdict` and `tracker.decomp_verdict` choose the source, as `tracker.state` does. In `field` mode the value is the organisation single-select issue field `msw-plan` or `msw-decomp`, whose options must be named exactly as the band identifiers, in band order; otherwise `verdict set` exits 1, since GitHub does not report the value back. In `labels` mode it is exactly one `msw-plan:<band>` or `msw-decomp:<band>` label, swapped in one `PUT`. The blocks have `source`, `field` and `label`, and no `status`: a verdict never touches the issue status.
- **PR**, from the landing decision: PRs have no issue fields, so the verdict is always exactly one `msw-verdict:<band>` label, from a fixed template that is not configured. `msw-state.py` creates a missing verdict label on first use, colour `ededed`, description `mintswarm verdict`.

A re-assessment replaces the value; the same band again writes nothing.

## Short titles

Every work item has a short name of two or three words, for displays with little room, such as dependency graph nodes and status boards. `tracker.short_title` chooses where it is kept, as `tracker.state` does, with `source`, `field` and `label` and no `status`:

- `field`: the organisation free-text issue field `msw-short-title`, defined once by an organisation admin, as for `msw-state`;
- `labels`: one `msw-short-title:<name>` label; GitHub compares label names case-insensitively, so the first label created keeps its case.

The analyse stage proposes the name in its `## Short title` section, in natural case, and the analysis save step ("Sizes" in `.mintswarm/dogfood/plan-approval/README.md`) runs `short-title read` before publishing and `short-title set` straight after. `set` writes only when the field is absent or blank, or no matching label exists, so a name a person set, the literal `none` included, is never overwritten.

Field values are untrusted text, and items analysed before the field existed have none, so every display shows the name through one rule, which keeps the case of the words it is given:

- **Sanitising:** each character other than ASCII letters, digits, space and `- . / &` becomes a space, runs of spaces collapse, and the result is cut to 24 characters.
- **Fallback:** when that leaves nothing, the name comes from the title: a Conventional Commits prefix such as `feat(core):` or `fix!:` is dropped, filler words are dropped, and the first three words are kept, fewer when they exceed 24 characters, or `Untitled` when no word is left. "feat(core): Bootstrap crates/core, crates/server and crates/steward" gives `Bootstrap crates core`.

`msw-state.py` is the only home of the rule: every display calls `short-title get ISSUE` or, offline, `short-title name --title TITLE --value VALUE`, rather than reimplementing it.

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

It prints one `ok`, `FAIL` or `skip` line per check and the totals, and exits with the container's status. `linux/check.sh` runs only in a container with the marker `/etc/mintswarm-linux-check`, which only `linux/Dockerfile` writes, and its per-user steps only as that image's `msw` user, so it never touches the keyring or files of a toolbox or distrobox that shares the host's home. The container runs with `--network none` and gets no volume, no variable, no socket and no SSH agent from the host, so the Keychain, `~/.config/mintswarm`, `~/.gnupg` and `~/.password-store` never reach it; every key it uses is generated inside it. `yq` in the image is a stand-in (`linux/yq`, on PyYAML) that accepts only `yq -o=json . FILE`, the form `reclassify.py` and `msw-state.py` (with `FILE` `/dev/stdin`) use. It is a recorded manual check, not a gate: run it after changing `key-store.sh` and post its output on the issue or PR. The items listed under "What has been verified where" still need a real Linux host.

## Not covered

- Verified (signed) commits: bot commits are unsigned; squash merges are signed by GitHub.
- The `master` and `mintswarm/*` rulesets: both exist on GitHub (#18) and are not managed by these scripts. Creating the steward, installing it and adding it to the `master` ruleset's bypass list are the maintainer's steps ([Steward](#steward)).
