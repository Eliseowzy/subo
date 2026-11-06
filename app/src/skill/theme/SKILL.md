---
name: subo-theme
description: Create or modify color themes (.subo-theme.json) for Subo, the offline LaTeX workbench. Use when the user wants a custom Subo look — colors, fonts, corner radius, light and dark mode.
---

# Subo color themes

Subo (素帛) is an offline desktop LaTeX editor. A color theme is a plain JSON file
(`<id>.subo-theme.json`) that overrides Subo's design tokens: colors, fonts, radii, shadows and
editor syntax colors. Users import it in **Settings → General → Color themes → Import**.

Talk with the user in their own language. Deliver a single JSON file they can save and import.
Interface languages are a separate skill (`subo-locale`); this one only covers themes.

## Files in this skill

```
reference/default.subo-theme.json   the built-in theme, every token with its current value
scripts/validate.mjs                checks a theme exactly like Subo's importer does
```

The reference files were exported from the user's own copy of Subo, so they match their version.
If the user gives you a theme they exported from Subo, start from that file instead.

Always finish by running the validator (Node 18+, no dependencies):

```bash
node scripts/validate.mjs path/to/theme.subo-theme.json
```

Fix every error. Warnings mean something will be dropped on import; fix those too unless the
user agrees. If you cannot run code, check the rules below by hand.

## Color theme format

```json
{
  "format": "subo-theme",
  "version": 1,
  "id": "nord",
  "name": "Nord",
  "author": "optional",
  "description": "optional, one sentence",
  "base":  { "font-ui": "...", "r": "8px" },
  "light": { "accent": "#5e81ac", "bg": "#eceff4" },
  "dark":  { "accent": "#88c0d0", "bg": "#2e3440" }
}
```

- `id`: letters, digits, `-` `_` `.`, starts with a letter or digit, ≤ 64 chars. Not `default`.
  Importing a pack with an existing id replaces it, so reuse the id when revising.
- `base` applies in both modes, `light` only in light mode, `dark` only in dark mode.
  Every section is optional. A token you leave out keeps Subo's built-in value, so a theme can
  be just a few lines (for example, only `accent*` in both modes).
- Token names are written without the leading `--`. Only names that appear in
  `reference/default.subo-theme.json` are accepted. Unknown names are dropped.
- Values are plain CSS values: colors (`#hex`, `rgb()`, `hsl()`, `oklch()`), lengths, font stacks,
  shadows. **Not allowed**: `url(...)`, `@import`, `;`, `{`, `}`, `<`, `>`, backslashes. Subo is
  offline, so web fonts cannot load. Use font names installed locally, with generic fallbacks.
- The user picks Light / Dark / System separately. Design **both** `light` and `dark` unless
  they explicitly want only one.

### Token meanings

| Tokens | Role |
|---|---|
| `bg` | Window background (behind panels) |
| `panel` | Chrome: sidebars, file tree, pane bars, toolbars — one step darker than the writing surface |
| `raised` | Cards, menus, dialogs, setting groups |
| `sunken` | Inset wells (path fields, code blocks, PDF area) — the deepest surface |
| `field` | Text input background |
| `hover`, `press` | Translucent overlays for hovered / pressed items |
| `editor-bg`, `editor-active`, `editor-selection`, `editor-match` | Writing surface: code editor and Markdown preview background, current line, selection, search matches |
| `ink`, `ink-2`, `ink-3`, `ink-4` | Text: primary → secondary → hints and placeholders → disabled text and decorative icons only (never text people need to read) |
| `line`, `line-strong` | Hairline borders, stronger dividers |
| `field-line` | Bottom border of text inputs, strong enough to tell an input apart from its surroundings |
| `accent`, `accent-hover`, `accent-soft`, `accent-line`, `on-accent` | The single emphasis color: primary buttons, selection, focus; its hover state, tinted background, border; text drawn on top of `accent` |
| `accent-text` | `accent` used as text (links, selected labels). Defaults to `accent` mixed 15% toward `ink`, so it follows your accent; set it only if that mix falls short of 4.5:1 |
| `danger`, `danger-soft`, `danger-line` | Errors and destructive actions |
| `warn`, `warn-dot`, `warn-soft`, `warn-line` | "Something needs attention" (missing packages, warnings) |
| `header`, `header-ink`, `header-dim`, `header-hover`, `header-line`, `header-edge` | The top bar of the workspace: background, text, dim text, hover overlay, separators, bottom edge |
| `header-accent`, `header-accent-hover`, `header-on-accent` | The Compile button on the top bar (it sits on `header`, not on `bg`, so it has its own accent) |
| `header-ok`, `header-busy`, `header-danger`, `header-badge`, `header-on-badge` | Compile status text on the top bar (done / saving / failed), the error-count badge and its text |
| `header-mark-ink`, `header-mark-dot` | The Subo logo on the top bar: the backslash and the dot (also the compile status light) |
| `mark-tile`, `mark-ink`, `mark-dot`, `mark-shadow` | The Subo logo on the home and onboarding screens: the tile behind it, the backslash, the dot, the tile's `box-shadow` (light theme uses a light tile, dark theme a dark one) |
| `syn-command`, `syn-arg`, `syn-math`, `syn-number`, `syn-bracket`, `syn-comment` | LaTeX syntax colors: `\commands`, `{arguments}`, `$math$`, numbers, brackets, `% comments` |
| `sidebar`, `toolbar`, `card`, `popover`, `control`, `preview-bg` | **Region tokens** (see below): file tree and settings nav; pane bars; inline cards (setting groups, template cards); menus, palette, toasts, dialogs; buttons and keycaps; the PDF area |
| `home-side`, `home-main`, `home-list` | Region tokens of the home screen: left column, right side, the recent-projects list |
| `shadow-sm`, `shadow`, `shadow-lg`, `page-shadow`, `backdrop` | Elevation shadows, PDF page shadow, modal backdrop |
| `font-ui`, `font-mono`, `editor-size` | Interface font stack, code font stack, editor font size |
| `fs-2xs`, `fs-xs`, `fs-sm`, `fs-md`, `fs-lg`, `fs-xl`, `fs-2xl` | Interface type scale (11 / 12 / 13 / 14 / 16 / 20 / 24 px by default). `fs-sm` is body text; raise the whole scale together if you want a larger interface |
| `r-xs`, `r-sm`, `r`, `r-lg`, `r-xl`, `r-pill` | Corner radii: keycaps and tags / small controls / buttons and menu items / cards and dialogs / large floating cards / pills |
| `ease`, `ease-exit` | Animation easing for things arriving (decelerate) and leaving (accelerate) |

### Surface tokens vs. region tokens

Components never pick a surface level directly; they read a **region token**, and every region
token defaults to a surface token (`sidebar` → `var(--panel)`, `card` → `var(--raised)`,
`home-list` → `var(--card)`, `home-side` → `var(--sidebar)`, `home-main` → `var(--bg)`, …).
So a theme normally only sets the surface tokens and every region follows. Override a region
token only when one place needs to differ, e.g. a calmer home list in dark mode:

```json
"dark": { "home-list": "#211c16" }
```

A region token may itself be `var(--some-token)` or a `color-mix()`, for example
`"popover": "var(--editor-bg)"`. The defaults are in `reference/default.subo-theme.json`: most sit
under `base`; `card` and `control` sit under `light` / `dark` because in dark mode they are mixed
halfway between `raised` and the background, so inline cards don't glare like menus do.

### Design checklist

1. Settle on a palette first: background family, one accent, and the semantic danger / warn hues.
2. Contrast (check every surface: `bg`, `panel`, `raised`, `sunken`, `editor-bg`):
   `ink` ≥ 7:1; `ink-2` and `ink-3` ≥ 4.5:1 (hints are still text people read);
   `accent-text`, `danger` and `warn` ≥ 4.5:1, also on their own `*-soft` backgrounds;
   `field-line` on `field` ≥ 3:1; `on-accent` on `accent` ≥ 4.5:1; `header-ink` on `header` ≥ 7:1;
   `header-on-accent` on `header-accent` ≥ 4.5:1; every `syn-*` on `editor-bg` ≥ 4.5:1
   (only `syn-bracket` may go down to 3:1). `ink-4` is exempt because it is only used for disabled
   states and decorative icons.
3. Surfaces must be distinguishable: `bg`, `panel`, `raised`, `sunken` usually differ by a few
   lightness steps. The stacking order, in light and dark alike, is
   `sunken` < `bg` < `panel` < `editor-bg` ≤ `raised`. The sidebar (`panel`) and the writing
   surface (`editor-bg`) sit side by side, so `editor-bg` must be clearly lighter than `panel`
   (contrast ratio ≈ 1.1–1.2, i.e. CIEDE2000 ΔE00 ≥ 3), never equal or darker. Keep
   `hover`/`press` translucent.
4. `accent-soft` is a faint tint (a solid light color, or rgba at about 0.12 alpha in dark mode).
   `accent-line` sits between soft and full.
5. Keep danger reddish and warn amber/yellow in every theme. Users rely on those meanings.
6. Dark mode is a separate design, not an inversion. Lower saturation, lighter accents, and
   deeper shadows (`rgba(0,0,0,.4+)`).
7. Changing `font-ui`/`font-mono` is allowed, but keep CJK fallbacks such as `"PingFang SC"` and
   `"Microsoft YaHei"` in `font-ui` because Subo users write Chinese.

## Hand-off

Give the user the file and say:

1. Save it as `<id>.subo-theme.json`.
2. In Subo: **Settings → General → Color themes → Import**. The theme is activated right away,
   and the list shows a light and a dark preview of it. Re-importing the same id updates it.
3. To share or back up all imported themes, use **Export all** in the same section.
