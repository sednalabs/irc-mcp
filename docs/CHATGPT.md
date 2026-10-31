# ChatGPT plugin integration

This project is a **tool-only MCP resource server**, not a submitted or
approved ChatGPT plugin. It uses Streamable HTTP at a stable HTTPS `/mcp`
URL behind a private proxy. No widget or embedded agent runtime is required.

## Authentication and linking

Use an established HTTPS OAuth 2.1 issuer with authorisation code + PKCE **S256**,
discovery metadata, and a supported token-endpoint authentication method.
The server refuses startup if the issuer does not advertise PKCE S256 and one
compatible token method.

Choose an actual ChatGPT client registration mode supported by the issuer:
a predefined registered client (the current configuration uses an exact
client-ID allowlist), Client ID Metadata Document (CIMD), or Dynamic Client
Registration (DCR). Configure the production redirect URI shown in ChatGPT's
plugin connection settings and match the issuer's RFC 9207 support. Do not
invent a common redirect URI or assume any arbitrary client is approved.

ChatGPT sends the OAuth `resource` parameter; the issuer must preserve it
and mint audience-bound access tokens for this server's exact public `/mcp`
URL. The server validates signature, issuer, audience, subject, expiration,
client and scopes. Read tools require `irc.read`; sending, joining and
leaving require `irc.write` as well. Per-tool `securitySchemes` and the
matching `_meta` mirror inform the client of each tool's required scopes.
When a connected account lacks write scope, the write tools return a
tool-level `mcp/www_authenticate` challenge.

One configured OAuth subject maps to one persistent IRC service identity.
This is not multi-tenant impersonation or per-chat identity isolation.

## Tool and content contracts

The eight tools use a closed, operator-configured private conversation
inventory. `irc_send` and `irc_part` explicitly declare possible irreversible
side effects; read tools are read-only. All annotations are hints, while the
server performs its own authorisation. IRC content remains untrusted data.
Readable JSON is supplied in both text and `structuredContent` for host
interoperability. Effect receipts distinguish WeeChat accepting input
from IRC/recipient delivery and agent attention.

The `2026-07-28` MCP protocol and native `subscriptions/listen` are
supported only where the connecting host implements them. A Codex or
ChatGPT client without subscriptions can pull messages using `irc_inbox`
or `irc_read`. MCP Events webhooks and dot-triggered wake are not included.

## Test with a real ChatGPT connection

Deploy an authorised, security-fixed WeeChat Relay privately over a Unix
socket, and publish only the OAuth-protected `/mcp` HTTPS endpoint.

Verify:
- Protected-resource metadata and bearer challenges resolve to the issuer.
- Issuer discovery advertises S256 and selected CIMD/DCR/predefined-client
  mode, plus actual token authentication methods and the correct callback.
- Tool scanning returns all eight tools, exact input/output schemas,
  descriptor-level + mirrored `securitySchemes`, and explicit safety hints.
- ChatGPT OAuth linking succeeds with a real account and audience-bound token.
- A read-only token cannot send; ChatGPT can request a permitted write scope
  without exposing credentials or expanding the server's subject allowlist.
- Reading a real channel, sending an authorised message, receiving a reply,
  and retrying an ambiguous effect behave as documented.
- Subscriptions are consumed only where the host supports them; there is no
  unsupported claim of autonomous model wake.
- Failed authentication, invalid inputs, revoked permissions, failed Relay
  connections, rate limiting and cancellation produce safe errors.

For **public Plugin Directory submission**, a source repository alone is
insufficient. OpenAI requires an operating stable server, verified domain,
website/support/privacy-policy/terms URLs, review test accounts and test
cases, release information and a review demonstration. Do not invent these
external assets or claim submission approval.

References:
- https://developers.openai.com/plugins/build/mcp-server
- https://developers.openai.com/plugins/build/auth
- https://developers.openai.com/plugins/plugin-guidelines
- https://developers.openai.com/plugins/deploy/submission
