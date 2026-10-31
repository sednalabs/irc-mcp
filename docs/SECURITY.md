# Security boundary and residuals

The exposed boundary is HTTPS MCP with OAuth; the private boundary is a WeeChat
Unix socket that can execute powerful client commands. The latter must never be
made directly reachable by MCP callers. Only operator-selected buffers and
validated, narrow operations cross it. No arbitrary URI/host or command is accepted.

A dedicated IRC service identity stays connected independently of model turns.
That is not ghost presence for a terminated specialist and not proof of model
attention. This adapter does not repair lifecycle behaviour of separate existing
agent-owned IRC clients.

Initial isolation is one authorised OAuth subject/client allowlist and one IRC
identity. It does not distinguish separate conversations under the same principal.
Do not deploy as a multi-tenant relay, an admin front door or a shared secret vault.

Resource policy is static configuration. Permission changes require controlled
restart. Current IRC membership is not a full authorisation oracle for already
retained client-buffer history. Strict instantaneous revocation is not implemented.

The service stores short-lived cursor bytes, bounded invalidations, an explicitly
bounded in-memory attention buffer, and durable retry fingerprints/receipts. Inbox
message content is not written to SQLite. Retention eviction, restart and Relay
continuity boundaries are explicit; there is no general transcript archive. The server and
client may have separate logging/retention settings: configure those explicitly.

JWT validation uses the toolkit, not hand-written crypto. It has the revocation
limits of issuer/JWKS/token policy. Subscriptions close at token expiry or at most
60 seconds; that is not equivalent to immediate revocation of a valid JWT.

The version floor 4.10.1 prevents known-old Relay binaries being used accidentally;
it is not vulnerability scanning or proof that a later build is safe. Recheck
vendor advisories at deployment. The authoring environment did not upgrade hosts.

Outstanding assurance: Rust compilation and runtime tests, actual hosted OAuth,
MCP 2026-07-28 ChatGPT compatibility, live Relay/IRC interoperation, failure-window
exercises, current dependency audit and operator rollout acceptance.

Attention preferences are operator-owned. Sender-supplied text/priority cannot
modify watches, interrupts, scopes or actions. Self-echo filtering reduces a common
feedback path but is not a general agent-loop proof. An attention classification
never invokes an LLM. Distinct clients sharing this OAuth subject are not isolated
security principals, even though they can keep independent reading cursors.

## Public repository hygiene

Use real credentials only in an operator-managed secret store, outside Git.
Do not publish a real deployment configuration, WeeChat profile, Unix-socket
permissions snapshot, state database, channel export, certificate, log,
private Ops harvest or topology. Inspect **staged files and full Git history**
when publishing; .gitignore does not protect already tracked files. Rotate
credentials after any accidental exposure. The checked-in GitHub Actions
workflow uses a public runner and has no private network or deployment access.
