# vendor

One crate, one patch, one reason.

## `gpui_ce_web`

`gpui_ce_web 0.1.0` on crates.io cannot be compiled. Its `platform.rs` does

```rust
include_bytes!("../../../assets/fonts/ibm-plex-sans/IBMPlexSans-Regular.ttf")
```

for eight font files, and that path points outside the published crate — the
`assets/` directory lives at the root of the `gpui-ce` workspace and is not in
the `.crate` tarball. So every build of the published crate fails at compile
time with eight `couldn't read` errors, and the web backend cannot be used
from crates.io at all.

This is that crate, byte for byte, with the eight paths pointing at the fonts
beside it instead — the same files, taken from the `gpui-ce` repository the
crate is published from. Nothing else is changed.

It is applied through `[patch.crates-io]` in the workspace manifest, so the
dependency graph, the version and the feature set are all exactly what they
would be if the published crate worked.

**Delete this directory and the patch when the fix lands upstream** — nothing
here is ours to maintain, and everything that uses it goes through `gpui` and
`gpui_platform` rather than through this crate directly.
