# Toolkit staging consumer migration

This branch tests the **unmerged** toolkit integration candidate
`9983a946d99f950f39f4ec656b31fbd726f818b9` from https://github.com/sednalabs/mcp-toolkit-rs/pull/331.

Adopted:
- Generic bounded `InvalidationSet` from toolkit core; message data remains
  separately retained in IRC's attention inbox.
- ChatGPT OAuth `tools/list` HTTP descriptor projection from the toolkit
  server; eliminates IRC's duplicate JSON response adapter.
- Generic tool-level reauthorisation challenge metadata from toolkit auth.

Intentionally **not yet adopted**: `ScopedCursorStore` in toolkit PR #325.
IRC currently limits **aggregate** cursor state to 1 MiB, while the upstream
store only bounds entry count and individual serialized payloads. Replacing it
without an equivalent whole-store limit would weaken the existing memory
safety guarantee. Local staged cursor support remains until the upstream API
is proven equivalent.

This is an experimental source integration, not a deployed ChatGPT connector,
successful live WeeChat test, or ready-to-merge main revision. The toolkit
integration PR is a validation assembly; consumer pin must be moved to
accepted main and lockfile/CI revalidated before production.
