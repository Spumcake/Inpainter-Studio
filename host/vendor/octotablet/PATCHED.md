# Patched copy of octotablet 0.1.0

This is octotablet 0.1.0 from crates.io (MIT, see LICENSE.txt), used through
`[patch.crates-io]` in the app's `Cargo.toml`. The only change is in
`src/platform/wl/mod.rs`. When binding `wl_seat` and `zwp_tablet_manager_v2`,
it caps the version at the one the compiled protocol describes.

Upstream binds whatever version the compositor advertises. The protocol
definitions bundled with 0.1.0 (wayland-protocols 0.31.2) only describe tablet
version 1. A compositor offering version 2 then sends `zwp_tablet_v2` event 5
(`bustype`). libwayland cannot parse that event, so the shared Wayland
connection fails and the whole window exits. This was observed on 2026-10-10.
Upstream `master` at `1b0a14e` still binds the advertised version.

Drop this copy once a release caps the bound version.
