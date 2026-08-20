# Ingot brand assets

The repository README expects two theme-aware logo files in this directory:

- `ingot-lang-light.png` — artwork for GitHub's light theme
- `ingot-lang-dark.png` — artwork for GitHub's dark theme

GitHub selects between them through the README's `<picture>` element. Keep the
names exactly as written above.

Before committing, crop the large empty margin around the character and export
each image at no more than roughly 1,600 px on its longest side. The README
renders it at 360 px wide, so the original 6,250 × 6,250 canvas would add a lot
of repository weight without looking sharper on the page.
