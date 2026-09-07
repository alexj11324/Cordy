# A refinement: solid symbol and narrower seams

Source mark is pure black. Both Composer foreground layers have glass disabled, no shadow or specular effect. Dark appearance overrides foreground to pure white. Background remains white/default and black/dark.

The two horizontal seams shrink from 4 to 2 SVG units: upper seam 39–41, lower seam 87–89. External extrema and glyph scale 0.90 are preserved; only local seam boundary control points change. Original A files remain unchanged.

Apple ictool rendered light.png and dark.png. Sampled interior pixels: light [0,0,0,255]; dark [255,254,255,255] after Apple rendering despite the pure-white source override. These are design previews, not an installed application change.
