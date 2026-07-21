use crate::Result;
use crate::eval_db;
use crate::types::{DateActual, EvalArtifact, Expected, Scenario};
use chrono::Utc;
use clap::Parser;
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Parser)]
pub(crate) struct Args {
    #[arg(long)]
    scenario: PathBuf,

    #[arg(long, default_value = "eval_runs")]
    out_dir: PathBuf,

    #[arg(long)]
    keep_db: bool,
}

// Runs the classifier benchmark scenario and writes a human report plus machine-readable actuals.
pub(crate) fn run(args: Args) -> Result<()> {
    let scenario_path = args.scenario.canonicalize()?;
    let scenario_dir = scenario_path
        .parent()
        .ok_or("scenario path should have a parent directory")?;
    let scenario: Scenario = serde_json::from_slice(&fs::read(&scenario_path)?)?;
    let db_path = temp_db_path(&scenario.name);
    let batch_size = scenario.batch_size.unwrap_or(10).to_string();
    let prompt_path = classification_prompt_path()?;

    let mut actual_dates = BTreeMap::new();
    for date in &scenario.dates {
        let export_path = resolve_path(scenario_dir, &date.export);
        run_quotewatch(&db_path, &date.date, &export_path, &batch_size)?;
        actual_dates.insert(
            date.date.clone(),
            DateActual {
                threads_seen: eval_db::threads_seen_for_date(&db_path, &date.date)?,
                state_counts: eval_db::state_counts(&db_path)?,
            },
        );
    }

    let artifact = EvalArtifact {
        scenario: scenario.name.clone(),
        generated_at: Utc::now().to_rfc3339(),
        model: eval_db::latest_model(&db_path)?,
        prompt_hash: file_hash(&prompt_path)?,
        tool_hash: file_hash(&workspace_root().join("assets/llm/classification_tool.json"))?,
        totals: eval_db::usage_totals(&db_path)?,
        dates: actual_dates,
        final_threads: eval_db::final_threads(&db_path)?,
        transitions: eval_db::transitions(&db_path)?,
    };

    let failures = compare(&scenario.expected, &artifact);
    let run_dir = args.out_dir.join(format!(
        "{}-{}",
        sanitize_name(&scenario.name),
        Utc::now().format("%Y%m%dT%H%M%SZ")
    ));
    fs::create_dir_all(&run_dir)?;
    fs::write(
        run_dir.join("actual.json"),
        serde_json::to_string_pretty(&artifact)?,
    )?;
    fs::write(
        run_dir.join("report.md"),
        render_report(&artifact, &failures, &db_path),
    )?;

    if args.keep_db {
        fs::copy(&db_path, run_dir.join("eval.sqlite"))?;
    }
    let _ = fs::remove_file(&db_path);

    println!("Wrote {}", run_dir.join("report.md").display());
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!("classifier eval found {} mismatch(es)", failures.len()).into())
    }
}

// Replays one prepared Gmail export through the real product binary.
fn run_quotewatch(db_path: &Path, date: &str, export_path: &Path, batch_size: &str) -> Result<()> {
    let status = Command::new("cargo")
        .current_dir(workspace_root())
        .env("DATABASE_URL", "")
        .env("REPORT_EMAIL_TO", "")
        .args([
            "run",
            "--quiet",
            "-p",
            "quotewatch",
            "--",
            "--db",
            db_path.to_str().ok_or("db path should be UTF-8")?,
            "run",
            "--date",
            date,
            "--input-json",
            export_path.to_str().ok_or("export path should be UTF-8")?,
            "--batch-size",
            batch_size,
        ])
        .status()?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("QuoteWatch eval run failed for {date}").into())
    }
}

// Compares the expected benchmark outcome with the actual artifact collected from the temp DB.
fn compare(expected: &Expected, actual: &EvalArtifact) -> Vec<String> {
    let mut failures = Vec::new();

    compare_field(
        &mut failures,
        "scenario",
        "prompt_hash",
        expected.prompt_hash.as_deref(),
        Some(actual.prompt_hash.as_str()),
    );
    compare_field(
        &mut failures,
        "scenario",
        "tool_hash",
        expected.tool_hash.as_deref(),
        Some(actual.tool_hash.as_str()),
    );

    for (date, expected_count) in &expected.threads_seen_by_date {
        let actual_count = actual
            .dates
            .get(date)
            .map(|date| date.threads_seen)
            .unwrap_or_default();
        if actual_count != *expected_count {
            failures.push(format!(
                "{date}: expected {expected_count} threads_seen, got {actual_count}"
            ));
        }
    }

    for (date, expected_counts) in &expected.state_counts_by_date {
        let actual_counts = actual
            .dates
            .get(date)
            .map(|date| &date.state_counts)
            .cloned()
            .unwrap_or_default();
        compare_state_counts(&mut failures, date, expected_counts, &actual_counts);
    }

    for (thread_id, expected_thread) in &expected.final_threads {
        let Some(actual_thread) = actual.final_threads.get(thread_id) else {
            failures.push(format!("{thread_id}: missing final thread"));
            continue;
        };
        compare_field(
            &mut failures,
            thread_id,
            "state",
            expected_thread.state.as_deref(),
            Some(actual_thread.state.as_str()),
        );
        compare_field(
            &mut failures,
            thread_id,
            "quote_sent_at",
            expected_thread.quote_sent_at.as_deref(),
            actual_thread.quote_sent_at.as_deref(),
        );
        compare_field(
            &mut failures,
            thread_id,
            "latest_customer_message_at",
            expected_thread.latest_customer_message_at.as_deref(),
            actual_thread.latest_customer_message_at.as_deref(),
        );
        compare_field(
            &mut failures,
            thread_id,
            "latest_company_message_at",
            expected_thread.latest_company_message_at.as_deref(),
            actual_thread.latest_company_message_at.as_deref(),
        );
    }

    let actual_transitions = actual.transitions.iter().cloned().collect::<BTreeSet<_>>();
    for expected_transition in &expected.transitions {
        if !actual_transitions.contains(expected_transition) {
            failures.push(format!("missing transition {expected_transition:?}"));
        }
    }

    failures
}

// Reports each mismatched state bucket separately so failures identify the exact regression.
fn compare_state_counts(
    failures: &mut Vec<String>,
    date: &str,
    expected_counts: &BTreeMap<String, usize>,
    actual_counts: &BTreeMap<String, usize>,
) {
    let states = expected_counts
        .keys()
        .chain(actual_counts.keys())
        .collect::<BTreeSet<_>>();

    for state in states {
        let expected = expected_counts.get(state).copied().unwrap_or_default();
        let actual = actual_counts.get(state).copied().unwrap_or_default();
        if expected != actual {
            failures.push(format!(
                "{date}: expected `{state}` count {expected}, got {actual}"
            ));
        }
    }
}

// Compares optional thread-level fields only when the scenario chooses to assert them.
fn compare_field(
    failures: &mut Vec<String>,
    thread_id: &str,
    field: &str,
    expected: Option<&str>,
    actual: Option<&str>,
) {
    if let Some(expected) = expected
        && actual != Some(expected)
    {
        failures.push(format!(
            "{thread_id}: expected {field}={expected:?}, got {actual:?}"
        ));
    }
}

// Renders the concise markdown report humans inspect after an eval run.
fn render_report(artifact: &EvalArtifact, failures: &[String], db_path: &Path) -> String {
    let mut report = String::new();
    report.push_str(&format!("# Classifier Eval: {}\n\n", artifact.scenario));
    report.push_str(&format!("- Generated at: `{}`\n", artifact.generated_at));
    report.push_str(&format!("- Model: `{}`\n", artifact.model));
    report.push_str(&format!("- Prompt hash: `{}`\n", artifact.prompt_hash));
    report.push_str(&format!("- Tool hash: `{}`\n", artifact.tool_hash));
    report.push_str(&format!(
        "- Cost: `${:.6}` / INR `{:.4}`\n",
        artifact.totals.cost_usd, artifact.totals.cost_inr
    ));
    report.push_str(&format!(
        "- Tokens: input `{}`, cached `{}`, cache creation `{}`, output `{}`\n",
        artifact.totals.input_tokens,
        artifact.totals.cached_input_tokens,
        artifact.totals.cache_creation_input_tokens,
        artifact.totals.output_tokens
    ));
    report.push_str(&format!("- Temp DB: `{}`\n\n", db_path.display()));

    report.push_str("## Date Results\n\n");
    for (date, actual) in &artifact.dates {
        report.push_str(&format!("### {date}\n\n"));
        report.push_str(&format!("- Threads seen: `{}`\n", actual.threads_seen));
        for (state, count) in &actual.state_counts {
            report.push_str(&format!("- `{state}`: `{count}`\n"));
        }
        report.push('\n');
    }

    report.push_str("## Mismatches\n\n");
    if failures.is_empty() {
        report.push_str("No mismatches found.\n");
    } else {
        for failure in failures {
            report.push_str(&format!("- {failure}\n"));
        }
    }
    report
}

// Hashes prompt/tool files so reports can identify behavior-changing inputs.
fn file_hash(path: &Path) -> Result<String> {
    Ok(fnv1a_hex(&fs::read(path)?))
}

// Computes a small stable hash without adding a dependency to the xtask crate.
fn fnv1a_hex(bytes: &[u8]) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

// Creates a unique local DB path for an eval run.
fn temp_db_path(name: &str) -> PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before Unix epoch")
        .as_nanos();
    env::temp_dir().join(format!(
        "quotewatch-eval-{}-{}-{suffix}.sqlite",
        sanitize_name(name),
        std::process::id()
    ))
}

// Lets scenario export paths be relative to the scenario file.
fn resolve_path(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

// Turns scenario names into filesystem-friendly report directory names.
fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

// Finds the workspace root from the xtask crate location.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask manifest should live under workspace root")
        .to_path_buf()
}

// Resolves the same external prompt selected by the product process so eval hashes
// identify the actual behavior-changing input without committing that input.
fn classification_prompt_path() -> Result<PathBuf> {
    let configured = env::var("CLASSIFICATION_PROMPT_FILE")
        .map_err(|_| "CLASSIFICATION_PROMPT_FILE must be set for classifier evals")?;
    if configured.trim().is_empty() {
        return Err("CLASSIFICATION_PROMPT_FILE must not be empty".into());
    }
    let path = PathBuf::from(configured);
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(workspace_root().join(path))
    }
}
