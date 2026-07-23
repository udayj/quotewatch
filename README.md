# QuoteWatch

QuoteWatch is a Rust service that monitors quotation threads in Gmail and
produces an actionable follow-up report. It is intentionally narrow: it turns
mailbox activity into two work queues—quotation replies still owed and sent
quotations that need follow-up—without requiring a parallel CRM data-entry flow.

The monitored mailbox uses Gmail's read-only scope. Optional report delivery
uses a separate Gmail send authorization, so report sending does not require
write access to the monitored mailbox.

## System boundaries

QuoteWatch has five main boundaries:

- Gmail OAuth and API clients fetch thread data and known workflow labels.
- Digest construction limits and normalizes the message context sent to a
  classifier.
- A Fireworks or Anthropic client combines an external prompt with the committed
  structured-output tool schema.
- The app/domain layer prepares and validates a complete run in memory.
- The storage layer atomically commits runs, thread snapshots, classifications,
  transitions, usage, and costs to a local or remote database through libSQL.

The repository map and run sequence are in
[docs/architecture.md](docs/architecture.md). Operational configuration and
failure behavior are in [docs/operations.md](docs/operations.md). Data-flow and
credential guidance are in
[docs/security-and-data-handling.md](docs/security-and-data-handling.md).

## Gmail access

The monitored account requests only:

```text
https://www.googleapis.com/auth/gmail.readonly
```

QuoteWatch does not create drafts, modify labels, archive or delete messages, or
change read/unread state. It does read the exact labels `Ignore`,
`Order Received`, and `Order Closed` as deterministic signals that a thread is no
longer actionable.

Optional report delivery requests this scope for the configured sender account:

```text
https://www.googleapis.com/auth/gmail.send
```

## Classification workflow

For each as-of date, QuoteWatch fetches matching threads and refreshes previously
active threads. It constructs chronological digests, keeping the most recent 14
eligible messages, limiting each body excerpt to 2,000 characters, and retaining
attachment names. Control-labeled threads bypass the LLM and become `IGNORED`.

Remaining digests are classified in batches into:

- `QUOTATION_REPLY_PENDING`
- `FOLLOWUP_PENDING`
- `IGNORED`

The classifier must return exactly one known thread ID per input thread. Invalid
or incomplete batches are rejected; incomplete multi-thread batches are retried
one thread at a time. Narrow deterministic guards then exclude clear procurement
and completed order/payment flows.

The classification prompt is not compiled into the binary. Set
`CLASSIFICATION_PROMPT_FILE` to an external file. The committed
[`classification_prompt.example.txt`](assets/llm/classification_prompt.example.txt)
is a neutral starting point, not an operational policy.

## Prepare, then commit

A run is assembled as a validated `PreparedRun` before CRM run state is written.
Preparation may read existing thread state to refresh active work and calculate
transitions. Gmail token refresh may also update encrypted token storage when it
is enabled. Runs, thread inputs, classifications, transitions, and usage are not
written until `Database::commit_prepared_run`.

The commit executes in one database transaction. A deterministic `run_key`
combines account, as-of date, optional label, and model. Retrying a successful or
ambiguous commit is idempotent: an existing run key turns the write into a
no-op rather than duplicating runs or transitions. Each date in a requested range
is prepared and committed separately.

## Local setup

Requirements:

- Rust 1.89.0 with `rustfmt` and `clippy`
- a Google OAuth client for Gmail
- a Fireworks or Anthropic API key when classification is enabled

Create local configuration and an ignored operational prompt:

```bash
cp .env.example .env
cp assets/llm/classification_prompt.example.txt .classification_prompt.txt
```

Edit `.env` to replace placeholder values. At minimum, a classified Gmail run
needs `GMAIL_ACCOUNT`, a Google OAuth client source,
`CLASSIFICATION_PROMPT_FILE`, the selected classifier provider, and that
provider's API key. A downloaded Google OAuth client can remain in the default
ignored `client_secret.json` path.

Build and run the checks:

```bash
cargo build --all-features
cargo fmt -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```

Run for the resolved current date in India Standard Time:

```bash
cargo run -- run --date today-ist
```

Run an explicit date and limit the number of fetched threads:

```bash
cargo run -- run --date 2026-01-15 --limit 50
```

The default local database is `quotewatch.sqlite`. Set `DATABASE_URL` and
`TURSO_AUTH_TOKEN` to use a remote libSQL database instead.

## Commands

```bash
# Process the default date and print the concise report
cargo run -- run

# Process a date range, committing once per day
cargo run -- run --start 2026-01-12 --end 2026-01-15

# Replay a prepared Gmail export
cargo run -- run --date 2026-01-15 --input-json exports/gmail.json

# Import/digest without paid classification (this still commits data)
cargo run -- run --date 2026-01-15 --input-json exports/gmail.json --skip-classify

# Print current actionable state
cargo run -- report
cargo run -- report --format detailed

# Export Gmail JSON without classifying or committing it
cargo run -- export-json --date 2026-01-15 --limit 50 --out exports/gmail.json
```

Leaving `REPORT_EMAIL_TO` empty keeps report output local. When configured,
successful `run` commands send the concise report after the database commit.

## Tests and evaluations

The default suite covers date handling, digest limits, prepared-run invariants,
classifier response validation and deterministic guards, Gmail parsing, retry
policy, report rendering, token encryption, rollback, and idempotency. No network
or external service is required for the default test run.

The ignored live test exercises Gmail, a paid classifier, a temporary local
database, persistence, and report reads. It requires an explicit opt-in date and
uses the same external prompt configuration:

```bash
RUN_LIVE_GMAIL_E2E=1 \
RUN_LIVE_GMAIL_E2E_DATE=2026-01-15 \
cargo test live_gmail_to_classification_flow -- --ignored --nocapture
```

Classifier regression scenarios are supported through `xtask`; see
[`fixtures/evals/README.md`](fixtures/evals/README.md). Use synthetic data for
anything committed to the repository.

## Retry and failure behavior

Gmail reads, OAuth refreshes, and classifier requests retry transient connection
failures, timeouts, HTTP 408/429 responses, and server errors with bounded
exponential backoff. Transient database operations use six retries. The entire
commit transaction is retried so `run_key` can resolve ambiguous remote failures.

Report email sending is deliberately not retried automatically: a successful
send followed by a lost response could otherwise duplicate the message. A report
send failure occurs after the database commit and does not roll back CRM state.

## Security and data handling

When `OAUTH_TOKEN_ENCRYPTION_KEY` is set, OAuth token JSON stored in libSQL is
encrypted with ChaCha20-Poly1305. The database itself still contains sensitive
mailbox snapshots, digests, and classifications and must be protected as such.
Production configuration and customer data are external to this repository.

## Known limitations

- QuoteWatch is a CLI process, not a scheduler or long-running daemon.
- Classification requires an external LLM provider and an operator-maintained
  prompt; model output can change across providers or model versions.
- Message direction is inferred from configured account/domain headers and can
  be imperfect for unusual aliases or forwarding arrangements.
- Digest as-of cutoffs and `today-ist` use India Standard Time.
- The three workflow-control label names are fixed in code.
- OAuth-token encryption covers token rows only, not the complete application
  database or exported Gmail JSON.
- `--skip-classify` is an import mode, not a dry run.
- There is no built-in deployment workflow or full-database retention policy.

## License

QuoteWatch is licensed under the [MIT License](LICENSE).
