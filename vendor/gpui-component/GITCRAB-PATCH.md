# GitCrab component patch

Source: the crates.io `gpui-component` 0.5.1 package, published by Longbridge.
Upstream: https://github.com/longbridge/gpui-component
License: Apache-2.0; retained in `LICENSE-APACHE`.
Original package SHA-256:
`d021d46b4088d3d93a57ccdf443da85695a77272108caca2f6fe5369f584966a`.

Changes:

- `src/button/button.rs`: enabled buttons request the pointer cursor, including
  selectors and library-created dialog buttons. Disabled and loading buttons
  retain the arrow cursor. Explicit `Styled::cursor_default()` overrides remain
  supported for native tab controls. `Button::interaction_highlight(false)` lets
  tab labels suppress hover and press feedback while retaining tooltips and
  keyboard focus. Close buttons retain their hover and press feedback.
- `src/menu/menu_item.rs`: enabled menu entries request the pointer cursor.
  Disabled entries and noninteractive menu labels retain the arrow cursor.

The patch is applied through the root Cargo manifest so production, development
and CI use the same behavior without modifying the local Cargo cache.
