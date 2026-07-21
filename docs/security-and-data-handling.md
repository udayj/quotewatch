# Security and data handling

QuoteWatch processes sensitive mailbox content. Its read-only Gmail scope limits
mailbox mutations, but it does not make the fetched data non-sensitive. Protect
the runtime environment, database, exports, prompts, logs, and LLM-provider
account accordingly.

## Data flow

| Data | Source | Destination | Notes |
| --- | --- | --- | --- |
| OAuth client credentials | ignored file or environment | Google OAuth endpoints | Never stored in application tables |
| Gmail OAuth tokens | encrypted DB, environment, or ignored token file | Gmail API | DB storage exists only when the encryption key is configured |
| Raw Gmail threads | Gmail or replay export | `threads.raw_thread_json` | Contains message headers, bodies, IDs, and attachment metadata |
| Thread digests | app memory | classifier provider and `threads.digest_json` | Last 14 messages; body excerpts capped at 2,000 characters; attachment names retained |
| Classifications | classifier provider | thread columns and `classification_json` | Can contain requester identity, requirements, dates, and reasoning |
| Reports | database | stdout and optional report recipients | Concise and detailed formats expose different fields |
| Eval artifacts | local fixture/export files | ignored `eval_runs/` | May reproduce sensitive source data if real exports are used |

The classifier receives digest content and metadata. Review the selected
provider's retention and training terms before sending mailbox data.

## Gmail permissions

The monitored mailbox uses only:

```text
https://www.googleapis.com/auth/gmail.readonly
```

The code lists messages and labels and fetches complete threads. It never calls
Gmail mutation endpoints for the monitored mailbox. Recognized control labels
are observed, not created or modified.

Optional report delivery uses:

```text
https://www.googleapis.com/auth/gmail.send
```

Configure a separate sender account when the monitored account must remain
strictly read-only. Supplying send authorization for the monitored account would
broaden that account's granted permissions even though read operations still use
the read-only token purpose.

## OAuth token storage

With `OAUTH_TOKEN_ENCRYPTION_KEY` set, token JSON stored in libSQL is encrypted
using ChaCha20-Poly1305 with a random nonce per write. Nonces and ciphertext are
base64 encoded for text columns. The key must decode to exactly 32 bytes and
remains outside the database.

Encryption is limited to OAuth token rows. It does not encrypt raw threads,
digests, classifications, reports, run metadata, local token files, exports, or
the database file as a whole. Back up and rotate the encryption key with care:
losing it makes stored tokens unreadable, while reusing a compromised key leaves
existing ciphertext exposed to anyone who also has the database.

When DB token storage is disabled, refreshed tokens are written to ignored local
files. Secure those files with operating-system permissions and secret-management
controls appropriate to the environment.

## Secrets and configuration

The repository ignores common OAuth files, tokens, private keys, `.env` variants,
operational prompt files, databases, Gmail exports, private eval fixtures, and
generated eval runs. Ignore rules are a guardrail, not a substitute for review.

Before sharing changes:

1. inspect the complete Git diff and tracked-file inventory;
2. search for email addresses, mailbox/thread IDs, service URLs, account/resource
   names, and credential-shaped values;
3. scan the complete Git history with a secret scanner;
4. confirm no ignored input is required by build or default tests; and
5. use only synthetic fixtures and examples.

Do not place an operational classification prompt in `assets/llm/`. Keep it at an
ignored path or outside the checkout and select it through
`CLASSIFICATION_PROMPT_FILE`.

## Database and export handling

The database is the audit record and contains substantially more data than the
concise report. Restrict access, use encrypted storage at rest where available,
and define retention/deletion procedures outside the application. QuoteWatch does
not currently implement full-database encryption or automated retention cleanup.

Gmail export JSON is a replay input and can contain complete message text and
identifiers. Keep real exports out of Git. If a regression case must be shared,
create a synthetic thread with invented `.example`/`.test` addresses and IDs;
do not merely replace a few obvious names in a real export.

## Logs and report recipients

Normal output includes reports, retry diagnostics, OAuth account context, and
report-send metadata. Treat stdout/stderr as potentially sensitive and avoid
forwarding it to broadly accessible logging systems without review.

The concise report minimizes fields but still contains requester names, dates,
subjects, and Gmail thread links. The detailed report adds requirements,
classification reasons, company-message dates, expiry, and confidence. Verify
recipient lists and report format before enabling email delivery.

## External prompt and tool schema

The operational prompt can encode confidential workflow policy and examples, so
it is loaded from an external file. The committed example uses only neutral
company/customer terminology. The structured-output tool schema is committed
because it contains field definitions and state names, not credentials or
customer identifiers.

Prompt injection remains a model-level risk: email content is untrusted input.
The example prompt tells the model to treat messages as data, and exact thread-ID
validation prevents the model from inventing additional rows, but these controls
do not prove that every classification is correct. Review provider changes and
use regression scenarios with synthetic data.
