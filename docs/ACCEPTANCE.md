# Qualification still required

These are tests to execute, not results already obtained.

## Build and protocol boundary

Run `scripts/qualify.sh --prepare` on Rust 1.99, then `scripts/qualify.sh` after reviewing preparation. Review and commit generated Cargo.lock and
formatting. Resolve any compiler/Clippy failures before producing a binary.
Run the whole dependency-free helper crate test suite and the service tests.
Confirm request-bound `Principal` extensions survive the pinned RMCP HTTP path.

Use the actual MCP 2026-07-28 wire contract to test discovery, tools/list,
resources/list/read, tools/call and subscriptions/listen over HTTPS. Assert the
ack precedes updates, notifications contain the matching subscription ID and a
cancelled stream releases capacity. Check private/zero-TTL history responses.
A host limited to older MCP revisions must receive an honest incompatibility,
not a falsely advertised legacy path.

## Authentication and authorisation

Use a disposable issuer and keys. Reject missing/invalid/expired tokens, wrong
issuer/audience, missing scope, unapproved OAuth client ID and wrong subject.
A forged bare auth context must not satisfy the request-bound gate. Reject foreign
Host, Origin and absolute-URI authority. Missing Origin remains valid for an
otherwise authorised server-to-server client. Changing or omitting a permitted
channel key must never disclose other buffers. Read-only scope/target must reject
all three writes before any Relay request that can cause an effect.

Check real OAuth discovery, sign-in and access-token issuance from the hosted
ChatGPT integration. Do not substitute a hand-created API key or dev token for
that proof. Check issuer access-token lifetime/revocation requirements separately.

## Private Relay

Confirm current vendor advisories, installed executable provenance and version.
Verify local socket ownership/mode and negative authentication. Ensure no public
Relay listener exists. The API version returned by the actual binary must match
an intentionally accepted version and the captured response fixtures.

Exercise version/buffer/line reads, controlled input and WebSocket sync-then-snapshot
with separate real IRC sender and receiver. Test reconnect, buffer clear/close,
missing metadata, oversized JSON/frames and stalled reads. Verify server-time,
account and message tags without assuming they are exposed by every WeeChat build.
Read a cursor after a reconnect or deleted anchor; require an explicit gap.

## Send uncertainty and cancellation

Repeat a send with the same key, concurrently and after service restart. Require
one dispatch within the 24-hour window. Change the body under the same key;
require rejection. Simulate client acceptance with the HTTP reply lost. Kill the
adapter after receipt reservation and before completion. Require outcome_unknown,
not an automatic resend or a claim that nothing happened. Check a response-store
failure after input acceptance. Check disk-full before dispatch and receipt budget.
No message text or bearer/password may appear in the SQLite file or logs.

## Functional journey

From ChatGPT: list the configured conversations, read a test channel, send one
question, have a separate agent reply, and read the reply. Observe exact sender
identity, scope and data boundaries. A real host must handle subscriptions without
waking a model for routine stream maintenance. Record actual supported behaviour.
No successful webhook/dot wake or full server-history claim follows from this test.

## Operational rollout

Use a dedicated profile with no OPER/admin identity and no input-rewriting scripts.
Run one adapter instance per configured identity/database. Keep receipts persistent.
Preserve snapshots and existing authorised clients. Verify proxy forwarding and
SSE behaviour; apply the configuration through the existing approved rollout path.

## Attention and handoff regressions

Execute `tests/inbox_contract.rs` and `tests/relay_events_contract.rs`, plus the
attention/inbox/relay-events unit tests. Prove non-destructive retry after a lost
response, independent cursors, arrivals during page consumption, and separation
between notification coalescing and message identity. Two identical texts with
different source line IDs must remain two events. A same-line revision is an update.

Verify real deployed Relay tag spelling, `self_msg`, local nick metadata and
`buffer_line_data_changed` frames. Ensure DMs, mentions and watches select only
configured buffers; a bogus plugin/type or changed buffer identity fails closed.
Send irrelevant highlighted text and sender-provided priority strings: neither
may bypass policy. Verify current case mapping rather than inferring it from a nick.

Exercise byte/count/age eviction, an expired cursor, and a disconnect between
cursor resolution and snapshot return. Require explicit gap/outage results. Test
an empty inbox while disconnected: it must not be reported as complete silence.
Check snapshot-before-live gaps by keeping sync enabled before establishing the
metadata baseline. Pre-baseline traffic is outside coverage, not silently included
in a guarantee. Compare after-baseline capture against an independent IRC sender.

For native subscriptions, keep one consumer slow and another active; fan-out must
not wait on network writes. Cancel an idle listener and one waiting on a rate-limit
delay; capacity must be released. Observe actual post-reconnect catch-up through
`irc_inbox` cursors, not a presumed MCP transport replay guarantee.
