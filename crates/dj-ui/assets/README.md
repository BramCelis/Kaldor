# Kaldor logo, scaled for the app

The mark: a record as an eight-sided cut stone, faceted in five purples
(lit from the top left), with a ring and two decks orbiting it as small cut
diamonds (white and light purple).

- `kaldor-mark.svg`: the mark in a 100 x 100 view box. Rendered with resvg
  at the exact pixel size of the top bar (`src/brand.rs`). `#FFFFFF` (the
  white parts' lit side) and `#C9CED6` (their shaded side) turn into the
  ink on the light theme.
- `kaldor-icon-256.png`: the window and taskbar icon, the mark on the dark
  rounded square.
- `crates/dj-app/assets/kaldor.ico`: the same at 16, 24, 32, 48, 64, 128
  and 256 px (PNG entries). Put into the Windows .exe by `crates/dj-app/build.rs`.

Both icons are made from the SVG:

```sh
cargo run -p dj-ui --example make_icons -- crates/dj-ui/assets/kaldor-mark.svg crates/dj-ui/assets/kaldor-icon-256.png crates/dj-app/assets/kaldor.ico
```

The purple accent in `src/brand.rs` (`PURPLE`, #6B2FF0) is the record's
middle tone.
