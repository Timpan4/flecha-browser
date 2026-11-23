import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";

const marker = 0xf8bf36;
const overlay = 0xf53ab5;
const page = 0x193f61;
const args = process.argv.slice(2);
const ready = args[0] === "--ready";
if (ready) args.shift();
const [scaleArg, logicalWidthArg, logicalHeightArg, ...files] = args;
const [scale, logicalWidth, logicalHeight] = [scaleArg, logicalWidthArg, logicalHeightArg].map(Number);
assert([scale, logicalWidth, logicalHeight].every((value) => Number.isSafeInteger(value) && value > 0));
assert(ready ? files.length === 1 : files.length >= 2, "unexpected capture count");

function capture(file) {
  const [width, height] = execFileSync("magick", ["identify", "-format", "%w %h", file], { encoding: "utf8" })
    .split(" ").map(Number);
  const pixels = execFileSync("magick", [file, "-depth", "8", "rgb:-"], { maxBuffer: width * height * 3 });
  assert.equal(pixels.length, width * height * 3, file);
  const pixel = (x, y) => pixels.readUIntBE((y * width + x) * 3, 3);

  let markerRight = -1;
  let markerBottom = -1;
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      if (pixel(x, y) === marker) {
        markerRight = Math.max(markerRight, x);
        markerBottom = Math.max(markerBottom, y);
      }
    }
  }
  assert(markerRight >= 0 && markerBottom >= 0, `${file}: missing bottom-right page marker`);
  const viewportWidth = logicalWidth * scale;
  const viewportHeight = logicalHeight * scale;
  const left = markerRight + 1 - viewportWidth;
  const top = markerBottom + 1 - viewportHeight;
  assert(left >= 0 && top >= 0, `${file}: viewport is clipped`);

  function readValue(start) {
    let result = 0;
    for (let bit = 0; bit < 32; bit++) {
      const cell = pixel(left + (start + bit * 8 + 4) * scale, top + 36 * scale);
      assert(cell === 0 || cell === 0xffffff, `${file}: invalid telemetry cell ${bit}: ${cell.toString(16)}`);
      if (cell !== 0) result += 2 ** bit;
    }
    return result;
  }
  const frame = readValue(0);
  const dimensions = readValue(256);
  assert(frame > 0, `${file}: the page has not drawn a counter`);
  assert.equal(dimensions >>> 16, viewportWidth, `${file}: innerWidth × DPR`);
  assert.equal(dimensions & 0xffff, viewportHeight, `${file}: innerHeight × DPR`);

  const overlayLeft = left + (logicalWidth - 24 - 96) * scale;
  const overlayTop = top + 28 * scale;
  const overlayRight = overlayLeft + 96 * scale;
  const overlayBottom = overlayTop + 40 * scale;
  for (let y = overlayTop - 1; y <= overlayBottom; y++) {
    for (let x = overlayLeft - 1; x <= overlayRight; x++) {
      const inside = x >= overlayLeft && x < overlayRight && y >= overlayTop && y < overlayBottom;
      assert.equal(pixel(x, y), inside ? overlay : page, `${file}: overlay pixel at ${x}, ${y}`);
    }
  }
  assert.equal(pixel(left + 1, top + viewportHeight - 2), page, `${file}: bottom-left page pixel`);
  return { frame, left, top };
}

const results = files.map(capture);
for (let i = 1; i < results.length; i++) {
  assert(results[i].frame >= results[i - 1].frame, "captured counter moved backwards");
}
if (!ready) assert(results.at(-1).frame > results[0].frame, "captured counter did not advance");
console.log(JSON.stringify({
  pixels: [logicalWidth * scale, logicalHeight * scale],
  frames: results.map((result) => result.frame),
  origin: [results[0].left, results[0].top],
}));
