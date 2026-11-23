import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";

const marker = 0xf8bf36;
const overlay = 0xf53ab5;
const page = 0x193f61;
const telemetry = { bits: 32, cell: 8, top: 32 };
const overlayRect = { width: 96, height: 40, top: 28, right: 24 };
const telemetryWordWidth = telemetry.bits * telemetry.cell;
const minWidth = 2 * telemetryWordWidth + overlayRect.width + overlayRect.right + 1;
const args = process.argv.slice(2);
const ready = args[0] === "--ready";

if (ready) args.shift();

const [scaleArg, logicalWidthArg, logicalHeightArg, screenWidthArg, screenHeightArg, ...files] = args;
const dimensions = [scaleArg, logicalWidthArg, logicalHeightArg, screenWidthArg, screenHeightArg].map(Number);
const [scale, logicalWidth, logicalHeight, screenWidth, screenHeight] = dimensions;

assert(dimensions.every((value) => Number.isSafeInteger(value) && value > 0));

assert(logicalWidth >= minWidth && logicalHeight > overlayRect.top + overlayRect.height,
  `viewport cannot contain the fixture probes (minimum width: ${minWidth})`);

assert(ready ? files.length === 1 : files.length >= 2, "unexpected capture count");

class PresentationPending extends Error {}

function presented(condition, message) {
  if (!condition) throw new PresentationPending(message);
}

function capture(file) {
  const [width, height] = execFileSync("magick", ["identify", "-format", "%w %h", file], { encoding: "utf8" })
    .split(" ").map(Number);

  const pixels = execFileSync("magick", [file, "-depth", "8", "rgb:-"], { maxBuffer: width * height * 3 });
  assert.equal(pixels.length, width * height * 3, file);

  const pixel = (x, y) => {
    assert(x >= 0 && x < width && y >= 0 && y < height, `${file}: pixel out of bounds at ${x}, ${y}`);

    return pixels.readUIntBE((y * width + x) * 3, 3);
  };

  let markerRight = -1;
  let markerBottom = -1;
  let overlayPixels = 0;

  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const color = pixels.readUIntBE((y * width + x) * 3, 3);

      if (color === marker) {
        markerRight = Math.max(markerRight, x);
        markerBottom = Math.max(markerBottom, y);
      }

      if (color === overlay) overlayPixels++;
    }
  }

  presented(markerRight >= 0 && markerBottom >= 0, `${file}: missing bottom-right page marker`);
  const viewportWidth = logicalWidth * scale;
  const viewportHeight = logicalHeight * scale;
  const left = markerRight + 1 - viewportWidth;
  const top = markerBottom + 1 - viewportHeight;
  presented(left >= 0 && top >= 0, `${file}: viewport is clipped`);

  function readValue(start, row) {
    let result = 0;

    const center = telemetry.cell / 2;

    for (let bit = 0; bit < telemetry.bits; bit++) {
      const cell = pixel(left + (start + bit * telemetry.cell + center) * scale,
        top + (telemetry.top + center + telemetry.cell * row) * scale);

      presented(cell === 0 || cell === 0xffffff, `${file}: invalid telemetry cell ${bit}: ${cell.toString(16)}`);

      if (cell !== 0) result += 2 ** bit;
    }

    return result;
  }

  const frame = readValue(0, 0);
  const dimensions = readValue(telemetryWordWidth, 0);
  presented(frame > 0, `${file}: the page has not drawn a counter`);
  presented(
    (dimensions >>> 16) === viewportWidth && (dimensions & 0xffff) === viewportHeight,
    `${file}: page reports a different size than ${viewportWidth}×${viewportHeight}`,
  );

  const screen = [readValue(0, 1), readValue(telemetryWordWidth, 1)]
    .map((value) => [value >>> 16, value & 0xffff]);

  const expectedScreen = [screenWidth, screenHeight];

  assert.deepEqual(screen[0], expectedScreen, `${file}: unexpected logical display size`);
  assert.deepEqual(screen[1], expectedScreen, `${file}: unexpected logical work-area size`);

  const overlayLeft = left + (logicalWidth - overlayRect.right - overlayRect.width) * scale;
  const overlayTop = top + overlayRect.top * scale;
  const overlayRight = overlayLeft + overlayRect.width * scale;
  const overlayBottom = overlayTop + overlayRect.height * scale;

  for (let y = overlayTop - 1; y <= overlayBottom; y++) {
    for (let x = overlayLeft - 1; x <= overlayRight; x++) {
      const inside = x >= overlayLeft && x < overlayRight && y >= overlayTop && y < overlayBottom;
      assert.equal(pixel(x, y), inside ? overlay : page, `${file}: overlay pixel at ${x}, ${y}`);
    }
  }

  assert.equal(pixel(left + 1, top + viewportHeight - 2), page, `${file}: bottom-left page pixel`);
  assert.equal(overlayPixels, (overlayRight - overlayLeft) * (overlayBottom - overlayTop),
    `${file}: overlay color appears outside its rectangle`);

  return { frame, left, top, screen };
}

let results;

try {
  results = files.map(capture);
} catch (error) {
  if (!ready || !(error instanceof PresentationPending)) throw error;
  console.log(error.message);
  process.exitCode = 75;
}

if (results) {
  for (let i = 1; i < results.length; i++) {
    assert(results[i].frame >= results[i - 1].frame, "captured counter moved backwards");
  }

  if (!ready) assert(results.at(-1).frame > results[0].frame, "captured counter did not advance");

  console.log(JSON.stringify({
    pixels: [logicalWidth * scale, logicalHeight * scale],
    frames: results.map((result) => result.frame),
    origin: [results[0].left, results[0].top],
    screen: results[0].screen,
  }));
}
