# Project instructions

## Goal

Generate simple, maintainable, idiomatic Rust. Prefer clarity over cleverness.
Avoid unnecessary abstractions, macros, generics, traits, async, or dependencies
unless they clearly simplify the code.

## Rust toolchain and style

- Use Rust 1.89.0 and do not change the repository toolchain without discussion.
- Follow `rustfmt` formatting and `clippy` guidance.
- Prefer small modules with clear responsibilities and explicit, readable code.
- Use meaningful names and keep public APIs minimal; make items private by
  default.
- Prefer composition and direct data flow over elaborate trait hierarchies.
- Avoid premature optimization.

## Architecture boundaries

- `src/app/` owns command orchestration, date/export helpers, run preparation,
  and optional report delivery coordination.
- `src/domain/` owns provider-independent data shapes, digest construction,
  workflow states, and `PreparedRun` invariants.
- `src/integrations/` owns Gmail/OAuth, classifier-provider wire formats, and
  shared HTTP retry behavior.
- `src/storage/` owns libSQL, migrations, atomic commit behavior, idempotency,
  report reads, and encrypted OAuth-token persistence.
- `src/reporting/` renders output and must not mutate workflow state.
- Keep provider response types out of the domain and storage layers.
- Preserve the prepare-then-commit boundary. CRM run/thread/transition writes
  belong in the storage transaction, not preparation.
- Keep the classification prompt external. Do not compile an operational prompt
  into the binary or commit it to the repository.

## Error handling and dependencies

- Do not use `unwrap()` or `expect()` in production code unless a documented
  invariant makes failure impossible.
- Use `Result<T, E>` for fallible operations and preserve context at boundaries.
- Prefer `thiserror` for typed errors.
- Add dependencies only when they provide clear value that the standard library
  or existing dependencies do not.
- Keep async boundaries contained inside the existing libSQL integration unless
  a concrete requirement justifies broader async use.

## Testing

- Add unit tests for nontrivial logic and integration tests for public behavior.
- Test behavior and invariants rather than implementation details.
- Keep default tests deterministic and independent of Gmail, paid classifiers,
  remote databases, and working credentials.
- Live tests must remain ignored and require explicit environment opt-in.
- Use only synthetic messages, addresses, thread IDs, exports, scenarios, and
  expected results in committed tests and fixtures.
- Do not weaken rollback, idempotency, schema-validation, or scope-boundary tests
  to make a change pass.

## Confidentiality and security

- Never add customer data, real mailbox addresses, production credentials,
  OAuth material, account/resource identifiers, or customer-specific rules.
- Never commit an operational classification prompt. Use
  `CLASSIFICATION_PROMPT_FILE` and ignored/local storage.
- Do not derive a fixture by lightly redacting real email. Construct it from
  synthetic content instead.
- Treat Gmail exports, local databases, eval artifacts, reports, and logs as
  sensitive even when Git ignores them.
- Preserve the read-only Gmail scope for the monitored mailbox. Report sending
  uses its separate authorization path.

## Documentation

- Document existing behavior rather than aspirational capabilities.
- Keep configuration tables aligned with environment variables actually read by
  the code.
- Explain why a boundary or retry choice exists; avoid comments that only restate
  code.

## Required verification

Before finishing a change, run:

```bash
cargo fmt -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```

If sandbox policy prevents loopback test servers, rerun the test command with the
minimum permission needed for localhost binding and report that constraint.
