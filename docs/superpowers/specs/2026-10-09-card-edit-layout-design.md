# Card details on the edit form: three rows, buttons inline

**Status:** approved by the owner 2026-10-09. Deskwarden only, edit form
(`src/vault_window/detail_edit.rs`), Card records.

## The owner's report

- The card's edit rows do not match the rest of the edit form. Its Remove
  buttons sit under the label instead of inline at the end of the row, as on
  the login form (§8a: a row is `field(flex: 1)` followed by its buttons).
- The Bank row uses different buttons from the rest of the form, and they are
  not vertically centred.
- Related fields should share a line: brand with number; month, year, code and
  ZIP together.

## The layout

```
CARD DETAILS
------------------------------------------------------------
Cardholder   [Denis Platonov                      ] [Remove]
------------------------------------------------------------
Card         [Mastercard v] [****************  Show] [Remove]
------------------------------------------------------------
Expires      [10] / [29]   Code [***  Show]   ZIP [91307   ]
------------------------------------------------------------
Bank         [dashboard.brex.com      v]           [Clear ]
             Chosen from the sites already in your vault...
```

1. **Cardholder.** It is one field, with its Remove button inline at the end.
2. **Card.** The brand chooser (the existing `form_combo`, at its natural
   width) sits in front of the number field. The number field takes the rest
   of the row and keeps its in-field Show. There is one Remove inline at the
   end. It clears the number and the brand, as the separate rows' Remove
   buttons did between them.
3. **Expires, Code, ZIP.**
   - Month and year are two short boxes, sized for two digits, joined by `/`.
   - Then the label `Code` and the security-code box, with its in-field Show.
   - Then the label `ZIP` and the billing ZIP box, which takes the remaining
     width.
   - **No Remove on this row** (owner's decision). An empty box is not saved,
     exactly as an empty box is not saved today.
   - The inline labels use the form's field-label style. They are vertically
     centred on the boxes.
4. **Bank.**
   - The picker is a drop-down-style button showing the chosen domain, or
     `Choose a bank…` when there is none. It opens the existing chooser.
   - Clear is inline at the end, in the same button style as Remove on the
     other rows.
   - Both are vertically centred on the row, in the form's standard row
     height.
   - The help sentence stays under the row, as it is.

### The bank is shown by name, not by its address (owner, 2026-10-09)

The read pane's Bank row and the edit form's bank picker button show a **name**
(`Brex`), not the stored domain (`dashboard.brex.com`). The stored value stays
the domain, because it is what finds the icon. Only what is drawn changes.

The name is resolved in this order:

1. **The name of the vault login the domain was chosen from.** The chooser
   offers domains from the vault's own logins. Find the logins whose saved
   websites have this host:
   - if there is exactly one, use its name;
   - if there are several and all carry the same name, use that name.
2. **Otherwise, the registrable domain's first label, capitalised.** For
   example, `dashboard.brex.com` becomes `Brex`, and `chase.co.uk` becomes
   `Chase`. The registrable domain comes from the same public-suffix rules
   `uri_match` uses.
3. **If neither applies** (an IP address or an unparseable value), the stored
   value as it is.

The domain is still visible as a tooltip on the name, so nothing is hidden.
Copy-on-click on the read pane copies the **name**. The chooser's list shows
each entry as `Name — domain`, so the person can tell two logins for the same
bank apart.

All rows use the form's existing row primitives: the label column, row
height, `row_button_width` for the trailing buttons, full-bleed row rules,
and `SECTION_FIELD_HEIGHT` boxes. Nothing new is invented, so the card reads
as the same form as a login.

**Unchanged:**

- the Autofill card stays on card records (owner's decision);
- which fields exist and how they are saved;
- validation;
- the read pane.

## Testing

Use the existing edit-form layout harness (`edit_pane_layout_tests`):

- A Card draft draws three field rows plus Bank, in the order above.
- Brand sits left of the number on the same line.
- Month, year, code and ZIP share one line, with their centres aligned
  within 0.5 pt.
- Every trailing button's right edge lines up with the login form's trailing
  buttons.
- The Bank row's picker and Clear are vertically centred on the row.
- No Remove is drawn on the Expires row.
- Remove on the Card row clears the number and the brand.
- Empty month, year, code or ZIP saves nothing for that field.
