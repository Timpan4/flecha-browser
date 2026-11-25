// Corner checks for a page whose background fills its frame. A page in a card is rounded to
// --page-radius (ui/tokens.css), and a page with no card, alone in HTML fullscreen or in a popup
// window, stays square.
import { execFileSync } from 'node:child_process';

// The default Space's Light tint, which the window wears around its pages
// (flecha-ui/src/theme.rs). The harness sessions read the System theme as Light.
const tint = [0xcd, 0xd9, 0xf7];

/** The colour at the top-left corner of the bounding box of `color` in `path`. */
export function pageCorner(path, color) {
  // The black border keeps -trim working when the page reaches the capture's edge.
  const [x, y] = execFileSync('magick', [
    path, '-transparent', color, '-alpha', 'extract', '-negate',
    '-bordercolor', 'black', '-border', '1', '-trim', '-format', '%X %Y', 'info:',
  ], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] }).trim().split(' ').map(offset => Number(offset) - 1);

  if (!Number.isInteger(x) || !Number.isInteger(y) || x < 0 || y < 0) throw new Error(`no ${color} page in ${path}`);

  const hex = execFileSync('magick', [path, '-format', `%[hex:p{${x},${y}}]`, 'info:'],
    { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] });

  return { x, y, color: `#${hex.trim().slice(0, 6).toUpperCase()}` };
}

/**
 * Why the page in `color` is not rounded, if it is not. Outside the rounded corner the window's
 * tint shows, darkened by the card's shadow at most. The card's own background is lighter than
 * the tint, so a square background under a rounded page fails too.
 */
export function squareCorner(path, color) {
  const corner = pageCorner(path, color);
  const channels = [1, 3, 5].map(start => Number.parseInt(corner.color.slice(start, start + 2), 16));

  if (corner.color === color.toUpperCase() || channels.some((value, index) => value > tint[index])) {
    return `the ${color} page's top-left corner (${corner.x}, ${corner.y}) is ${corner.color}, not the window tint`;
  }
}

/** Why the page in `color` is not square, if it is not. */
export function roundedCorner(path, color) {
  const corner = pageCorner(path, color);

  if (corner.color !== color.toUpperCase()) {
    return `the ${color} page's top-left corner (${corner.x}, ${corner.y}) is ${corner.color}, not the page`;
  }
}
