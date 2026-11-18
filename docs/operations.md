# Operations

QuoteWatch is configured through `.env` or the process environment and invoked
as a CLI. It does not include a scheduler or deployment wrapper.

## Runtime prerequisites

- Rust 1.89.0, including `rustfmt` and `clippy`
- outbound HTTPS access to Gmail, Google OAuth, the selected classifier, and a
  remote libSQL endpoint when configured
- a Google OAuth client authorized for the accounts in use
- `sqlite3` only when running the ignored live integration test, whose assertions
  query the temporary database through the CLI

Malformed `.env` files are startup errors. A missing `.env` is allowed so hosted
processes can rely on environment injection.

## Configuration reference

### Gmail and Google OAuth

| Variable | Required | Meaning |
| --- | --- | --- |
| `GMAIL_ACCOUNT` | For `run` and `export-json` | Monitored mailbox; Gmail access uses `gmail.readonly` |
| `GMAIL_TOKEN_JSON` | Optional | Bootstrap/read token JSON supplied as a secret |
| `GOOGLE_OAUTH_CLIENT_JSON` | One OAuth client source | Full downloaded Google client JSON |
| `GOOGLE_OAUTH_CLIENT_ID` | With client secret | Individual OAuth client ID |
| `GOOGLE_OAUTH_CLIENT_SECRET` | With client ID | Individual OAuth client secret |
| `GOOGLE_OAUTH_AUTH_URI` | Optional | Override for the Google authorization endpoint |
| `GOOGLE_OAUTH_TOKEN_URI` | Optional | Override for the Google token endpoint |

If no OAuth client environment source is present, the CLI reads
`client_secret.json` by default or the path passed with `--client-secret`.

Token lookup order is:

1. encrypted database token storage, when `OAUTH_TOKEN_ENCRYPTION_KEY` is set;
2. the relevant token JSON environment variable;
3. the local token file; and
4. interactive OAuth on `127.0.0.1:8765`.

Interactive consent waits up to ten minutes. Access tokens with more than 60
seconds remaining are reused; otherwise a matching refresh token is used when
available.

### Classifier

| Variable | Required | Meaning |
| --- | --- | --- |
| `CLASSIFICATION_PROMPT_FILE` | For classified runs/evals | Path to a non-empty external prompt file; relative paths use the process working directory |
| `CLASSIFIER_PROVIDER` | Optional | `fireworks` (default) or `claude`; `LLM_PROVIDER` remains a compatibility alias |
| `FIREWORKS_API_KEY` | For Fireworks | Provider API key |
| `FIREWORKS_MODEL` | Optional | Fireworks model identifier; defaults to `accounts/fireworks/models/glm-5p3`, which is called with low reasoning effort because GLM 5.3 rejects requests with thinking disabled |
| `FIREWORKS_BASE_URL` | Optional | OpenAI-compatible endpoint override |
| `FIREWORKS_INPUT_USD_PER_1M` | Optional | Uncached input-token price |
| `FIREWORKS_CACHED_INPUT_USD_PER_1M` | Optional | Cached input-token price |
| `FIREWORKS_OUTPUT_USD_PER_1M` | Optional | Output-token price |
| `ANTHROPIC_API_KEY` | For Claude | Provider API key |
| `CLAUDE_MODEL` | Optional | Anthropic model identifier |
| `CLAUDE_BASE_URL` | Optional | Anthropic endpoint override |
| `CLAUDE_INPUT_USD_PER_1M` | Optional | Uncached input-token price |
| `CLAUDE_CACHE_WRITE_USD_PER_1M` | Optional | Cache-creation token price |
| `CLAUDE_CACHE_READ_USD_PER_1M` | Optional | Cache-read token price |
| `CLAUDE_OUTPUT_USD_PER_1M` | Optional | Output-token price |
| `USD_TO_INR` | Optional | Conversion rate used by the existing INR cost estimate |

The external prompt is loaded when the classifier is created. `--skip-classify`
does not construct a classifier, so it does not require prompt or provider
configuration.

### Database and token encryption

| Variable | Required | Meaning |
| --- | --- | --- |
| `DATABASE_URL` | Optional | Remote libSQL URL; an empty value selects the local `--db` path |
| `TURSO_AUTH_TOKEN` | With non-empty `DATABASE_URL` | Remote database authentication token |
| `OAUTH_TOKEN_ENCRYPTION_KEY` | Recommended for DB token storage | Base64-encoded 32-byte ChaCha20-Poly1305 key |

Generate a suitable token-encryption key with:

```bash
openssl rand -base64 32
```

Without this key, QuoteWatch does not read or write OAuth tokens in the database;
it uses secret environment values or ignored local token files instead.

### Optional report email

| Variable | Required | Meaning |
| --- | --- | --- |
| `REPORT_EMAIL_TO` | Optional | One or more recipients; empty disables sending |
| `REPORT_EMAIL_FROM` | When recipients are set | Gmail sender account with `gmail.send` authorization |
| `REPORT_EMAIL_FROM_NAME` | Optional | Display name in the From header |
| `REPORT_EMAIL_TOKEN` | Optional | Local sender-token path; defaults to `report_token.json` |
| `REPORT_EMAIL_TOKEN_JSON` | Optional | Bootstrap sender-token JSON supplied as a secret |

The sender authorization is independent of the monitored mailbox authorization.
Using a dedicated sender keeps the monitored account read-only.

## Command behavior

### Run

`cargo run -- run` resolves the default date, prepares the run, commits it, prints
the concise report, and optionally sends that report. Useful controls include:

- `--date YYYY-MM-DD` or `--date today-ist`
- `--start` and `--end` for an inclusive date range
- `--limit` for the maximum new thread IDs fetched from the Gmail date query
- `--batch-size` for classifier request size
- `--input-json` for a single-date replay without a live date-window fetch
- `--label` to distinguish run keys for intentional variants
- `--skip-classify` to commit inputs without paid classification

`--input-json` cannot be combined with a date range. `--batch-size 0` is rejected
when classification is enabled. `--skip-classify` still creates a run and stores
thread snapshots/digests; it is not a dry run.

Each run logs stage progress to stderr: the daily fetch, every classification
batch, the active-thread refresh, and the commit, each with its elapsed time.

### Report

`cargo run -- report` renders current unexpired actionable rows. `--format
detailed` adds requirement, reason, latest company message, expiry, and confidence.
`--email` sends the selected format when report-email configuration is present;
normal successful runs send the concise form automatically when configured.

### Export

`cargo run -- export-json --date YYYY-MM-DD --out PATH` fetches Gmail data and
writes a JSON snapshot without classification or CRM-state persistence. Exported
files contain mailbox data and should remain in ignored storage.

## Retry policy

| Boundary | Attempts and delay | Retried failures |
| --- | --- | --- |
| Gmail and OAuth refresh | 7 attempts; 2, 4, 8, 16, 32, 64 seconds plus up to 1 second jitter | timeouts, connection/request failures, response-body decode failures, HTTP 408/429/5xx, and HTTP 403 quota errors |
| Classifier | 5 attempts; 10, 20, 40, 40 seconds | timeouts, connection/request failures, HTTP 408/429/5xx |
| Database | 7 attempts total; 1, 2, 4, 8, 16, 30 seconds | selected libSQL connection, replication, transaction, busy/locked, network, rate-limit, and temporary failures |

Gmail quota errors (`rateLimitExceeded`, `userRateLimitExceeded`, or structured
`RATE_LIMIT_EXCEEDED`) are retried even when returned as HTTP 403; other 403
responses fail immediately. A valid `Retry-After` header can extend a wait. If it
asks for more than 64 seconds, the request fails with context instead of retrying
early. Retries repeat only the failed request and keep threads already fetched in
memory. Gmail retry logs include quota reasons and limits, but not account
identifiers or other provider content.

### Gmail request pacing

Gmail reads and token refreshes share a process-wide 1,500 ms minimum request
interval, including retries and active-thread refreshes. That allows about 40
requests per minute. Full thread reads cost 120 quota units each, so sustained
fetching uses 4,800 units per minute, leaving 20% headroom below Gmail's
6,000-unit per-user/project quota (see
[Google's quota documentation](https://developers.google.com/workspace/gmail/api/reference/quota)).
The interval is defined once in `src/integrations/http/gmail.rs`. Separate
processes do not share this timer, so avoid overlapping runs for the same account.

Classifier schema/decode failures are not transport retries. An incomplete batch
does receive a separate per-thread fallback. Report email sending is intentionally
single-attempt to avoid duplicates after an ambiguous response.

## Failure semantics

- A preparation or classification error leaves CRM run state uncommitted.
- Encrypted OAuth token rows may still change during Gmail access or refresh.
- Any statement failure during the run commit rolls back the transaction.
- A transient commit failure retries the complete transaction; `run_key` makes an
  already-committed retry a no-op.
- Report email happens after commit. Send failure returns an error but leaves the
  committed database state intact.

## Verification and evaluation

Run the repository checks exactly as CI does:

```bash
cargo fmt -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```

The ignored live test requires `sqlite3`, live Gmail and classifier credentials,
an external prompt, and a date containing at least one test thread:

```bash
RUN_LIVE_GMAIL_E2E=1 \
RUN_LIVE_GMAIL_E2E_DATE=2026-01-15 \
cargo test live_gmail_to_classification_flow -- --ignored --nocapture
```

Classifier regression scenarios run through:

```bash
CLASSIFICATION_PROMPT_FILE=.classification_prompt.txt \
cargo run -p xtask -- classifier-eval --scenario path/to/scenario.json
```

The runner records prompt/tool hashes, model, usage, cost, actual states, selected
thread fields, and transitions under `eval_runs/`.
