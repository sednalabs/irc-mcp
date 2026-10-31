# Proposed toolkit extraction (not yet upstreamed)

`mcp-toolkit-resource-state` is a dependency-free staging crate. The service uses
it directly so the proposed extraction has a concrete consumer. It contains:

- `InvalidationSet`: finite accepted inventory, duplicate coalescing and fair
  insertion-order draining. It is a dirty-resource set, not an event broker.
- `CursorStore`: opaque caller-generated tokens, mandatory principal/resource/
  generation binding, expiry and entry/byte limits. It is not an authority token.

`toolkit-resource-state.patch` proposes adding this small crate to the pinned
toolkit workspace. Maintainers may prefer these modules under toolkit-core;
that placement choice does not require copying application policy upstream.
The service-level async notification loop, IRC configuration, credentials, effect
receipts and channel permissions remain outside the helper crate.

The patch contains no private-project names, references or domain contracts.
It does not introduce another runtime, database or network process.

Review the patch against toolkit commit
`62e1add3eccadc191c5f7791bce4d2be1a8380a3`, apply on a feature branch, run formatter,
unit tests and the repository's normal checks, and submit through its existing
review process. Nothing has been pushed or merged by this package.

After acceptance, pin the consuming service to the actual accepted toolkit
revision, replace the path dependency with that package/module, regenerate and
review Cargo.lock, and delete the staging copy. Do not maintain two diverging
implementations. The patch's added source bytes are checked against this staging
crate by `tests/test_artifact_contracts.py`.

The 0.2 consumer adds generic regression journeys for an update after a dirty
marker is popped, repeated initial invalidation without queue growth/starvation,
and a discarded cursor read without consumption or TTL extension. The IRC-specific
inbox and attention policy intentionally stay in the service, not this crate.
