pub(crate) const SELECT_RUN_BY_KEY: &str = "select id from runs where run_key = ?1";

pub(crate) const INSERT_COMMITTED_RUN: &str = "
insert into runs(
  run_key, label, account, as_of_date, started_at, finished_at, threads_seen, model,
  input_tokens, cached_input_tokens, cache_creation_input_tokens, output_tokens,
  cost_usd, cost_inr, notes
)
values(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)";

pub(crate) const UPSERT_THREAD_INPUT: &str = "
insert into threads(thread_id, subject, state, previous_state, raw_thread_json, digest_json, last_seen_run_id, updated_at)
values(?1, ?2, 'IGNORED', null, ?3, ?4, ?5, ?6)
on conflict(thread_id) do update set
  subject = excluded.subject,
  raw_thread_json = excluded.raw_thread_json,
  digest_json = excluded.digest_json,
  last_seen_run_id = excluded.last_seen_run_id,
  updated_at = excluded.updated_at";

pub(crate) const REPORT_ITEMS: &str = "
select threads.thread_id, runs.account, state,
       requester_name, requester_email, requester_org, subject,
       latest_customer_message_at, latest_company_message_at, quote_sent_at,
       extracted_requirement, short_reason, confidence, expires_on
from threads
left join runs on runs.id = threads.last_seen_run_id
where state in ('QUOTATION_REPLY_PENDING', 'FOLLOWUP_PENDING')
  and (expires_on is null or expires_on >= coalesce((select max(as_of_date) from runs), date('now')))
order by state, coalesce(latest_customer_message_at, quote_sent_at, updated_at) desc";

pub(crate) const LATEST_RUN_COST_SUMMARY: &str = "
select started_at,
       finished_at,
       coalesce(input_tokens, 0),
       coalesce(cached_input_tokens, 0),
       coalesce(cache_creation_input_tokens, 0),
       coalesce(output_tokens, 0),
       coalesce(cost_inr, 0)
from runs
order by id desc
limit 1";

pub(crate) const LATEST_RUN_DATE: &str = "select as_of_date from runs order by id desc limit 1";

pub(crate) const ACTIVE_THREAD_IDS: &str = "
select thread_id
from threads
where state in ('QUOTATION_REPLY_PENDING', 'FOLLOWUP_PENDING')
  and (expires_on is null or expires_on >= ?1)";

pub(crate) const ACTIVE_RAW_THREADS: &str = "
select raw_thread_json
from threads
where state in ('QUOTATION_REPLY_PENDING', 'FOLLOWUP_PENDING')
  and (expires_on is null or expires_on >= ?1)
  and raw_thread_json is not null
  and raw_thread_json != '{}'";

pub(crate) const ACTIVE_DIGESTS: &str = "
select thread_id, digest_json
from threads
where state in ('QUOTATION_REPLY_PENDING', 'FOLLOWUP_PENDING')
  and (expires_on is null or expires_on >= ?1)
  and digest_json is not null";

pub(crate) const UPDATE_THREAD_CLASSIFICATION: &str = "
update threads set
  previous_state = state,
  state = ?1,
  requester_name = ?2,
  requester_email = ?3,
  requester_org = ?4,
  subject = coalesce(?5, subject),
  latest_customer_message_at = ?6,
  latest_company_message_at = ?7,
  quote_sent_at = ?8,
  latest_relevant_message_at = ?9,
  extracted_requirement = ?10,
  short_reason = ?11,
  confidence = ?12,
  classification_json = ?13,
  expires_on = ?14,
  updated_at = ?15
where thread_id = ?16";

pub(crate) const INSERT_TRANSITION: &str = "
insert into transitions(thread_id, run_id, previous_state, new_state, reason, changed_at)
values(?1, ?2, ?3, ?4, ?5, ?6)";

pub(crate) const PREVIOUS_STATE: &str = "select state from threads where thread_id = ?1";

pub(crate) const CREATE_SCHEMA: &str = "
create table if not exists runs (
    id integer primary key,
    run_key text,
    label text,
    account text not null,
    as_of_date text not null,
    started_at text not null,
    finished_at text,
    threads_seen integer default 0,
    model text,
    input_tokens integer default 0,
    cached_input_tokens integer default 0,
    cache_creation_input_tokens integer default 0,
    output_tokens integer default 0,
    cost_usd real default 0,
    cost_inr real default 0,
    notes text
);

create table if not exists threads (
    thread_id text primary key,
    subject text,
    state text not null default 'IGNORED',
    previous_state text,
    requester_name text,
    requester_email text,
    requester_org text,
    latest_customer_message_at text,
    latest_company_message_at text,
    quote_sent_at text,
    latest_relevant_message_at text,
    expires_on text,
    extracted_requirement text,
    short_reason text,
    confidence real default 0,
    raw_thread_json text,
    digest_json text,
    classification_json text,
    last_seen_run_id integer,
    updated_at text not null
);

create table if not exists transitions (
    id integer primary key,
    thread_id text not null,
    run_id integer not null,
    previous_state text,
    new_state text not null,
    reason text,
    changed_at text not null
);

create table if not exists oauth_tokens (
    token_key text primary key,
    nonce text not null,
    encrypted_token_json text not null,
    updated_at text not null
);
";

pub(crate) const CREATE_RUN_KEY_INDEX: &str = "
create unique index if not exists runs_run_key_unique
on runs(run_key)
where run_key is not null";

pub(crate) const READ_ENCRYPTED_TOKEN: &str =
    "select nonce, encrypted_token_json from oauth_tokens where token_key = ?1";

pub(crate) const WRITE_ENCRYPTED_TOKEN: &str = "
insert into oauth_tokens(token_key, nonce, encrypted_token_json, updated_at)
values(?1, ?2, ?3, ?4)
on conflict(token_key) do update set
  nonce = excluded.nonce,
  encrypted_token_json = excluded.encrypted_token_json,
  updated_at = excluded.updated_at";
