<!-- BEGIN repokit profile: agent -->
# Language Rules (General)

These are cross-project language rules that apply when no language-specific section overrides them.

## General engineering rules
- Prefer explicit, readable code over cleverness.
- Keep functions small and composable with clear inputs/outputs.
- Separate domain, API/transport, and persistence models.
- Use explicit mapping at boundaries; no implicit conversions.
- Keep dependency direction one-way and explicit.
- Validate inputs at the boundary; preserve invariants in constructors.
- Errors should be explicit and contextual; avoid silent failures.
- Tests must be deterministic and isolated.

## Data & persistence
- Use parameterized queries (never string interpolation for SQL).
- Migrations should be additive and reversible where feasible.
- Keep persistence details in adapters, not in domain logic.

## Observability
- Use structured logs with stable fields.
- Correlate logs/traces per request or job.
- Emit metrics/traces for critical paths.

## Documentation
- Document non-trivial decisions.
- Update docs when behavior or architecture changes.

## Security
- Never commit secrets.
- Apply least-privilege access.
- Sanitize inputs to prevent injection risks.

## API & contracts
- Define API contracts explicitly and keep them versioned.
- Validation and error mapping must be explicit.

## Testing expectations
- Unit tests for domain logic and mapping.
- Integration tests for adapters and IO boundaries.
- Avoid flakiness; tests should be repeatable.
<!-- END repokit profile: agent -->

<!-- BEGIN repokit profile: rust -->
# Language Rules (Rust)

These rules extend the general language rules and apply to Rust repositories.

## SQLx & Postgres
- All SQL queries must use `sqlx` compile-time checked macros (`query!`, `query_as!`, etc.).
- Prefer `sqlx::test` for database-backed tests.
- Avoid dynamic SQL string building; keep queries parameterized and explicit.
- Do not use SQL stored procedures or SQL enums.
- Store complex fields as `JSONB` and serialize/deserialize in the persistence adapter when converting to/from `*TableRow` models.

## Persistence & repositories
- Persistence adapters (SQL, etc.) must follow the repository pattern with a repository trait and separate read/write traits.
- Repositories always return domain models; internally they must use private `*TableRow` models for persistence conversions. `*TableRow` models represent the data as it is saved.
- Each repository defines its own custom error type.
- Use custom types for `Error` and `BoxFut` for readability.

## Error handling
- Prefer custom error types with `thiserror`.
- Avoid `anyhow` in library code; reserve for binaries where appropriate.
- IO boundaries must map domain/business errors into standardized user-facing errors.

## Enums & string representations
- Prefer using enums instead of strings where possible.
- If a string representation is needed, enums may implement `FromStr` and `AsRef<str>`.

## Architecture & patterns
- Prefer Service and Manager patterns with traits to improve testability.
- No direct mutable repository calls in HTTP handlers; use `*Service` implementations instead.
- Avoid the `async_trait` polyfill; prefer native async in traits or alternative patterns.
- Use pure functions in private modules, but avoid across domain boundaries.
- Keep domain logic independent of transport and persistence concerns.
- `mod.rs` is for documentation and `mod` declarations/visibility only; do not place code, traits, or structs in `mod.rs`.

## Runtime & composition
- Use the `processmanager` crate for runnable services.
- Use `instancebuilder` to construct service instances cleanly.

## API contracts
- REST APIs must have an OpenAPI spec.
- API validation and error mapping must be explicit.

## Testing
- Include unit tests for domain logic and mappers.
- Add integration tests for IO boundaries and adapters.
- Keep tests deterministic (no reliance on global state or timing).
<!-- END repokit profile: rust -->
