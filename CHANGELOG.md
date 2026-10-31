## ChatGPT tool-and-auth compatibility amendments (PR #1, pending CI)

- Per-tool OAuth schemes, human titles, corrected safety annotations and toolkit-based descriptor promotion.
- Scope-upgrade challenges and useful JSON text for clients without structured output support.
- Fail-fast issuer PKCE S256 and compatible token-method checks; ChatGPT linking and submission checklist.

# Changes

## 0.2.0 — source candidate, 31 October 2026

### Added

- `irc_inbox` tool and `irc://configured/inbox` resource with non-destructive,
  caller-managed opaque cursors and a typed result schema.
- Deterministic receiver-owned DM/mention/watch/mute policy; self-echo suppression,
  token-boundary mention matching, and explicit IRC case-mapping selection.
- Bounded in-memory attention observations, explicit retention gaps, UTF-8-safe
  previews and bounded identity/revision-based deduplication.
- Shared atomic generation reset for observation state and inbox contents.
- Per-listener notification pacing, actual Rust service/observer fixture tests,
  and generic invalidation/cursor regression tests in the proposed toolkit patch.

### Corrected

- Relay line updates use the documented `buffer_line_data_changed` event.
- Metadata uses WeeChat's `irc_tag_<name>_<value>` tag spelling, not an invented
  equals-sign representation; duplicate, oversized or unsafe metadata is unknown.
- Relay startup sends individual request objects, enables sync before the metadata
  snapshot, and bounds initial acknowledgement and WebSocket send waits.
- Configured buffer identity/type is checked before admitting events. Closing an
  unrelated buffer no longer resets the whole service.
- Resource reads now apply the same output-size and post-read authorisation checks
  as tool reads. Domain entry points require the configured subject.
- Observer shutdown clears live connectivity instead of leaving a stale observation.
- The qualification script separates explicit preparation from locked verification.

### Deliberately unchanged

- One authorised subject and one persistent service identity; no shared-admin profile.
- Durable send uncertainty and no automatic resend of ambiguous effects.
- No raw commands, mesh, scheduler, embedded LLM, webhook trigger, complete server
  history or new administrative authority.
- Public toolkit changes are a proposed patch, not an upstream landing.

### Qualification

See BUILD_STATUS.json. Artifact/schema checks are executed separately from the
Rust tests. No compilation, Rust test execution, real Relay, OAuth, ChatGPT or
production deployment is represented as complete without its own evidence.

### Public repository preparation (source-only amendment, 31 October 2026)

- Replaced self-hosted manual-only qualification with unprivileged GitHub-hosted
  PR/main/manual checks and fully pinned checkout action. No remote access
  or automatic deployment is configured.
- Extended .gitignore to exclude real configuration, credentials, certs,
  databases, WAL/SHM files, logs and backup archives while allowing examples.
- Documented public publication boundaries and explicitly labelled the source
  experimental pending compilation, a reviewed Cargo.lock and integration tests.
- No Rust application code or protocol contract was changed.

### First hosted qualification (31 October 2026)

- GitHub-hosted Rust 1.99 generated and reviewed `Cargo.lock`, ran rustfmt,
  compiled the toolkit-integrated service, and passed all 52 Rust tests.
- Resolved three Clippy style findings in test setup and module ordering.
- Prepared a locked, non-deploying strict CI rerun; live WeeChat/ChatGPT
  qualification remains a separate acceptance boundary.
- Hosted evidence: https://github.com/sednalabs/irc-mcp/actions/runs/41179746060
