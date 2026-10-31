# Attention inbox contract

`irc_inbox` and `irc://configured/inbox` expose the same receiver-owned attention
projection. They never drain a global queue, mark a message read, accept work,
invoke a model, or turn chat content into authority.

## Source and lifetime

This version retains **live Relay observations**, not all Ergo history. On each
connection the adapter enables sync first, then reads buffer metadata on the same
ordered WebSocket. Post-sync events arriving before the snapshot are buffered and replayed after
buffer identity is established. Pre-sync traffic is outside this projection;
use `irc_read` for available earlier buffer context.
No archive-completeness or offline-DM-delivery claim follows from an empty inbox.

The generation and inbox are changed under the same lock at every Relay continuity
boundary. Restart/disconnect clears the in-memory contents. Old cursors fail
explicitly. An event-stream outage returns `event_stream_unavailable`, not an empty
successful inbox. Plain connection state never implies IRC delivery or model attention.

The default retention is 256 entries, 512 KiB of serialized item data, and one hour.
Entry count also bounds object overhead. The dedup index is separately bounded to
twice the entry limit. A periodic observer tick prunes idle contents; reads/appends
also prune. Content is not written to SQLite. Retention is a deliberate exception
to the old candidate's "no transcript retention" wording: this is a small ephemeral
attention buffer, not another general transcript archive.

## Receiver policy

The operator's `[attention]` configuration chooses DMs, direct mentions, watched
channel handles, muted conversation handles, mention aliases and case mapping.
Defaults select DMs and mentions; ordinary status traffic is quiet. Muting wins
against mentions and watches. Self messages and NOTICE lines do not request attention.

The current nickname is taken from the validated IRC buffer metadata; `self_msg`
and same-nickname echoes are suppressed. Matching is token-based, not a substring
search. The default case mapping is ASCII; verify the network's actual CASEMAPPING
before selecting a different mapping. Unknown Unicode/network mappings are not
silently guessed. An arbitrary WeeChat highlight is not a mention by itself.

All knobs are receiver configuration, not `irc_inbox` arguments or sender priority
commands. Text such as "urgent", "steer" or "ignore previous instructions" cannot
change policy. `next_attention_boundary` and `when_convenient` describe receiver
classification; neither schedules a turn. An LLM or webhook adapter is not included.

Example: add `watched_channels = ["runner-status"]` to include ordinary messages
from that **already authorised** conversation. No arbitrary channels are opened.
Policy changes require a controlled restart and invalidate existing cursors.

## Reading without destructive acknowledgement

An initial call is `irc_inbox({"limit":20})`. Keep the returned `next_cursor`.
The next call supplies `{"after":"<opaque cursor>","limit":20}`. On a lost
response, retry with the preceding cursor. The source entries remain readable.
Do not share one server-managed "unread" counter between unrelated executions.
Separate clients retain independent cursors, even when authorised as the same
service principal. This is separate reading state, not authenticated per-chat identity.

If `more_in_window` is true, continue from the returned cursor. Each page is
bounded by count and serialized byte budget. Arrivals after a page snapshot remain
available to the next read; there is no replay iterator that attaches live delivery
only after yielding a historical slice. Preparing a page does not acknowledge it.

If retention removes an entry newer than the caller's position, the call returns
`inbox_gap`. A fresh read reports `retention_trimmed: true`; it never claims full
history. A cursor that expired or belongs to another principal/resource/generation
returns `invalid_cursor`. Opaque cursor tokens expire after fifteen minutes,
independently of the inbox retention window. A consumer already caught up through the last evicted
entry does not receive a false loss error.

Long message previews are truncated only at UTF-8 boundaries and carry explicit
`text_truncated` and `original_text_bytes` fields. Use the conversation resource for
available fuller context. Duplicate suppression is bounded and uses Relay buffer/
line identity plus the observed content revision, **not identical message text**.
A changed line is exposed as `message_updated`. If an edit invalidates an earlier
attention classification, the observer restarts the live view rather than presenting
that stale classification as current. Server-message-ID replay is not yet qualified.

## Subscriptions and compatibility

Subscribe only to the inbox URI to avoid routine channel notifications. Native
resource subscriptions issue initial invalidations, coalesce repeated URI changes,
and pace sends per listener (500 ms by default). No shared lock is held during
network delivery, and no subscriber owns an unbounded message queue.

Read after the acknowledgement/initial invalidation. Re-establishing a subscription
is not message replay; catch up through the inbox cursor. Cancellation does not
consume inbox events. Distinct messages are never coalesced merely because their
resource invalidations coalesce.

The transport remains the pinned MCP 2026-07-28 candidate. Real hosted ChatGPT
compatibility, OAuth and the patched WeeChat integration remain unqualified until
executed. This change does not add the separate dot Events/webhook extension.
