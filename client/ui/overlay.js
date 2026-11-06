// Selection overlay: shows the frozen screen and lets the user drag a
// rectangle. Coordinates are converted to image pixels before they are
// sent to the app.

import { t, translate } from "./i18n.js";

const { invoke, convertFileSrc } = window.__TAURI__.core;
translate();
const canvas = document.getElementById("screen");
const box = document.getElementById("selection");
const size = document.getElementById("size");
const hint = document.getElementById("hint");

let info = null;
let start = null;
let current = null;

const note = (line) => invoke("note", { line }).catch(() => {});

// Selection in CSS pixels -> rectangle in image pixels.
const toImage = (r) => {
  const sx = info.source.width / window.innerWidth;
  const sy = info.source.height / window.innerHeight;
  const x = Math.round(info.source.x + r.x * sx);
  const y = Math.round(info.source.y + r.y * sy);
  return {
    x,
    y,
    width: Math.max(1, Math.min(info.image_width - x, Math.round(r.w * sx))),
    height: Math.max(1, Math.min(info.image_height - y, Math.round(r.h * sy))),
  };
};

const rectOf = (a, b) => ({
  x: Math.min(a.x, b.x),
  y: Math.min(a.y, b.y),
  w: Math.abs(a.x - b.x),
  h: Math.abs(a.y - b.y),
});

const draw = (image) => {
  const dpr = window.devicePixelRatio || 1;
  canvas.width = Math.round(window.innerWidth * dpr);
  canvas.height = Math.round(window.innerHeight * dpr);
  const ctx = canvas.getContext("2d");
  ctx.imageSmoothingQuality = "high";
  const s = info.source;
  ctx.drawImage(image, s.x, s.y, s.width, s.height, 0, 0, canvas.width, canvas.height);
};

const show = (r) => {
  box.hidden = false;
  box.style.left = `${r.x}px`;
  box.style.top = `${r.y}px`;
  box.style.width = `${r.w}px`;
  box.style.height = `${r.h}px`;
  const px = toImage(r);
  size.textContent = `${px.width} × ${px.height}`;
};

const finish = async () => {
  if (!current || current.w < 3 || current.h < 3) {
    return;
  }
  const rect = toImage(current);
  try {
    await invoke("finish_selection", { rect });
  } catch (err) {
    hint.hidden = false;
    hint.textContent = t("editor.error", err);
  }
};

window.addEventListener("pointerdown", (event) => {
  if (event.button !== 0 || !info) {
    return;
  }
  hint.hidden = true;
  start = { x: event.clientX, y: event.clientY };
  current = rectOf(start, start);
  show(current);
});

window.addEventListener("pointermove", (event) => {
  if (!start) {
    return;
  }
  current = rectOf(start, { x: event.clientX, y: event.clientY });
  show(current);
});

window.addEventListener("pointerup", () => {
  if (!start) {
    return;
  }
  start = null;
  finish();
});

window.addEventListener("keydown", (event) => {
  if (event.key === "Escape") {
    invoke("cancel_selection");
  } else if (event.key === "Enter") {
    finish();
  }
});

(async () => {
  info = await invoke("overlay_info");
  note(
    `css=${window.innerWidth}x${window.innerHeight} dpr=${window.devicePixelRatio} ` +
      `screen=${window.screen.width}x${window.screen.height} image=${info.image_width}x${info.image_height}`,
  );
  const image = new Image();
  image.onload = () => draw(image);
  image.onerror = () => note("loading the capture failed");
  image.src = convertFileSrc("capture.png", "scrn");
  window.addEventListener("resize", () => image.complete && draw(image));
})();
