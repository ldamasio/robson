# Personal Site — UI Kit

Interactive click-through prototype for Leandro Damasio's personal site.
Three screens: Home, Work Detail, Notes.

## Screens

| Screen | Component | Description |
|---|---|---|
| Home | `Home.jsx` | Hero + Selected Work grid + Capabilities + Writing + Contact + Footer |
| Work detail | `WorkDetail.jsx` | Project header, prose column (42rem), metrics sidebar, stack |
| Notes index | `Notes.jsx` | Editorial listing + Note detail (full typographic read) |

## Interactions

- Work cards are clickable — navigate to Work Detail
- Writing rows are clickable — navigate to Note Detail
- Nav items route between all screens
- EN / PT-BR toggle in nav simulates dark/light theme switch (placeholder for actual i18n)
- Back buttons return to prior context

## Stack (production target)

Next.js 14 + Tailwind CSS + shadcn/ui + Geist (or Inter) + Geist Mono (or JetBrains Mono)

## Design decisions

- **Ambiguity preserved:** "Available for selected engagements" is the only CTA-adjacent copy.
  No "Hire me", no "Buy now", no agency-speak.
- **Work cards:** hover reveals `--accent-personal` (steel blue `#5C7080`) border.
  Primary accent (brass `#D9CBA3`) reserved for metric callouts in work detail.
- **Portrait:** placeholder square — replace with B&W crop of actual photo.
- **Notes tag color:** `#5C7080` (personal accent) used as the tag color — small, structural, not decorative.
