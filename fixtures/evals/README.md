# Classifier evaluation fixtures

The `xtask` runner replays prepared Gmail exports through the product binary and
compares the resulting state counts, selected thread fields, transitions, and
prompt/tool hashes with an expected scenario.

Use synthetic data for committed fixtures. Mailbox exports and scenarios derived
from real conversations belong under `fixtures/evals/private/`, which is ignored.
Generated reports, actuals, and optional databases are written to ignored
`eval_runs/` directories.

Run a scenario with the same external prompt selected for the application:

```bash
CLASSIFICATION_PROMPT_FILE=.classification_prompt.txt \
  cargo run -p xtask -- classifier-eval \
  --scenario fixtures/evals/private/example/scenario.json
```

Scenario paths are resolved relative to the scenario file. The supported shape
is illustrated below with synthetic identifiers and dates:

```json
{
  "name": "synthetic-quotation-flow",
  "batch_size": 10,
  "dates": [
    {
      "date": "2030-01-10",
      "export": "exports/day-1.json"
    }
  ],
  "expected": {
    "prompt_hash": "optional-fnv1a-hash",
    "tool_hash": "optional-fnv1a-hash",
    "threads_seen_by_date": {
      "2030-01-10": 2
    },
    "state_counts_by_date": {
      "2030-01-10": {
        "FOLLOWUP_PENDING": 1,
        "QUOTATION_REPLY_PENDING": 1
      }
    },
    "final_threads": {
      "synthetic-thread-001": {
        "state": "FOLLOWUP_PENDING",
        "quote_sent_at": "2030-01-10"
      }
    },
    "transitions": []
  }
}
```

Expected fields are optional where the scenario does not need that assertion.
