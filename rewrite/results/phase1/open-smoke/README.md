# First isolated fixture capture

`tools/verify-slice.sh` passed on Linux Xvfb on 2026-11-23 at 15:58 CET. The capture contained the fixture's background and marker colors. CEF emitted browser creation, HTTP 200 load, paint, and closure events. The launcher reported process namespace exit 0.

The [build stamp](build.sha256) identifies the tested executable. It used CEF beta 158.0.3 and the GPUI and cef-rs revisions in [pins.toml](../../../pins.toml), on main base `c2fe8c1`.
