// Corner checks for a page whose background fills its frame. A page in a card is rounded to
// --page-radius (ui/tokens.css), and a page with no card, alone in HTML fullscreen or in a popup
// window, stays square. The harness displays run at scale 1, so a logical pixel is one pixel.
import { execFileSync } from 'node:child_process';

// The default Space's Light tint, which the window wears around its pages
// (flecha-ui/src/theme.rs). The harness sessions read the System theme as Light.
const tint = [0xcd, 0xd9, 0xf7];

// The page area's Light drop shadow (flecha-ui/src/tokens.rs page_shadow): Flecha's ink in two
// layers at 0x1a and 0x0f alpha. Where both layers reach full strength, the tint is darkest.
const ink = [11, 37, 69];

const darkest = tint.map((value, index) => Math.floor([0x1a, 0x0f]
  .reduce((under, alpha) => under + (ink[index] - under) * alpha / 255, value)));

// tokens::PAGE_RADIUS. The arc crosses each corner's diagonal radius × (1 − 1/√2) pixels in,
// so a pixel a step short of that is outside the card and a pixel a step past it is inside.
const radius = 12;
const crossing = radius * (1 - Math.SQRT1_2);
const outsideStep = Math.floor(crossing) - 1;
const insideStep = Math.ceil(crossing) + 1;

const magick = args => execFileSync('magick', args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] }).trim();

/** The bounding box of `color` in `path`, with each corner's direction into the page, if any. */
function corners(path, color) {
  // The black border keeps -trim working when the page reaches the capture's edge.
  const [width, height, x, y] = magick([
    path, '-transparent', color, '-alpha', 'extract', '-negate',
    '-bordercolor', 'black', '-border', '1', '-trim', '-format', '%w %h %X %Y', 'info:',
  ]).split(' ').map(Number);

  const [left, top] = [x - 1, y - 1];

  // A capture without the colour trims to nothing, which magick reports at offset -1.
  if (![width, height, left, top].every(Number.isInteger) || left < 0 || top < 0) return undefined;

  const [right, bottom] = [left + width - 1, top + height - 1];

  return [
    { name: 'top-left', x: left, y: top, dx: 1, dy: 1 },
    { name: 'top-right', x: right, y: top, dx: -1, dy: 1 },
    { name: 'bottom-right', x: right, y: bottom, dx: -1, dy: -1 },
    { name: 'bottom-left', x: left, y: bottom, dx: 1, dy: -1 },
  ];
}

/** The colours at `points` in `path`, as #RRGGBB. */
function colors(path, points) {
  return magick([path, '-depth', '8', '-format', points.map(([x, y]) => `%[hex:p{${x},${y}}]`).join(' '), 'info:'])
    .split(' ').map(hex => `#${hex.slice(0, 6).toUpperCase()}`);
}

/** Whether `hex` is the window's tint, darkened by the card's shadow at most. */
function windowTint(hex) {
  const channels = [1, 3, 5].map(start => Number.parseInt(hex.slice(start, start + 2), 16));

  return channels.every((value, index) => value <= tint[index] && value >= darkest[index]);
}

/**
 * Why the page in `color` is not rounded to the page radius, if it is not. At each corner of the
 * page's box, the corner pixel and a pixel just outside the arc show the window's tint, and a
 * pixel just inside the arc shows the page. The card's own background is lighter than the tint,
 * and anything darker than the full shadow is not the window, so a square background under a
 * rounded page or a corner filled with another colour fails too.
 */
export function cornerNotRounded(path, color) {
  const page = color.toUpperCase();

  const box = corners(path, color);

  if (!box) return `no ${color} page in ${path}`;

  for (const { name, x, y, dx, dy } of box) {
    const [corner, outside, inside] = colors(path, [
      [x, y], [x + dx * outsideStep, y + dy * outsideStep], [x + dx * insideStep, y + dy * insideStep],
    ]);

    if (corner === page || !windowTint(corner)) return `the ${color} page's ${name} corner (${x}, ${y}) is ${corner}, not the window tint`;

    if (outside === page || !windowTint(outside)) return `the ${color} page's ${name} corner is ${outside} ${outsideStep}px in, not the window tint: its radius is under ${radius}px`;

    if (inside !== page) return `the ${color} page's ${name} corner is ${inside} ${insideStep}px in, not the page: its radius is over ${radius}px`;
  }
}

/** Why the page in `color` is not square, if it is not. */
export function cornerNotSquare(path, color) {
  const page = color.toUpperCase();
  const box = corners(path, color);

  if (!box) return `no ${color} page in ${path}`;

  const found = colors(path, box.map(({ x, y }) => [x, y]));
  const index = found.findIndex(hex => hex !== page);

  if (index >= 0) return `the ${color} page's ${box[index].name} corner (${box[index].x}, ${box[index].y}) is ${found[index]}, not the page`;
}
