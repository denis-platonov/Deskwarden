# Menus and scroll bars to the v2 design

The owner: two things still look like the 0.x app -- the dropdown menus and the
scroll bars.

## Tasks

1. **`kit::menu`** -- one popover, one row, one select. Surface per the design's
   add-step menu (radius 16, padding 6, ring `rgba(20,26,48,.06)`, shadow
   `0 18px 40px rgba(20,26,48,.2)`, opens 8 under its anchor); row 42 high,
   radius 11, padding `0 12`, gap 11, 15/600, icon 16, hover wash, selected =
   blue wash + check, danger = error red on the danger wash. Tests pin every
   number.
2. **Route every menu through it.** `Popup::menu`, `context_menu`,
   `menu_button`, `ComboBox` and `selectable_value` are gone from `src/`
   outside `kit/menu.rs`; the Area-built popovers (language picker, sign-in
   steps' add menu, people suggestions) take the kit's frame and rows. A source
   scan (`kit::menu::tests::every_menu_in_src_goes_through_the_kit`) keeps it so.
3. **Global style.** `theme::apply` sets `menu_corner_radius`, `popup_shadow`,
   `menu_margin` and `spacing.scroll` so a menu or scroll area that is missed
   still looks v2.
4. **Scroll bars.** `theme::scroll_style()` in `Style::spacing.scroll`.
5. **Previews.** `ui_preview` surfaces: an open select, an open context menu, a
   scrolled list.

## Menus converted

See the report; every site is listed with file and line there.

## Rulings

- Ruling: the select (closed box) is the app's field look (height 50, radius 14,
  16pt, 1.5 border) in forms and the row-control look (42, 12, 15/600) in rows
  of settings and composers -- the design draws no select at all, only
  segmented controls and buttons, so the box is the design's own input/button
  numbers with a drawn chevron -- cost if wrong: two constants (`FIELD`, `ROW`).
- Ruling: a menu row's hover wash is `Palette::hover` (`#f1f0ee`), not the add
  menu's literal `#f3f2f0` -- the palette has one nav hover and the dark theme
  needs a token, not a literal; the two differ by 2/255 -- cost if wrong: one
  colour.
- Ruling: a danger row (Delete) is the error red (`#b42318`) on the danger wash,
  not the add menu's `#8c3c33`. The design has no delete row in a menu; the app's
  existing Delete was already `theme::ERROR` and the owner asked for red. The
  add-step menu's secret steps keep their `#8c3c33` through `Item::ink`.
- Ruling: the design draws no scroll bar (its panes are plain `overflow: auto`),
  so the bar is the app's rule: floating, no track, 6pt idle and 10pt under the
  pointer, rounded, ink at 0.4 alpha (0.7 when grabbed), hidden until the pointer
  is over the area, 3pt in from the edge, taking no layout room. Surfaces that
  reserve their own lane (`scrollbar_in_gutter`: item list, detail pane, edit
  form) keep their lane and a 6pt bar that does not grow. Dragging the handle
  paints it `BLUE_DEEP` (egui reads `widgets.active.fg_stroke`) -- accepted.
- Ruling: the people-suggestions popover goes from radius 14 to the kit's rich
  look (18, padding 8) so it is the same surface as the avatar menu -- cost if
  wrong: one `Look`.
- Ruling: popovers that hold a text box or a palette of chips (the sequence
  builder's `+ Text`, `+ Key`, `+ Wait`) keep their contents and take only the
  kit's surface (`popover(.., MENU.fit(), ..)`); they are not lists of rows.
- Ruling: keyboard -- Escape closes (egui); Down/Up with nothing focused lands on
  the first/last row; further movement and Enter/Space are egui's focus and
  click handling; a focused row paints as a hovered one. Type-ahead is not
  built.
- Ruling: `theme::dropdown` (Send's lifetime) is now 42 high (the row control)
  rather than 32, because it is the kit's select.
