#!/usr/bin/env python3
"""Generate an add-only candidate plus one captured workspace-membership hunk."""
from pathlib import Path
import difflib
root = Path(__file__).resolve().parents[1]
staged = root / 'upstream/mcp-toolkit-resource-state'
# Captured pinned workspace prefix; not a fabricated full checkout.
before = '[workspace]\nresolver = "2"\nmembers = [\n  "crates/mcp-toolkit",\n  "crates/mcp-toolkit-core",\n  "crates/mcp-toolkit-auth",\n'
after = before.replace('  "crates/mcp-toolkit-core",\n', '  "crates/mcp-toolkit-core",\n  "crates/mcp-toolkit-resource-state",\n')
patch = 'diff --git a/Cargo.toml b/Cargo.toml\n'
patch += ''.join(difflib.unified_diff(before.splitlines(True), after.splitlines(True), fromfile='a/Cargo.toml', tofile='b/Cargo.toml'))
for source in sorted(p for p in staged.rglob('*') if p.is_file()):
    rel = 'crates/mcp-toolkit-resource-state/' + source.relative_to(staged).as_posix()
    patch += f'diff --git a/{rel} b/{rel}\nnew file mode 100644\n'
    patch += ''.join(difflib.unified_diff([], source.read_text().splitlines(True), fromfile='/dev/null', tofile='b/'+rel))
(root/'upstream/toolkit-resource-state.patch').write_text(patch)
