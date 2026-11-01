# Decisions (ADR)

This file is the active architecture decision register.
Use it only for durable rules that should shape how future software is architected, written, and reviewed.

Keep an entry only when it establishes a long-lived constraint such as:
- boundary ownership
- dependency direction
- domain / API / persistence separation
- contract and versioning rules
- runtime or operational policies
- repository-wide coding standards

Do not add issue-scoped implementation notes, refactor mechanics, file moves, test reshaping, cleanup passes, or other work-log entries unless they create a reusable architectural rule future work must follow.

If the decision would stop mattering once the current implementation is rewritten, it does not belong here.
If the entry can be summarized as "moved", "split", "extracted", or "reorganized", it probably belongs in the Issue, PR, or archive instead.
Prefer one entry per reusable rule, written as a normative statement.

Format:
- Date (YYYY-MM-DD)
- Decision
- Why
- Consequences

---

## 2026-10-30: No server-side state

- **Decision:** The server has no database. Client ids, authorization codes, and tokens are sealed values. A refresh token embeds the user's GitHub refresh token, and GitHub's single-use rotation provides reuse detection.
- **Why:** Open client registration on a public endpoint must not be able to fill a store. Holding no user data and no GitHub tokens at rest removes a whole class of breach. Any replica can serve any request.
- **Consequences:** A code can be replayed within its 60-second lifetime, which PKCE makes harmless. Single sessions cannot be revoked server-side. A lost refresh response forces a new sign-in. Do not add storage to work around these without revisiting this decision.

## 2026-10-30: The tenant comes only from the access token

- **Decision:** Every `TodoStore` call takes a `Tenant`, and a `Tenant` on the MCP path is built only by `OAuthService::authenticate`. Tool arguments never name an owner, repository, or installation. Requests run without MCP sessions.
- **Why:** Isolation has to hold by construction, not by each tool remembering a check.
- **Consequences:** Tools cannot address other repositories, even legitimately. Each request is authenticated on its own.

## 2026-10-30: One private repository on the user's own account

- **Decision:** An installation is usable only if it is on the signed-in user's personal account and grants exactly one private repository.
- **Why:** On a public repository, strangers could write text the user's agent reads as todos. On an organization, an installation token could exceed the user's own access to the repository.
- **Consequences:** Shared and team lists are out of scope. Supporting them needs a per-user permission check.

## 2026-10-30: Consent before GitHub

- **Decision:** The server shows its own consent page, bound to the browser by a `__Host-` cookie, before redirecting to GitHub.
- **Why:** Registration is open, and GitHub does not ask again once a user has authorized the App. Without this, any registered client could obtain a user's access by sending them a link.
- **Consequences:** Users see one more step. This step must not be skipped or remembered per client, since nothing is stored.
