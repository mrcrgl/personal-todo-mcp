# personal-todo-mcp

An open-source, multi-tenant [MCP](https://modelcontextprotocol.io) server for
personal todos. Your todos are GitHub Issues in a private repository you own,
so they keep working without this service, and you can leave at any time.

The server stores nothing. There is no database, and todo content only passes
through on its way to your repository.

> **Quickstart**
>
> 1. [Create a private repository](https://github.com/new) for your todos.
> 2. Add `https://personal-todo-mcp.mrcrgl.de/mcp` to Claude, Cursor, or any
>    other MCP client.
> 3. Sign in with GitHub and install the App on that repository when asked. Done.

<!-- Demo: add docs/demo.gif (Claude creating a todo, next to the issue on GitHub) here. -->

## Connect

The hosted instance runs at `https://personal-todo-mcp.mrcrgl.de/mcp`. If you
[host your own](#self-hosting), use `https://<your-host>/mcp` instead.

| Client | How to add the server |
|---|---|
| Claude (web, desktop) | *Settings → Connectors → Add custom connector*, then paste the URL |
| Claude Code | `claude mcp add --transport http todos https://personal-todo-mcp.mrcrgl.de/mcp` |
| Cursor | Add `{"mcpServers": {"todos": {"url": "https://personal-todo-mcp.mrcrgl.de/mcp"}}}` to `~/.cursor/mcp.json` |

The first time your client connects, a browser flow opens: confirm the client,
sign in with GitHub, and install the GitHub App on your todo repository. If the
App is already installed, that step is skipped.

Your client registers itself, so there is nothing to copy or configure.

The App must have access to **exactly one private repository**. A public
repository is refused because anyone could open issues on it, and your agent
would read those as your todos. Only installations on your personal account are
accepted.

To disconnect, uninstall the App in your
[GitHub settings](https://github.com/settings/installations).

## Tools

| Tool | Purpose |
|---|---|
| `todo_list` | List todos, soonest due first. Filter by state, priority, list, tag, or due date |
| `todo_get` | Read a todo with its comments |
| `todo_create`, `todo_update` | Create or change a todo |
| `todo_complete`, `todo_drop`, `todo_reopen` | Mark done, close as not planned, reopen |
| `todo_comment` | Add a comment |
| `labels_list`, `label_delete` | Show or remove lists and tags |
| `todo_repository` | Show where the todos are stored |

No tool takes an owner or repository. Both follow from your access token.

## How todos map to GitHub

| Todo | GitHub |
|---|---|
| Todo | Issue. Pull requests are ignored |
| Priority | `priority:high`, `priority:medium`, `priority:low` label |
| List | `list:<name>` label |
| Tag | Any other label |
| Due date | `Due: YYYY-MM-DD` as the last line of the issue body |
| Done | Closed as completed |
| Dropped | Closed as not planned |

You can edit todos in the GitHub UI too. Issues created through the server are
authored by the App's bot account, not by you.

## Self-hosting

### 1. Register a GitHub App

Under *Settings → Developer settings → GitHub Apps → New GitHub App*:

| Setting | Value |
|---|---|
| Callback URL | `https://<host>/oauth/github/callback` |
| Expire user authorization tokens | **On** (required) |
| Request user authorization (OAuth) during installation | **Off** |
| Setup URL | `https://<host>/oauth/github/setup` |
| Redirect on update | **On** |
| Webhook | Inactive |
| Repository permissions | **Issues: Read and write**, **Metadata: Read-only** |
| Where can this GitHub App be installed? | **Any account** |

Then note the App ID, client ID, and the URL name (slug). Generate a client
secret and a private key.

### 2. Configure

| Variable | Required | Meaning |
|---|---|---|
| `PUBLIC_BASE_URL` | yes | Public origin, e.g. `https://todo.example.com`. Must be `https` outside localhost |
| `TOKEN_KEYS` | yes | Comma-separated token keys, newest first |
| `GH_APP_ID` | yes | Numeric App ID |
| `GH_APP_SLUG` | yes | The App's URL name |
| `GH_APP_CLIENT_ID` | yes | OAuth client ID |
| `GH_APP_CLIENT_SECRET` | yes | OAuth client secret |
| `GH_APP_PRIVATE_KEY_PATH` | yes | Path to the private key `.pem` |
| `GH_API_URL` / `GH_WEB_URL` | no | Default to github.com |
| `PORT` / `RUST_LOG` | no | `10080` / `info` |

Generate a token key:

```bash
openssl rand 32 | basenc --base64url --wrap=0 | tr -d =
```

Every replica needs the same `TOKEN_KEYS`. To rotate, put a new key first and
keep the old one until refresh tokens sealed with it have expired (175 days).
Removing a key signs out everyone whose tokens it sealed, and clients registered
under it have to register again.

### 3. Run

```bash
docker build -f _build/Dockerfile -t personal-todo-mcp .
docker run -p 10080:10080 --env-file .env \
  -v "$PWD/app.pem:/keys/app.pem:ro" \
  -e GH_APP_PRIVATE_KEY_PATH=/keys/app.pem personal-todo-mcp
```

The server needs TLS in front of it, and the proxy must forward the original
`Host` header. `GET /healthz` is the probe endpoint. It keeps no state, so run as
many replicas as you like without sticky sessions.

On Kubernetes, use the chart in `_build/helm/personal-todo-mcp`. Images are
published to `ghcr.io/mrcrgl/personal-todo-mcp` as `sha-<commit>`. The chart
reads its secrets from an existing Secret, so they never pass through Helm:

```bash
kubectl create namespace personal-todo-mcp
read -rs CLIENT_SECRET
kubectl -n personal-todo-mcp create secret generic personal-todo-mcp \
  --from-literal=TOKEN_KEYS="$(openssl rand 32 | basenc --base64url --wrap=0 | tr -d =)" \
  --from-literal=GH_APP_CLIENT_SECRET="$CLIENT_SECRET" \
  --from-file=github-app.pem=app.pem

helm upgrade --install --atomic --wait -n personal-todo-mcp \
  -f my-values.yaml --set image.tag=sha-1a2b3c4 \
  personal-todo-mcp _build/helm/personal-todo-mcp
```

See `_build/helm/values.production.yaml` for an example of the values to set.

## Security model

- **Tenancy.** The tenant comes only from a validated access token and is
  required by every storage call. GitHub then enforces it again: each tenant
  uses its own installation token, limited to Issues and Metadata.
- **Tokens.** Client ids, codes, and tokens are encrypted with XChaCha20-Poly1305
  and bound to their purpose, so one kind cannot stand in for another. Access
  tokens last one hour and are bound to this server's `/mcp` resource.
- **Consent.** Registration is open, so a consent page names where tokens will go
  before anything reaches GitHub, and a browser-bound cookie stops a link from
  being completed in someone else's browser.
- **Installations.** `installation_id` from a redirect is ignored. The
  installation is looked up with the signed-in user's own token.

Because there is no server-side state:

- An authorization code can be replayed for its 60-second lifetime. PKCE still
  makes a stolen code useless without the verifier.
- A single session cannot be revoked server-side. Uninstalling the App ends access
  within 10 minutes, and revoking the App's authorization stops the next refresh,
  within an hour.
- Each refresh spends a single-use GitHub token. If its response is lost in
  transit, or two refreshes race, the client has to sign in again.

## Development

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

The tests run the real application against a fake GitHub, so they need no
credentials. See [`AGENTS.md`](AGENTS.md) for the working agreements.

## License

[MIT](LICENSE)
