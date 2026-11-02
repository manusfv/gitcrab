# xattr 0.2.3 compatibility patch

Source: https://crates.io/api/v1/crates/xattr/0.2.3/download

Published crate SHA-256: `6d1526bbe5aaeb5eb06885f4d987bcdfa5e23187055de9b83fe00156a821fabc`.
Upstream: https://github.com/Stebalien/xattr. Original MIT/Apache-2.0 licenses are retained alongside all production source.

Only behavior change: `src/sys/mod.rs` selects `libc::ENODATA` on Linux/Android instead of the deprecated/removed `libc::ENOATTR` alias. macOS/BSD retain their platform's ENOATTR value. Linux CI at d4ebb8d identified the actual compilation failure against libc 0.2.191. The old alias in libc 0.2.177 was explicitly equal to ENODATA; [upstream libc discussion](https://github.com/rust-lang/libc/issues/1356) documents this platform distinction.

This crate is transitive through GPUI's zed-async-tar. Prefer removing this patch when the GPUI family updates that dependency. No libc downgrade or broad GPUI fork is maintained. The upstream integration tests are not copied; GitCrab's platform CI builds the actual dependent native application.
