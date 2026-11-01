# Project Rules (Repo‑specific)

This is the *only* place for repo‑specific rules; everything else should remain general.

---

## Overview
- **Purpose:** Public, multi-tenant MCP server that manages personal todos as GitHub Issues in a private repository each user owns.
- **Primary constraints:** Strict tenant isolation. No server-side state. Onboarding needs nothing but the server URL.
- **Explicit non‑goals:** Storing todo content, team or organization use, webhooks, reminders, a web UI beyond the authorization pages.

## Architecture summary
- **High‑level modules/layers:**
  - `domain`: todo model and invariants. Pure.
  - `todos`: use cases (`TodoService`) and the `TodoStore` port.
  - `oauth`: stateless authorization server and the `IdentityProvider` port.
  - `mcp`: tool schemas and mapping.
  - `github`: adapter implementing both ports.
  - `app`: composition root.
- **Dependency direction:** `mcp`, `oauth` → `todos` → `domain`; `github` → ports.
- **Forbidden dependencies:**
  - `domain` must not depend on anything else in this crate, on HTTP, or on GitHub.
  - `todos` and `oauth` must not depend on `github`. Only `app` wires it in.
  - `mcp` tools must not call GitHub or read tokens.

## Technology stack
- **Languages:** Rust
- **Frameworks/libraries:** `rmcp`, `axum`, `reqwest`, `chacha20poly1305`
- **Database/storage:** None. See `.agent/CONTEXT/DECISIONS.md`. The SQLx rules in `LANGUAGE_RULES.md` do not apply.
- **Observability:** `tracing` with structured fields

## Model boundaries & mapping
- **Domain models live in:** `src/domain/`
- **API DTOs live in:** `src/mcp/dto.rs`, `src/oauth/dto.rs`
- **Persistence models live in:** `src/github/models.rs`
- **Mapping code lives in:** `src/github/mapping.rs`, `src/mcp/dto.rs`
- **Mapping tests live in:** next to the mapping code

## Error handling conventions
- **Error types:** `thiserror` enums per boundary: `StoreError`, `IdentityError`, `TokenError`, `FlowError`.
- **User‑facing error mapping:** Tool failures are tool results (`isError`), so the model can act on the message. Protocol errors are for server faults.

## Testing & quality gates
- **Required test types:** Unit tests for domain, mapping, and protocol rules. Integration tests in `tests/` run the real app against a fake GitHub.
- **Tenant isolation:** New tools need a test proving they stay inside the tenant.
- **Security controls:** A test must fail when a control is removed. Check this by disabling the control.
- **Mandatory commands:** See `.agent/DOD.md`.

## Security & compliance
- **Secrets management:** Environment and mounted files only. `Config` and anything holding a token must not implement `Debug`.
- **Tenant source:** A `Tenant` is built only from a validated token. Tool arguments never name an owner or repository.
- **Logging:** Never log tokens, codes, sealed values, query strings, or todo content.
- **New sealed values:** Add a `Purpose`. Never reuse one across meanings.
- **PII:** Only GitHub user id and login, inside sealed tokens and logs.

## Workflow specifics
- **Default branch:** `main`
- **Merge strategy:** Squash

## Documentation expectations
- **Docs that must be updated for changes:** `README.md` for tools, configuration, or the GitHub App setup.
- **ADR/decision log location:** `.agent/CONTEXT/DECISIONS.md`
- **What belongs in the decision log:** stable layer ownership, domain/API/persistence boundary rules, contract versioning rules, and cross-cutting runtime policies.
- **What does not belong in the decision log:** issue-slice refactors, extracted services, renamed files, file splits, test-module reorganizations, or cleanup passes unless they create a reusable repo-wide rule.
- **Where implementation history goes:** issue and PR updates by default.

## Language‑specific conventions
### Rust
- **Async traits:** Ports return `BoxFut`, because they are used as trait objects.
- **Lint configuration:** `cargo clippy --all-targets --all-features -- -D warnings`

---

## Repo quickstart
- **Test:** `cargo test`. No credentials needed.
- **Run:** See the configuration table in `README.md`.
- **Common pitfalls:** rmcp rejects unknown `Host` headers with 403, so `PUBLIC_BASE_URL` must match the public host.
