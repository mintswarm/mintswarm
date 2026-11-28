# mintswarm-server

The control-plane server binary. `serve` runs the control plane, `migrate` applies its database migrations, and `webhooks` receives the GitHub App's webhooks and appends each verified delivery to a log file, so a session can watch the log instead of polling GitHub.

## Running the control plane

```sh
mintswarm-server serve --config mintswarm.yaml
```

`--config` defaults to `mintswarm.yaml` in the current directory. On start, `serve`:

1. loads the settings and refuses a `listen` address that is not loopback (see below);
2. binds `listen` and prints `mintswarm-server serve: listening on http://<addr>` on stderr;
3. takes the leader lock, a Postgres session advisory lock held on a connection of its own;
4. applies the database migrations, as `migrate` does;
5. makes the `projects` rows match `projects:` in the settings;
6. starts the readiness probe.

Only one instance leads at a time. A second `serve` against the same database is a standby: it tries for the lock every second and answers `/readyz` with `503` until the leader's database session ends, then takes over and goes through steps 4 to 6. While the database cannot be reached, `serve` keeps trying every second instead of exiting. A leader whose lock session ends or stops answering exits 1, so it must run under a supervisor that restarts it, such as compose with a restart policy; it comes back as a standby.

It runs until Ctrl-C (SIGINT) or SIGTERM, standby or leader, then stops serving, releases the lock and exits 0. It exits 1 on a settings error, a refused address, a failed migration or reconciliation, or a lost lock.

### Health

- `GET /healthz` answers `200` while the process runs.
- `GET /readyz` answers `200` when this instance holds the leader lock, the database answers and every migration is applied. Otherwise it answers `503` with a one-line body naming the failing checks, such as `not ready: leader, migrations`. The database and migration checks run every 2 seconds in the background, each bounded by 2 seconds, so the answer is at most a few seconds old and a hung database never holds a request.

### Listening only on loopback

No auth provider exists yet, so nothing could check who calls the plane. `serve` therefore refuses a `listen` address that is not loopback, such as `0.0.0.0:8080`, with `no auth provider is configured, so serve listens only on a loopback address`, and exits 1 before connecting to anything.

### The settings

```yaml
listen: 127.0.0.1:8080          # required: the address and port serve listens on
database:
  url: postgres://mintswarm@db:5432/mintswarm   # required: a Postgres connection string
  password: …                   # optional: replaces the URL's password
projects:                       # optional: the projects the plane serves
  - project_id: mintswarm       # required, unique
    repo: mintswarm/mintswarm   # required: owner/name
    host: github                # optional: github, the only host for now
    installation_id: 12345678   # required: the GitHub App installation
    weight: 1                   # optional: a positive integer, 1 by default
```

Any other key is refused. On each start, an entry without a row is added, and a row whose `repo`, `host`, `installation_id` or `weight` changed is updated. A row whose entry was removed from the file is left as it is.

These variables override keys of the file, and a key given only by its variable need not be in the file:

| Variable | Key |
| --- | --- |
| `MINTSWARM_LISTEN` | `listen` |
| `MINTSWARM_DATABASE_URL` | `database.url` |
| `MINTSWARM_DATABASE_PASSWORD` | `database.password` |

Each also has a `_FILE` form, such as `MINTSWARM_DATABASE_PASSWORD_FILE`, that names a file holding the value, for secrets mounted as files. One trailing newline is removed from the file's text. Setting both forms of one variable, an empty value, or a file that cannot be read or is empty is an error that names the variable. The database URL and password are never printed, in errors either.

## Applying the migrations

```sh
mintswarm-server migrate --config mintswarm.yaml
```

It applies the migrations built into the binary to the database of the settings, prints how many it applied, and exits 0, or 1 with the error on stderr. It uses only `database`, but the whole file must load, `listen` included, with the same overrides as `serve`. Two runs at once, or a run beside `serve`, are safe: each takes the migration lock in turn. `migrate new <name>` creates an empty migration in the source tree instead.

## Receiving webhooks

```sh
mintswarm-server webhooks --listen 127.0.0.1:8787 --log ~/.local/state/mintswarm/webhooks.jsonl
```

- It accepts `POST /github/webhook` and checks `X-Hub-Signature-256` against the webhook secret in constant time.
- A verified delivery is answered `202` at once and put on a queue as it came; only the signature check runs before the answer. A separate task builds each delivery's JSON line and drains the queue to the `--log` file, one line per delivery. The file is opened in append mode and created if absent; its directory must exist.
- A missing or bad signature gets `401` and one JSON line on stderr without the body, such as `{"received_at":"…","rejected":"bad_signature","delivery":"…","event":"ping"}`.
- A body over 25 MiB gets `413` and is not logged.
- A log write that fails happens after GitHub got its `202`, so GitHub shows the delivery as delivered. The failure is reported on stderr as one JSON line, such as `{"received_at":"…","error":"cannot append to the delivery log …: the disk is full","delivery":"…","event":"…"}`, and is not retried. Redeliver it by hand from **Recent Deliveries** (see below) after fixing the cause. If the failure left part of a line, that part stays in the log as a line that does not parse, and the next line starts on a line of its own. The same holds when a receiver was killed part-way through a line: on start, a log that does not end in a newline gets one before the next line.
- Three bounds refuse a request rather than wait or drop it silently:
  - `--max-in-flight` (default 16): the most requests whose bodies are read and checked at once. A request holds its body, up to 25 MiB, until its signature is checked, so this bounds what callers without the secret can make the receiver hold: 16 x 25 MiB = 400 MiB. GitHub sends each delivery as one request and the receiver answers as soon as the signature is checked, so GitHub's own requests rarely overlap. One request more gets `429 Too Many Requests` before its body is read.
  - `--max-queued` (default 256): the most verified deliveries waiting for the log. Only deliveries with a valid signature are queued, and the writer only falls behind while the disk stalls; 256 covers a burst such as a push that updates many pull requests at once. A verified delivery that finds the queue full gets `429`, plus a stderr line with `"rejected":"queue_full"`, and GitHub shows it as failed, to be redelivered by hand.
  - `--read-timeout` (default 10 seconds): how long a request may take, from taking its place among `--max-in-flight` to its answer, most of which is reading the body. The default is GitHub's own delivery timeout: GitHub gives up on a delivery not answered within 10 seconds and shows it as failed, so a request still arriving after that is either not from GitHub or one GitHub no longer waits for. A slower request gets `408 Request Timeout` and a stderr line with `"rejected":"read_timeout"`, and gives its place back, so callers that send headers and then stall cannot hold every place and make GitHub's deliveries get `429`.
- Every event is logged, `ping` and unknown ones included.
- On start it prints one line on stderr with the address and the log path; on a secret, log or address error it exits 1 before listening. A usage error, such as `--secret-env` with `--key-store` or a bound of 0, exits 2.
- It runs until Ctrl-C (SIGINT) or SIGTERM. It then stops accepting and waits for the requests in progress, for at most twice `--read-timeout` (20 seconds by default): one `--read-timeout` for a request head still arriving when the stop came, since the read timeout only starts once the head is complete, and one for the body read after it. A second Ctrl-C or SIGTERM ends the wait at once. Requests still open when the wait ends, such as a connection that stalled part-way through its head, are left unanswered; one of them that passes its signature check after that gets `500` and a stderr line with `"error":"the log writer has stopped"`. Either way it then drains the queue, so every delivery answered `202` is in the log, or reported on stderr if its write failed, and exits 0. Its last stderr line says how the stop ended: every request answered, the deadline passed, or a second signal came. If the log writer failed instead, for example by panicking, deliveries it still held were answered `202` but may be missing from the log, and every verified delivery after it failed got `500` and a stderr line with `"error":"the log writer has stopped"`; the stop then ends with a stderr line saying the log writer failed, and exits 1. SIGKILL skips all this: deliveries still queued are lost, and GitHub shows them as delivered.
- The log is never rotated. To start a new one, stop the receiver, move the file and start it again.

### The webhook secret

The secret is never logged. Either form works; the key store is safer, since an environment variable is visible to other processes of the same user.

- **Key store:** store it once with `.mintswarm/dogfood/github/key-store.sh`, then name the store, service and account:

  ```sh
  printf '%s' "$SECRET" | .mintswarm/dogfood/github/key-store.sh put mintswarm-webhook github
  mintswarm-server webhooks --listen 127.0.0.1:8787 --log "$LOG" \
    --key-store keychain --key-service mintswarm-webhook --key-account github
  ```

  `--key-store` takes `keychain`, `secret-service`, `pass`, `systemd-creds` or `file`, the backend `key-store.sh backend` prints, and needs `--key-service` and `--key-account`. When it is given, the variable is not read, and `--secret-env` is refused.

- **Environment variable:** export `MSW_WEBHOOK_SECRET`, or name another variable with `--secret-env`.

One trailing newline is removed from the value; an empty value is refused. Generate a secret with, for example, `openssl rand -hex 32`.

### Running it with compose

The root `compose.yaml` runs the receiver as the service `webhooks` and an ngrok agent forwarding the reserved domain to it as the service `ngrok`. Both restart unless stopped, so they come back after a crash, a Docker restart or the machine waking. The receiver publishes no port; only ngrok reaches it. Docker with Compose v2.24 or later and BuildKit is needed.

It reads three files on the host at run time; none is copied into the repository or an image:

- `~/.config/mintswarm/webhook-secret`, read as an env file: its line `mintswarm-dev-keeper-secret=<value>` sets the variable the receiver reads with `--secret-env mintswarm-dev-keeper-secret`. As with any environment variable, anyone who can run `docker inspect` on the machine can see it.
- `~/.config/mintswarm/ngrok-domain`: the reserved domain, mounted into the ngrok container.
- ngrok's own `ngrok.yml`, which holds the auth token, mounted read-only into the ngrok container. Its default path is the macOS one, `~/Library/Application Support/ngrok/ngrok.yml`; elsewhere set `NGROK_CONFIG` to its path, such as `~/.config/ngrok/ngrok.yml` on Linux.

Both the receiver and the ngrok agent run as uid 1000. Docker Desktop on macOS maps ownership; on a Linux host, `build/webhooks` must be writable and `ngrok.yml` readable by that uid.

Only one agent can hold the reserved domain, so first stop any receiver or ngrok agent already running for it. Then, from the repository root:

```sh
mkdir -p build/webhooks
docker compose up -d --build webhooks ngrok
```

The delivery log is `build/webhooks/webhooks.jsonl` on the host, mounted into the receiver. Tail it, or the services' own output, with:

```sh
tail -n 0 -F build/webhooks/webhooks.jsonl
docker compose logs -f webhooks ngrok
```

`docker compose logs ngrok` shows the tunnel online with the domain's forwarding address.

Stop them with `docker compose stop webhooks ngrok`. It sends SIGTERM, so the receiver stops as described above and drains its queue; `compose.yaml` gives it 30 seconds before a SIGKILL, more than twice `--read-timeout`. To start a new log, move the file and run `docker compose restart webhooks`.

## Forwarding to a local receiver

GitHub must reach the receiver over HTTPS. Use one of these forwarders.

- **smee.io** (no account). Open <https://smee.io/new> to get a channel URL, then run:

  ```sh
  npx smee-client --url https://smee.io/<channel> --target http://127.0.0.1:8787/github/webhook
  ```

  smee.io sees every payload, and anyone who has the channel URL can read the stream, so the channel URL is the only secret: keep it out of issues, logs and recordings. The signature still protects the receiver from forged deliveries. If every delivery through smee gets `401` while a direct `curl` with a correct signature gets `202`, the forwarder changed the body; switch to ngrok.

- **ngrok** (needs an account and its auth token). Once, `ngrok config add-authtoken <token>`; then:

  ```sh
  ngrok http 127.0.0.1:8787
  ```

  The webhook URL is the `https://…` forwarding address it prints, followed by `/github/webhook`. Without a reserved domain the address changes on every start, and the App's webhook URL must be updated. With a reserved domain, compose runs the agent and the receiver together; see [Running it with compose](#running-it-with-compose).

## Setting up the GitHub App

The webhook goes to the keeper App, the control plane's identity (`mintswarm-dev-keeper` while dogfooding). The runner App has no webhook.

In the keeper App's settings (**Settings → Developer settings → GitHub Apps → the App → General**):

1. Tick **Active** under **Webhook**.
2. Set **Webhook URL** to the forwarder's URL ending in `/github/webhook`.
3. Set **Webhook secret** to the secret the receiver reads.
4. Under **Permissions & events → Subscribe to events**, subscribe to:
   - Issues
   - Issue comment
   - Sub issues and Issue dependencies, if offered
   - Pull request
   - Pull request review
   - Pull request review comment
   - Check suite
   - Check run
   - Status
   - Push

   An event is offered only when the App has the matching read permission (Issues, Pull requests, Checks, Commit statuses, Contents). The keeper needs the Issues permission for the issue and issue comment events.

GitHub Apps always send `application/json`. A webhook whose content type is `application/x-www-form-urlencoded` is still logged, with the body as a string in `payload`, so set **Content type** to `application/json` where the setting exists.

**Advanced → Recent Deliveries** in the App's settings shows each delivery, the receiver's answer, and a **Redeliver** button; GitHub sends a `ping` when the webhook is first set.

## The log

Each line is one compact JSON object with these keys, in this order:

| Key | Value |
| --- | --- |
| `received_at` | when the receiver got it, RFC 3339 in UTC with milliseconds |
| `delivery` | the `X-GitHub-Delivery` header |
| `event` | the `X-GitHub-Event` header, such as `issue_comment` |
| `action` | `payload.action`, such as `created` |
| `installation` | `payload.installation.id` |
| `repository` | `payload.repository.full_name`, such as `owner/name` |
| `payload` | the body as parsed JSON, or as a string if it is not JSON |

A header or payload key that is absent is `null`. Each line is written whole under a lock, so readers see complete lines; a very large line may arrive in parts, which `jq` waits for. After a failed write reported on stderr, or a receiver that was killed part-way through a line, a line that does not parse may come before the next one.

Only `payload` is authenticated: the signature covers the body and nothing else. `delivery` and `event` come from the `X-GitHub-Delivery` and `X-GitHub-Event` headers, which are not signed, so a line's `event` and `delivery` may not be what GitHub sent. A valid delivery can also be replayed: anyone who has seen one, for example through the smee channel, can post it again and it is logged again as a new line. Consumers that act on lines should deduplicate on ids inside `payload`, such as `payload.comment.id`, not on `delivery`, and should read the kind of event from `payload` rather than trust `event` alone.

Follow new comments on one repository:

```sh
tail -n 0 -F "$LOG" | jq -c --unbuffered '
  select(.repository == "owner/name" and .event == "issue_comment" and .action == "created")
  | {at: .received_at, issue: .payload.issue.number, by: .payload.comment.user.login,
     body: .payload.comment.body}'
```
