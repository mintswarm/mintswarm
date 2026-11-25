# mintswarm-server

The control-plane server binary. `serve` and `migrate` only parse for now; `webhooks` receives the GitHub App's webhooks and appends each verified delivery to a log file, so a session can watch the log instead of polling GitHub.

## Receiving webhooks

```sh
mintswarm-server webhooks --listen 127.0.0.1:8787 --log ~/.local/state/mintswarm/webhooks.jsonl
```

- It accepts `POST /github/webhook` and checks `X-Hub-Signature-256` against the webhook secret in constant time.
- A verified delivery gets `202` and one JSON line in the `--log` file, which is opened in append mode and created if absent; its directory must exist.
- A missing or bad signature gets `401` and one JSON line on stderr without the body, such as `{"received_at":"…","rejected":"bad_signature","delivery":"…","event":"ping"}`.
- A body over 25 MiB gets `413` and is not logged. A log write that fails gets `500`, so GitHub shows the delivery as failed and it can be redelivered; if the failure left part of a line, that part stays in the log as a line that does not parse.
- At most eight requests are handled at once; one more gets `503` before its body is read, so callers without the secret cannot make the receiver hold more than eight bodies.
- Every event is logged, `ping` and unknown ones included.
- On start it prints one line on stderr with the address and the log path. It runs until stopped; on a secret, log or address error it exits 1 before listening.
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

  The webhook URL is the `https://…` forwarding address it prints, followed by `/github/webhook`. Without a reserved domain the address changes on every start, and the App's webhook URL must be updated.

## Setting up the GitHub App

In the App's settings (**Settings → Developer settings → GitHub Apps → the App → General**):

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

   An event is offered only when the App has the matching read permission (Issues, Pull requests, Checks, Commit statuses).

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

A header or payload key that is absent is `null`. Each line is written whole under a lock, so readers see complete lines; a very large line may arrive in parts, which `jq` waits for. After a `500`, a line that does not parse may come before the next one.

Only `payload` is authenticated: the signature covers the body and nothing else. `delivery` and `event` come from the `X-GitHub-Delivery` and `X-GitHub-Event` headers, which are not signed, so a line's `event` and `delivery` may not be what GitHub sent. A valid delivery can also be replayed: anyone who has seen one, for example through the smee channel, can post it again and it is logged again as a new line. Consumers that act on lines should deduplicate on ids inside `payload`, such as `payload.comment.id`, not on `delivery`, and should read the kind of event from `payload` rather than trust `event` alone.

Follow new comments on one repository:

```sh
tail -n 0 -F "$LOG" | jq -c --unbuffered '
  select(.repository == "owner/name" and .event == "issue_comment" and .action == "created")
  | {at: .received_at, issue: .payload.issue.number, by: .payload.comment.user.login,
     body: .payload.comment.body}'
```
