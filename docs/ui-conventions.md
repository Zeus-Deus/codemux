# UI conventions

The design system Codemux already has, written down. Every token below lives
in `src/globals.css` and is theme-driven: change the palette or the preset and
the whole app re-skins. Nothing here is a colour decision — these are the
scales that decide *size*, *corner*, *fill* and *rhythm*.

The rule that makes all of it worth having: **call sites consume token names,
never raw values.** A raw `text-xs` or `rounded-[7px]` looks identical today
and then fails to move when the user changes the interface size or the theme's
radius — which is exactly the bug class this file exists to prevent.

## Type scale

`@theme` in `src/globals.css`. Every step is rem, so the interface-size
setting (and its `Ctrl+=` / `Ctrl+-` shortcuts) scales the whole app.

| Token           | Size      | Use                                  |
| --------------- | --------- | ------------------------------------ |
| `text-micro`    | 0.625rem  | 10px — tight badges, keycaps         |
| `text-caption`  | 0.6875rem | 11px — mono meta, uppercase eyebrows |
| `text-label`    | 0.75rem   | 12px — secondary labels, counts      |
| `text-body-sm`  | 0.8125rem | 13px — dense rows, menus             |
| `text-body`     | 0.875rem  | 14px — primary UI text               |
| `text-body-lg`  | 0.9375rem | 15px — emphasized UI text            |

10px is the floor. Anything smaller is unreadable on a 1× desktop display.

Do not use Tailwind's stock `text-xs` / `text-sm` / `text-base`: they are
fixed pixel values that ignore the interface-size setting. `text-xs` →
`text-label`, `text-sm` → `text-body`, `text-base` → `text-body-lg`.

## Radius ladder

`@theme inline` derives the whole ladder from the theme's `--radius`, so a
theme that asks for square corners gets them everywhere.

| Token         | Use                                            |
| ------------- | ---------------------------------------------- |
| `rounded-sm`  | anything under ~20px tall: badges, chips, keycaps, dots |
| `rounded-md`  | controls and cards: buttons, inputs, list rows, cards   |
| `rounded-lg`  | dialogs, popovers, panels                      |

Three steps is the whole vocabulary. Two rules:

1. **A child's radius is never larger than its parent's.** That single rule is
   what makes nested surfaces look machined instead of accidental.
2. **Bare `rounded` is not a token** — it compiles to a hard 4px that no theme
   can reach. Use `rounded-sm`.

Sanctioned exceptions, and nothing else: `rounded-[inherit]` (structural, not
decorative), `rounded-none` (deliberate), `rounded-full` on true circles
(avatars, status dots), the two asymmetric chat-bubble tail radii, and the
composer strip/pill seam. Where an optical cap is genuinely needed, clamp
instead of hardcoding, so the theme stays in control of the curve:

```
rounded-[min(var(--radius-md),10px)]
```

## Surface alpha ladder

Three steps of neutral ink over the current surface, plus two hairlines. The
alpha applies to `--foreground`, so light themes get dark ink and dark themes
get light ink with no per-scheme rule.

| Token                    | Alpha | Use                                     |
| ------------------------ | ----- | --------------------------------------- |
| `bg-surface-1`           | 3%    | resting fill — a card that must read as a card |
| `bg-surface-2`           | 6%    | hover                                   |
| `bg-surface-3`           | 9.5%  | active / selected                       |
| `border-hairline`        | 7%    | default rule between rows and sections  |
| `border-hairline-strong` | 11%   | a rule that has to survive next to a fill |

Improvised alphas (`bg-foreground/[0.055]`) are the thing this replaces: they
made hover in one panel a different jump from hover in the next. Text alphas
are a separate axis and deliberately not part of this ladder.

## Section labels (eyebrows)

One component — `Eyebrow` in `src/components/ui/eyebrow.tsx` — locks size
(`text-caption`), family (mono), weight, case and `tracking-eyebrow` (0.1em).
Uppercase mono at 11px needs the letters opened up or it reads as a smudge.
Use the component; do not re-roll the five classes.

## Control geometry

| Size            | Height | Use                                 |
| --------------- | ------ | ----------------------------------- |
| `Button` `xs`   | 24px   | inline, inside dense text rows      |
| `Button` `sm`   | 32px   | toolbars, panel headers, dense rows |
| `Button` default| 36px   | standard actions                    |
| `Button` `lg`   | 40px   | primary actions in dialogs          |

Panel headers come in two documented heights, from one primitive —
`PanelHeader` in `src/components/ui/panel-header.tsx`:

- `variant="floating"` — 40px, GUI chrome that overlaps content
- `variant="inline"` — 36px, in-flow, carries a bottom border

If a call site has to patch a primitive's geometry from the outside, that is a
bug in the primitive, not a local fix: 64% of `size="sm"` callers used to
override their own height, which is how `sm` came to mean four things.

## Icons

Three sizes — `size-3`, `size-3.5`, `size-4` — one spelling (`size-*`, never
`h-4 w-4`), and one stroke weight. Icons set the optical weight of every row;
thirteen sizes and fifteen stroke weights make alignment unachievable even
when the boxes agree. True circles (avatars, status dots) and brand logos are
carved out.

## Motion

Three durations, always explicit:

- `duration-100` — controls (hover, press)
- `duration-150` — surfaces (panels, rows, cards)
- `duration-250` — overlays (dialogs, popovers, sheets)

An implicit duration silently inherits 150ms, which is how eleven distinct
durations ended up in one window. Name the properties that transition rather
than using `transition-all`, and gate anything that loops on `motion-safe:`.

## Focus

One base-layer `:focus-visible` rule in `src/globals.css` gives every
focusable element the same outline. Do not suppress it with `outline-none`
unless focus is genuinely shown some other way — a dialog that takes focus as
a whole, a command list whose selected row is filled, a bare input inside a
container that carries `focus-within:`. Suppressing it *and* adding nothing is
how 49 controls became unreachable by keyboard.

When the ring has to hug a different shape, one recipe layers on top:

```
focus-visible:ring-2 focus-visible:ring-ring/60
```

## Scrollbars

Two utilities, one intent each:

- `thin-scrollbar` — a thin, quiet, always-present bar for a list that is
  tall enough to keep one. It also reserves its gutter (`scrollbar-gutter:
  stable`), so a growing list does not shove its own content sideways.
- `no-scrollbar` — hides the bar outright, for a tab strip or rail that
  scrolls by drag and wheel. It is a real utility from `shadcn/tailwind.css`;
  a comment once claimed otherwise and three hand-rolled copies followed.

Do not hand-roll `[scrollbar-width:…]` plus a `::-webkit-scrollbar` block:
the two render differently on WebKitGTK, which is the engine this app ships
on.

## Numbers

Anything that ticks — timers, counts, sizes, percentages — carries
`tabular-nums`, so a `0s → 1m` change does not nudge its neighbours sideways.

## The guardrail

`src/lib/ui-token-contract.test.ts` counts violations per category and fails
if any count rises. It is a Vitest contract test (the repo has no ESLint, and
one rule does not justify a linting toolchain), modelled on
`src/lib/theme-color-contract.test.ts`. Adding a raw `text-sm` to a component
fails the suite and names the file.

Tailwind v4 only generates CSS for classes it can see in the source, and jsdom
never loads that CSS — so a migration can pass every test and still ship an
unstyled surface. After any token migration, build and check the compiled CSS
actually contains the new utilities:

```
npm run build
grep -o 'surface-[123]\|hairline' dist/assets/*.css | sort -u
```
