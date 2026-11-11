// Selection overlay: shows the frozen screen and lets the user drag a
// rectangle. Coordinates are converted to image pixels before they are
// sent to the app. Every monitor has its own overlay window; the one where
// the drag started tells the others the selection in image pixels, so a
// selection across monitors shows on each of them.

import { t, translate } from "./i18n.js";

const { invoke, convertFileSrc } = window.__TAURI__.core;
const { emit, listen } = window.__TAURI__.event;
const SELECTION_EVENT = "overlay-selection";
translate();
const canvas = document.getElementById("screen");
const box = document.getElementById("selection");
const size = document.getElementById("size");
const hint = document.getElementById("hint");
const dim = document.getElementById("dim");

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

const place = (r) => {
  dim.hidden = true;
  box.hidden = false;
  box.style.left = `${r.x}px`;
  box.style.top = `${r.y}px`;
  box.style.width = `${r.w}px`;
  box.style.height = `${r.h}px`;
};

// The selection in image pixels, clamped to the image, for the other
// overlays.
const imageRect = (r) => {
  const sx = info.source.width / window.innerWidth;
  const sy = info.source.height / window.innerHeight;
  const left = Math.max(0, info.source.x + r.x * sx);
  const top = Math.max(0, info.source.y + r.y * sy);
  const right = Math.min(info.image_width, info.source.x + (r.x + r.w) * sx);
  const bottom = Math.min(info.image_height, info.source.y + (r.y + r.h) * sy);
  return { x: left, y: top, w: Math.max(0, right - left), h: Math.max(0, bottom - top) };
};

// At most one event per 16 ms. A timer, not requestAnimationFrame:
// WebKitGTK may hold back animation frames during a drag across monitors.
let pending = null;
let sent = false;
const broadcast = (r) => {
  const first = pending === null;
  pending = imageRect(r);
  if (first) {
    setTimeout(() => {
      const rect = pending;
      pending = null;
      emit(SELECTION_EVENT, { from: label, rect })
        .then(() => {
          if (!sent) {
            sent = true;
            note(`${label} sends its selection to the other overlays`);
          }
        })
        .catch((err) => note(`${label}: sending the selection failed: ${err}`));
    }, 16);
  }
};

const show = (r) => {
  place(r);
  size.hidden = false;
  const px = toImage(r);
  size.textContent = `${px.width} × ${px.height}`;
  broadcast(r);
};

// A selection made on another monitor: show the part that lies on this
// one, or dim the whole screen if none does.
const showRemote = (rect) => {
  const s = info.source;
  const sx = window.innerWidth / s.width;
  const sy = window.innerHeight / s.height;
  const left = Math.max(0, (rect.x - s.x) * sx);
  const top = Math.max(0, (rect.y - s.y) * sy);
  const right = Math.min(window.innerWidth, (rect.x + rect.w - s.x) * sx);
  const bottom = Math.min(window.innerHeight, (rect.y + rect.h - s.y) * sy);
  start = null;
  current = null;
  hint.hidden = true;
  size.hidden = true;
  if (right <= left || bottom <= top) {
    box.hidden = true;
    dim.hidden = false;
    return;
  }
  place({ x: left, y: top, w: right - left, h: bottom - top });
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

const label = window.__TAURI__.window.getCurrentWindow().label;

(async () => {
  info = await invoke("overlay_info");
  let received = false;
  await listen(SELECTION_EVENT, (event) => {
    if (event.payload.from !== label && info) {
      if (!received) {
        received = true;
        note(`${label} shows the selection from ${event.payload.from}`);
      }
      showRemote(event.payload.rect);
    }
  });
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
