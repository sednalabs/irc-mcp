# IRC MCP 0.2.0 — experimental source candidate

A hosted Rust MCP adapter for a private, patched WeeChat Relay connected to an
existing IRC server. The service uses `mcp-toolkit-rs` and its pinned RMCP fork.
It does not implement an agent runtime or expose raw WeeChat commands.

**Status: EXPERIMENTAL / NOT PRODUCTION READY.** Source implementation, not a compiled or deployed release. This
package was authored locally without an available Rust/Cargo toolchain,
private Relay access or a configured OAuth issuer. Publication of the source via a draft PR
does not establish build qualification or deployment. See `BUILD_STATUS.json` for
exact checks actually run. Rust tests and the real ChatGPT journey remain to be
executed on an approved worker. No Cargo.lock has been fabricated.

## Implemented source surface

Eight tools: `irc_status`, `irc_channels`, `irc_channel_info`, `irc_read`,
`irc_inbox`, `irc_send`, `irc_join`, `irc_part`. Exact input/output descriptors are in
`schema/tools.json`. Configured conversations can be channels or already-open
DM buffers. There is no arbitrary target, URL, shell, script or raw-command tool.

The main 0.2 addition is a non-destructive attention inbox. DMs and direct mentions
are selected by default; routine status messages stay quiet unless their channel
is explicitly watched. Reads never mark messages consumed. See
[the attention contract](docs/ATTENTION.md) and [the change log](CHANGELOG.md).

The service includes:

- Hosted Streamable HTTP through the toolkit/RMCP, OAuth discovery and JWKS
  validation, an exact caller-subject policy, read/write scopes and host/origin
  guards. Authentication and caller policy precede subscription acknowledgement.
- Readable JSON resources and native `subscriptions/listen`, backed by a
  bounded, coalescing invalidation queue. No model polling or embedded LLM.
- WeeChat HTTP/JSON and WebSocket over a Unix socket, exact API-version opt-in,
  a minimum stable WeeChat version gate, bounded payloads and reconnect handling.
- Principal/resource/generation-bound cursors for a bounded client-buffer window.
  Backend reconnects invalidate continuity rather than silently accepting stale IDs.
- SQLite retry receipts persisted before sends. A lost or cancelled response
  remains `outcome_unknown`; the same key does not dispatch again within 24 hours.
- Graceful MCP shutdown. The dedicated WeeChat service identity is persistent;
  the adapter does not pretend an HTTP request is an agent's lifetime.

This first version advertises **MCP 2026-07-28 only**. Earlier protocol eras are
not advertised until their request-authentication and transport paths have been
qualified. Actual ChatGPT protocol/transport compatibility is not yet proven.
Native resource notifications are not dot-trigger webhooks or proof of attention.

## Local build qualification

Install Rust 1.99.0 and the dependencies in `requirements-validation.txt` on an
approved build worker, then run:

```sh
./scripts/qualify.sh --prepare
# Review/commit formatting, the generated extraction patch and Cargo.lock.
./scripts/qualify.sh
```

The explicit `--prepare` mode formats and generates Cargo.lock; normal verification
uses `--locked` and does not silently change dependency resolution. Both modes run
the checks, tests and Clippy. Review and commit prepared changes before publication. GitHub-hosted CI runs on pull requests, pushes to `main`, and manual dispatch, using a pinned checkout action and no deployment credentials. CI intentionally fails until a reviewed `Cargo.lock` is committed; this is a build qualification requirement, not a temporary bypass. The workflow rejects an untracked lockfile or an altered tree.
Git dependencies are pinned at source; transitive dependency resolution is still
outstanding until the lockfile exists. Nothing in these scripts deploys a service.

The temporary dependency in `upstream/mcp-toolkit-resource-state` is the same
source proposed by the toolkit patch. It has no external dependencies. It is not
an existing published toolkit package. See `upstream/README.md`.

## Configuration and isolation

Start from `config.example.toml`. Replace the example public origin, issuer,
registered OAuth client ID and exact authorised `sub`. Configure the issuer to
mint access tokens with audience equal to the public `/mcp` URL and scope
`irc.read`; grant `irc.write` only when sending is intended. Both token subject
and OAuth client identity are restricted. This is one authorised subject using
one dedicated service identity, not multi-tenant agent identity federation.

Provision a separate, non-admin WeeChat profile with verified IRC TLS and SASL.
Do not attach this adapter to an administrator's profile, terminal or credentials.
Do not load scripts or input hooks that reinterpret ordinary message text.
Confirm a currently supported, security-fixed WeeChat build: 4.10.1 is a minimum
version floor, **not** a claim that all future advisories are resolved. The API
version is separately configured and checked at runtime.

Keep Relay on a private Unix socket. Permit only the dedicated adapter user/group
to connect. Store its password in the owner-only file named in configuration;
never place it in a tool call, command argument, Git file, transcript or log.
Create the state directory with mode 0700 and the same owner as the secret file.
The systemd example supplies a private state directory and restrictive defaults.

The adapter binds to loopback only. Terminate HTTPS in the existing reverse proxy,
preserve Host/authentication/MCP headers, disable buffering and caching, and
keep the backend port and Relay socket private. `deploy/nginx-location.conf` is
an integration fragment, not an installed or verified proxy configuration.

Once configured and qualified:

```sh
irc-mcp --config /etc/irc-mcp/config.toml
```

`/health` is liveness only. Use authenticated `irc_status` for Relay observations.
The API does not claim a live IRC connection merely because Relay responds.

## Conversation contracts

Call `irc_channels` for permitted target handles. `irc_join` and `irc_part` work
only on configured channels with write permission. DM buffers must already be
open in the dedicated profile. This version does not provision IRC accounts.

`irc_send` accepts single-line text of at most 400 UTF-8 bytes. It rejects control
characters and a leading slash instead of relying on unqualified slash escaping.
All effectful tools require a unique request key; reuse the same key on a retry.
A key reused with changed input is rejected. Receipt retention is 24 hours; do not
retry an old unknown send with a new key. The receipt store needs persistent local
storage. Exactly-once delivery is not claimed.

`accepted_by_client` means the WeeChat input endpoint returned 204 and that result
was recorded. It does not mean Ergo accepted the message, anyone received it,
the recipient model read it, or any work was accepted. `outcome_unknown` means
inspection is required before deciding whether another send is appropriate.

`irc_read` scans at most 200 retained buffer lines and returns a bounded page.
Messages are normalised from IRC message/notice lines; status lines still advance
the local cursor. IDs are returned as strings. Server account, timestamp and
message-ID metadata remain null when unavailable; nick prefixes are not identity
proof. Message text and metadata are untrusted conversational input.

Cursor failure or an absent anchor returns a gap. No server-side CHATHISTORY,
complete archive, offline DM replay or exactly-once consumption is claimed.
A fetch does not mark a message read in WeeChat or accept any external obligation.
The configured MCP read policy is not automatic live synchronisation of IRC
membership revocations; a previously populated buffer can retain earlier data.
Immediate membership-based history revocation is not qualified in this version.

## Subscriptions

Subscribe to the returned resource URIs using native `subscriptions/listen`.
The SDK sends the acknowledgement. The service registers a bounded listener,
issues an initial invalidation to cover the acknowledgement/registration gap,
then sends only requested resource invalidations. Read after acknowledgement.

The inbox URI offers selective attention without subscribing to every conversation.
Repeated changes to one resource coalesce; message identities do not. Notification
sends are paced independently per listener. Slow listeners are timed out without
holding up others. Listeners end on disconnect, cancellation, shutdown, token
expiry or the 60-second maximum stream lifetime. Compatible clients renew the
protocol stream, not the model. JWT revocation is limited by issuer/token policy;
a fresh stream alone cannot revoke a still-valid self-contained JWT.

No MCP Events callback adapter, automatic dot wake or resource-notification wake
behaviour is claimed. Those are later, independently tested integrations.

## Before live use

Run the Rust checks, isolated Relay transport tests and `docs/ACCEPTANCE.md`.
Then verify the actual hosted OAuth and ChatGPT tool journey. Preserve the
existing IRC pilot; do not restart other clients or expose Relay as an incidental
part of deploying this adapter. This source candidate is not approval to deploy.

## Public repository policy

The source is designed for public review under Apache-2.0. Do not commit an
operator's `config.toml`, authentication material, TLS certificates, WeeChat
profiles, Relay sockets, database/state directories, logs, snapshots,
production URLs, channel membership exports or private operational reports.
Only `config.example.toml` and the generic deployment templates belong in Git.

`.gitignore` is defence in depth, not a security boundary: **review staged
files and history before every push**. If a credential is accidentally
committed, rotate/revoke it even if the commit is later removed. Never replace
these placeholders with production values inside tracked example files.

The checked-in workflow uses only GitHub-hosted runners and unprivileged
source verification. It does not use a self-hosted runner, access private
network resources, deploy, or publish releases. CI requires a checked-in,
reviewed `Cargo.lock`; the source archive does not invent one.

The `validation/` directory records previous *local artefact checks*, not
compilation or live integration. `BUILD_STATUS.json` is the authoritative
record of what has and has not been verified.
