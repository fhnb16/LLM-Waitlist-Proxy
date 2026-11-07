---
name: unslop
description: Turns vibecoded, AI-looking UI into a designed interface with its own character. Finds a direction from the product itself, then fixes spacing, color, icons, copy, type, layout, details, visuals and motion in that order, and can generate a real hero visual with an image tool when one is available. Use when the user asks to unslop, polish, clean up or redesign a screen so it stops looking AI-generated, or when a UI shows the usual tells like purple gradients, glows, gradient text, emoji labels, "New ✨" badges, glossy blobs, uppercase tracked labels, pulsing elements or uneven spacing. Works on React, Tailwind, HTML and CSS, for dashboards, apps and landing pages.
license: CC-BY-4.0
metadata:
  author: Damir Sokolovsky
  x: https://x.com/heydamir
  site: https://heydamir.design/unslop
---

# Unslop

unslop was created by Damir Sokolovsky ([@heydamir](https://x.com/heydamir)), who started the Unslopping series on X. It is licensed under CC BY 4.0, so keep this credit in any copy or adaptation.

> Project note (LMQ): an abridged adaptation kept as a design rule for the repository. Landing-page heroes,
> image generation and the story video are left out, as are the `references/` and `scripts/` folders; the
> full skill is at https://heydamir.design/unslop. How the rules apply to the LMQ panel and native window is in AGENTS.md → «Дизайн интерфейса».

Vibecoded UI fails in the same predictable ways. Every surface glows, every label has an emoji, spacing is whatever the model typed that day, and the copy shouts. Unslopping fixes the craft and keeps the product. The features, content and order of sections stay. What changes is that every size, color, word and image becomes deliberate.

Removing slop is only half the job. A screen that is clean but could belong to any product is the next template. Every run ends with a screen that only this product could have.

The difference between slop and craft is where the richness goes. Slop spreads color and effects over the chrome, the backgrounds, buttons, borders and labels. Craft keeps the chrome quiet and puts the richness into one or two moments of content that people remember, like a rendered object, a beautiful chart or an image from the product's world.

Work like a designer doing a redline pass, one layer at a time, across the whole screen.

## What good looks like

- **One bright element.** Each screen has one polished thing that carries the color and the craft, a rendered object, a painted scene or a rich chart. Everything around it is quiet, so it reads instantly.
- **Quiet chrome.** Navigation, buttons and text use the tinted neutrals and one accent. Nothing glows, nothing competes with the bright element.
- **A clear idea.** You can say in one sentence what the visual shows and why it belongs to this product.
- **Text on calm space.** The headline sits on a part of the image or layout that was left empty for it, short and specific, with one primary and one secondary action and nothing stacked under them.
- **Real product, real material.** Previews look like the real app with plausible data, and objects look like real things with weight, light and shadow.

## Run it

1. **Look at the screen.** Find the files that render it. If you can run the app or open the file in a browser and take a screenshot, do it, so you judge what people see and not only the code.
2. **Audit.** List every sign of slop you find, one line each with where it lives.
3. **Decide how much to change.** Read the audit honestly before planning anything.
   - **Heavy slop**, many signs across color, type, icons and copy. Run the full process below.
   - **Light slop**, a screen that is mostly designed with a few signs left. Polish mode. Fix only what the audit found, take the direction from the existing brand instead of inventing a new one, and keep the palette, type, layout and visual. Bigger ideas go into the report as suggestions, not changes.
   - **No slop.** Say so plainly and suggest two or three small refinements at most, with exact values. Apply them only if the user agrees, and stop there. Never change a good screen just to make it different.
4. **Set the direction and stop.** Write the five lines described below. Show the audit and the direction to the user and wait for their answer before editing, since changing the direction later means redoing everything. Skip the wait only when the user already said to just do it.
5. **Fix in nine passes, in order.** Finish a pass across the whole screen before starting the next. Spacing comes first because everything else reads differently once the rhythm is right. Copy comes before type and layout because the length of the headline decides its size and where it sits. Visuals and motion come last because they should fit a screen that already works without them.
6. **Check.** Run the checklist at the end of this file, render again and compare with the audit list. Anything still on the list gets fixed.
7. **Report** the direction, a few lines per pass about what changed, and any image prompt you wrote.

Keep a run quick. Render the screen to check it at most three times.

## Direction

Before touching anything, write five short lines. Derive them from what the page says the product does and who it is for. Never from what is trending.

- **Product.** What it does and for whom, in one plain sentence.
- **Character.** Two or three concrete words, like `calm, exact, a little warm` or `fast, technical, playful`. Words like clean, modern, sleek, minimal and premium are not allowed, because they describe every template.
- **World.** The place or material the product naturally belongs to. This world sets the tint of the neutrals, the accent and the visual.
- **Moments.** One or two places where the screen gets rich color, material and detail, chosen from what the product shows.
- **Signature.** One small detail only this product would have, used consistently. One detail, not five.

If the product already has a real brand, like a logo file, a color or a typeface that is not slop, keep it and build the direction around it.

## 1. Spacing

- Everything sits on a 4px grid. Margins, paddings, gaps and fixed heights are multiples of 4. A 2px step is fine for nudges inside small controls, and an odd value that exactly centers something stays.
- Same thing, same size. All rows in a list share one height and one gap, all cards in a row share one padding.
- Space shows grouping. Space inside a group is smaller than space between groups.
- Things line up. Icons sit in one column, labels start on one line, numbers align right in one column.
- Padding follows density. Compact panels and sidebars around 16, cards 20 to 24, page edges 24 to 32.
- Navigation is the densest part of an app. Rows are 32 to 36 tall with 2 to 4 between them. A section label sits closer to its own items than to the section above it.
- Remove one-off values that break the rhythm and replace them with steps from the scale.

## 2. Color

- Build from one neutral ramp, tinted toward the world from the direction. Background, raised surfaces, borders and text all come from that ramp.
- In apps and dashboards the backgrounds, surfaces, borders and text stay neutral. The content has to carry the color.
- Three text roles, primary, secondary and muted.
- In the chrome, one accent, used rarely. It marks the primary action, the selection or the one number that matters.
- The moments from the direction are the exception. Inside them color can be rich, as long as each color means something.
- Accent with text on it reaches 4.5 to 1 contrast.
- Semantic colors only carry meaning, like green for a good change and red for an error, and they are toned down to sit inside the palette.
- Remove hue gradients on surfaces, gradient text, colored glows and colored shadows, radial color washes and blobs behind content, and neon borders.
- Borders are quiet hairlines, the text color of the ramp at 6 to 10% opacity.
- On dark themes depth comes from surface steps and hairlines, not from shadows. On light themes shadows are neutral, soft and small.
- A gradient is allowed only as a small tone shift inside one neutral. Never from one hue to another.

## 3. Icons

- Every emoji used as an icon or as decoration in a label becomes a line icon from one set. Use the set the project already has.
- One size and one stroke everywhere, 16 or 18px with a 1.5 stroke.
- Icons take the color of their text role, usually secondary or muted. They never get their own colors.
- The icon box has the same width in every row and the gap to the label is fixed, 8 or 10, so labels align.
- Drop icons that add nothing. Categories that need color get a small status dot.

## 4. Copy

- Headlines say what the product does for whom, in plain words, so they could not fit another product.
- No exclamation marks in interface text.
- No greeting headlines. The title names the context.
- Short labels. `Search`, not `Search anything...`.
- A change says what it compares to, like `+12% vs last month`.
- Headlines, subheads, labels, buttons and captions end without a period. Only paragraphs of several sentences keep their periods.
- No hype words like supercharge, seamless, unlock, magic, revolutionize or game-changing.
- Claims that cannot be checked become quiet and specific if the user confirms they are real, and are removed otherwise.
- Mock data looks real, with plausible numbers and names.
- Badges like `PRO`, `New ✨` or `Beta 🚀` go away unless the product really needs them, and then they are quiet text.

## 5. Type

- Choose type for the character in the direction. Keep the project's font when it is part of a real brand. One family for the interface, plus at most one partner, a mono for figures and meta or a display face for large headlines.
- A few sizes from a scale and two weights, 400 and 500, with 600 at most for rare emphasis. No 700 to 900 headings, no gradient text.
- Hierarchy comes from size and color, not from weight.
- Sentence case everywhere. No uppercase labels with wide tracking.
- Small text never gets negative letter spacing. Tight tracking is only for large display text.
- Numbers use `tabular-nums` and one format across the screen.
- No italic for emphasis.

## 6. Layout

- Keep the sections and their order. Inside a section, alignment and proportions may change when that makes the screen clearer.
- Apps and dashboards keep real app proportions. The shell fills the viewport, a sidebar of 240 to 280 and fluid content. Never shrink an app into a narrow centered column.
- Long headlines align left. Centered text works for one or two short lines.
- One primary action and at most one secondary action next to it.
- Navigation stays quiet.
- On small screens everything stacks in reading order. A dense preview gets simpler on mobile instead of being zoomed down. No text goes below 11px.

## 7. Details

- Nested radius. An inner radius equals the outer radius minus the padding between them, never less than 4. Controls 8 to 10, cards 12 to 16, panels 14 to 20. Not everything is a pill.
- Counts and meta become quiet text, often mono and muted, instead of glowing pills.
- Keyboard shortcuts become small keycaps.
- Charts sit on a muted grid with muted mono labels. No rainbow bars inside one series, no glowing bars, no uppercase day labels.
- Charts have a hierarchy. One main chart is clearly the biggest thing on the screen; anything else that shows data is smaller and different in kind.
- One accent per chart. Several hues only when the chart really has to tell series apart.
- Stat cards are numbers. A label, a big figure and a change with what it compares to. No sparklines, tallies, mini bars or icons added to fill them.
- Vary the sizes. Cards of equal width side by side, each with the same kind of content, read as a template.
- Pick the chart type for the data. A trend is a line, a comparison is bars, a share is one stacked bar or a list with bars, a single goal is a progress bar.
- The active state is a small surface step with a hairline highlight, not a gradient with a glow.
- Hover and focus states exist and stay inside the palette. A visible focus ring for keyboard users.
- The signature detail from the direction appears where it belongs and nowhere else.

## 8. Visuals

- Decorative filler goes. Glossy blobs, orbs, nebulas, floating cubes, glowing grids, holographic brains and robots say nothing about the product.
- Dashboards and app screens almost never need generated images. Their moments come from the product itself: a chart drawn with care, figures set beautifully, real content, a status that feels alive.

## 9. Motion

- Remove motion that loops for no reason, like pulse or ping on things that are not live, shimmer on headings, breathing glows, floating elements and parallax.
- Remove `transition: all`, bouncy springs, scale on hover for cards and anything that moves more than a few pixels on hover.
- Keep motion that explains something. A hover is a quick color or surface change, a press is a tiny scale down, a menu fades and rises a few pixels from where it opens, a changed number tweens.
- Transition color, background, border, opacity and transform, and name them. Never animate width, height, top, left, margins or large blurs.
- One easing curve and a few durations across the screen. Short for feedback, a little longer for things that enter.
- At most one entrance on load.

## Do not

- Do not rename the product. Keep a real logo as it is.
- Do not change what the product is. Keep its features, content and the order of sections unless the user asks for more.
- Do not replace removed decoration with new decoration, and do not swap one trend for another like glass, grain, noise, neon or bento grids.
- Do not invent features, claims or data.
- Do not fall back to the same look for every product.

## Checklist

- [ ] The direction is written and every pass follows it
- [ ] Every spacing value is on the 4px scale and equal things have equal sizes
- [ ] Icons, labels and numbers each sit in one aligned column
- [ ] One tinted neutral ramp, three text roles, one accent with enough contrast
- [ ] No hue gradients, gradient text, glows, colored shadows or color blobs
- [ ] No emoji left in the interface
- [ ] One icon set at one size and stroke
- [ ] Type chosen for the character, weights 400 to 600, no uppercase tracked labels, no italic, no negative tracking on small text
- [ ] No exclamation marks, no trailing periods on short strings, no greeting headline, no unverified claims
- [ ] Numbers in one format with tabular figures
- [ ] Nested radii add up
- [ ] The first screen is composed as a whole with no dead empty band
- [ ] No looping or bouncy motion, one easing curve, no animated layout properties
- [ ] Template check. If you swapped the product name for another product's, would the screen still fit? If yes, push the direction harder in the copy, the visual and the signature detail
