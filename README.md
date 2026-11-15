# personal-todo-mcp

An open-source, multi-tenant [MCP](https://modelcontextprotocol.io) server for
personal todos, notes, and lists. Your todos are GitHub Issues in a private
repository you own. Your notes are Markdown files, and your lists are tables
with a schema, in the same repository. All of it keeps working without this
service, and you can leave at any time.

The server stores nothing. There is no database, and todo, file, and table
content only passes through on its way to your repository.

## Connect

1. [Create a private repository](https://github.com/new) for your todos.
2. Add the server URL (`https://<host>/mcp`) to your MCP client.
3. Follow the browser flow: confirm the client, sign in with GitHub, and install
   the GitHub App on your todo repository. If it is already installed, that step
   is skipped.

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
| `file_list` | List Markdown files, optionally in one folder |
| `file_read` | Read a file as numbered lines, in windows of 400 by default |
| `file_edit` | Change a file by exact string replacement |
| `file_write` | Create a file, or replace a whole one given its `sha` |
| `file_delete` | Delete a file given its `sha` |
| `table_list` | List tables |
| `table_create` | Create a table with typed columns |
| `table_describe` | Show a table's columns, row count, and `sha` |
| `table_alter` | Add or remove columns |
| `table_drop` | Delete a table given its `sha` |
| `table_query` | Read rows, filtered, sorted, and paged |
| `table_insert`, `table_update`, `table_delete` | Change up to 500 rows per call, or every row matching a filter |

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

## How files map to GitHub

Files live in the `files/` folder of your todo repository, on its default
branch. Tool paths are relative to that folder: `projects/house.md` is
`files/projects/house.md` in the repository. Every change is one commit by the
App's bot account, so the repository's history is the file history.

- Only Markdown (`.md`) files, UTF-8, up to 512 KB each.
- Paths must be relative. `..`, segments starting with a dot (such as
  `.github`), backslashes, and control characters are refused.
- `file_read` returns each file's `sha`. `file_write` needs it to replace a
  file, and `file_delete` needs it to delete one. A file changed in the
  meantime is never overwritten. `file_edit` re-reads and retries once by
  itself, because its replacements either still match or fail.

## How tables map to GitHub

A table is one file, `tables/<name>.jsonl`, on the default branch. Line 1 holds
the schema, every other line one row:

```jsonl
{"version":1,"description":"What I read","next_id":3,"columns":[{"name":"title","type":"string","required":true},{"name":"status","type":"enum","values":["unread","read"]},{"name":"finished","type":"date"}]}
{"id":1,"title":"Dune","status":"read","finished":"2026-01-02"}
{"id":2,"title":"Emma"}
```

- Names use `a-z`, `0-9`, `_`, and `-` (columns: no `-`). `id` is reserved and
  assigned by the server.
- Column types: `string`, `integer`, `number`, `boolean`, `date`
  (`YYYY-MM-DD`), `datetime` (RFC 3339), `enum` (with its values), and `list`
  (of strings). A column can be `required`.
- Every write is checked against the schema, and each call is one commit, so
  an import of 500 rows costs one request. A call applies completely or not at
  all.
- Row changes re-read the table and retry once if someone else wrote it in the
  meantime. `table_drop` needs the `sha`.
- Filters: `eq`, `ne`, `lt`, `lte`, `gt`, `gte`, `contains`, `in`, `is_null`,
  `not_null`, all of which must hold. `ne` also matches rows without a value.
- A table is at most 512 KB, a few thousand rows. Every query reads the whole
  table, so tables suit personal lists, not large data sets.
- You can edit the file on GitHub. If a line no longer fits the schema, the
  tools report that line number until it is fixed.

File and table tools need the App's **Contents** permission. If you installed
the App before files existed, GitHub asks you to approve the new permission.
Until you do, the file and table tools say so and the todo tools keep working.

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
| Repository permissions | **Contents: Read and write**, **Issues: Read and write**, **Metadata: Read-only** |
| Where can this GitHub App be installed? | **Any account** |

Then note the App ID, client ID, and the URL name (slug). Generate a client
secret and a private key.

Adding a permission to an existing App asks every installation to approve it.
Add Contents before you deploy a version with file tools. Installations that
have not yet approved it keep working for todos.

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
  uses its own installation tokens. Todo tools get one limited to Issues and
  Metadata, file and table tools one limited to Contents and Metadata.
- **Files.** Paths are checked before any request is made, so a path cannot
  leave `files/` or reach `.github/`. Writing workflow files would need the
  separate Workflows permission, which the App does not have. Logs never
  contain file content or paths.
- **Tables.** Table names are checked before any request is made, so a table
  is always a single file directly in `tables/`. Logs never contain table
  names or content.
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
