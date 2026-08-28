# Patchbay brand

This directory contains the approved Patchbay identity. Use these maintained
assets for public product surfaces; legacy internal identifiers are migrated at
their own compatibility boundaries.

## Idea

Eight jack sockets form an open routing matrix. A signal-lime cable carries work
from the upper-left port to the lower-right port, representing an issue moving
through people, agents, runtimes, and review without losing its context.

## Assets

| Asset | Use |
| --- | --- |
| `mark-color.svg` / `mark-color.png` | Default standalone mark on light or neutral surfaces |
| `mark-on-dark.svg` / `mark-on-dark.png` | Standalone mark on dark surfaces |
| `lockup-on-light.svg` / `lockup-on-light.png` | Horizontal wordmark on light surfaces |
| `lockup-on-dark.svg` / `lockup-on-dark.png` | Horizontal wordmark on dark surfaces |
| `preview.png` | Light/dark review sheet; not a production logo asset |

All production PNGs have a real alpha channel. SVG is the maintained source of
truth; PNGs are exports for surfaces that cannot consume SVG.

## Palette

- Ink: `#111111`
- Signal: `#B6F000`
- Warm white: `#F7F5F0`

## Usage

- Keep clear space of at least one jack-ring diameter around the mark.
- Use the ink version on light surfaces and the warm-white version on dark surfaces.
- Do not add glow, gradients, shadows, or a containing tile to the mark.
- Do not use the standalone mark below 24 px until its small-size geometry has
  been optically tuned and accepted.

The original direction was generated with the built-in image generation tool's
transparent-background mode, then redrawn as exact SVG geometry to remove raster
edge noise and make the identity maintainable.
