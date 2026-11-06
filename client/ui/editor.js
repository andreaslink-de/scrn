// Editor: draws arrows, highlights and blurred areas onto the selected
// area. Shapes live in image pixels; the canvas holds the image at full
// size and is only scaled down by CSS to fit the window.

import { t, translate } from "./i18n.js";

const { invoke } = window.__TAURI__.core;

const COLORS = [
  ["red", "#e53935"],
  ["orange", "#fb8c00"],
  ["green", "#43a047"],
  ["blue", "#1e88e5"],
  ["black", "#111111"],
  ["white", "#ffffff"],
];
const TOOLS = ["arrow", "mark", "blur"];
const MARK = "rgba(255, 221, 0, 0.42)";

const canvas = document.getElementById("canvas");
const ctx = canvas.getContext("2d");
const stage = document.getElementById("stage");
const status = document.getElementById("status");
const undoButton = document.getElementById("undo");
const saveButton = document.getElementById("save");
const colorGroup = document.querySelector(".colors");

let image = null;
let tool = "arrow";
let color = COLORS[0][1];
let shapes = [];
let drawing = null;
let busy = false;
let discardArmed = 0;
// Set from the app: with a server, saving uploads the image.
let uploads = false;

// Sizes in image pixels. The selection has the pixel density of the
// screen, so lines are scaled with it to look the same on HiDPI screens.
const dpr = () => Math.max(1, window.devicePixelRatio || 1);
const lineWidth = () => Math.round(4 * dpr());
const blurBlock = () => Math.round(12 * dpr());

const setStatus = (text) => {
  status.textContent = text;
};

// --- Drawing -------------------------------------------------------------

const drawArrow = (s) => {
  const lw = s.width;
  const angle = Math.atan2(s.y2 - s.y1, s.x2 - s.x1);
  const length = Math.hypot(s.x2 - s.x1, s.y2 - s.y1);
  const head = Math.min(lw * 5, length * 0.6);
  const spread = Math.PI / 7;
  // The line stops inside the head so its round cap does not show.
  const stop = Math.max(0, length - head * 0.8);
  ctx.save();
  ctx.strokeStyle = s.color;
  ctx.fillStyle = s.color;
  ctx.lineWidth = lw;
  ctx.lineCap = "round";
  ctx.lineJoin = "round";
  ctx.beginPath();
  ctx.moveTo(s.x1, s.y1);
  ctx.lineTo(s.x1 + Math.cos(angle) * stop, s.y1 + Math.sin(angle) * stop);
  ctx.stroke();
  ctx.beginPath();
  ctx.moveTo(s.x2, s.y2);
  ctx.lineTo(s.x2 - head * Math.cos(angle - spread), s.y2 - head * Math.sin(angle - spread));
  ctx.lineTo(s.x2 - head * Math.cos(angle + spread), s.y2 - head * Math.sin(angle + spread));
  ctx.closePath();
  ctx.fill();
  ctx.restore();
};

const box = (s) => ({
  x: Math.round(Math.min(s.x1, s.x2)),
  y: Math.round(Math.min(s.y1, s.y2)),
  w: Math.round(Math.abs(s.x2 - s.x1)),
  h: Math.round(Math.abs(s.y2 - s.y1)),
});

// Highlighter: translucent yellow, visible on light and dark backgrounds.
const drawMark = (s) => {
  const r = box(s);
  ctx.save();
  ctx.fillStyle = MARK;
  ctx.fillRect(r.x, r.y, r.w, r.h);
  ctx.restore();
};

// Blur by scaling the area down to one pixel per block and back up. The
// detail is gone for good; a filter blur could leave text guessable.
const drawBlur = (s) => {
  const r = box(s);
  if (r.w < 1 || r.h < 1) {
    return;
  }
  const block = s.block;
  const small = document.createElement("canvas");
  small.width = Math.max(1, Math.ceil(r.w / block));
  small.height = Math.max(1, Math.ceil(r.h / block));
  const sctx = small.getContext("2d");
  sctx.imageSmoothingEnabled = true;
  sctx.imageSmoothingQuality = "high";
  sctx.drawImage(canvas, r.x, r.y, r.w, r.h, 0, 0, small.width, small.height);
  ctx.save();
  ctx.imageSmoothingEnabled = true;
  ctx.imageSmoothingQuality = "high";
  ctx.drawImage(small, 0, 0, small.width, small.height, r.x, r.y, r.w, r.h);
  ctx.restore();
};

const DRAW = { arrow: drawArrow, mark: drawMark, blur: drawBlur };

const render = () => {
  ctx.globalCompositeOperation = "source-over";
  ctx.clearRect(0, 0, canvas.width, canvas.height);
  ctx.drawImage(image, 0, 0);
  for (const s of drawing ? [...shapes, drawing] : shapes) {
    DRAW[s.tool](s);
  }
};

// Shapes smaller than this are treated as a click, not a drag.
const tooSmall = (s) =>
  s.tool === "arrow"
    ? Math.hypot(s.x2 - s.x1, s.y2 - s.y1) < 3 * dpr()
    : box(s).w < 3 || box(s).h < 3;

// --- Layout --------------------------------------------------------------

// Shows the image at most at one image pixel per device pixel.
const fit = () => {
  if (!image) {
    return;
  }
  const style = getComputedStyle(stage);
  const width = stage.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight);
  const height = stage.clientHeight - parseFloat(style.paddingTop) - parseFloat(style.paddingBottom);
  const scale = Math.min(1 / dpr(), width / canvas.width, height / canvas.height);
  canvas.style.width = `${Math.max(1, Math.floor(canvas.width * scale))}px`;
  canvas.style.height = `${Math.max(1, Math.floor(canvas.height * scale))}px`;
};

const toImage = (event) => {
  const r = canvas.getBoundingClientRect();
  const clamp = (v, max) => Math.min(max, Math.max(0, v));
  return {
    x: clamp(((event.clientX - r.left) * canvas.width) / r.width, canvas.width),
    y: clamp(((event.clientY - r.top) * canvas.height) / r.height, canvas.height),
  };
};

// --- Toolbar -------------------------------------------------------------

const selectTool = (name) => {
  tool = name;
  for (const b of document.querySelectorAll(".tool")) {
    b.setAttribute("aria-pressed", String(b.dataset.tool === name));
  }
  colorGroup.hidden = name !== "arrow";
};

const selectColor = (value) => {
  color = value;
  for (const b of colorGroup.querySelectorAll(".swatch")) {
    b.setAttribute("aria-checked", String(b.dataset.color === value));
  }
};

const updateButtons = () => {
  undoButton.disabled = busy || shapes.length === 0;
  saveButton.disabled = busy || !image;
};

const undo = () => {
  if (shapes.length > 0 && !busy) {
    shapes.pop();
    render();
    updateButtons();
  }
};

for (const [name, value] of COLORS) {
  const b = document.createElement("button");
  b.type = "button";
  b.className = "swatch";
  b.setAttribute("role", "radio");
  b.dataset.color = value;
  b.dataset.tLabel = `color.${name}`;
  b.style.background = value;
  b.addEventListener("click", () => selectColor(value));
  colorGroup.append(b);
}

for (const b of document.querySelectorAll(".tool")) {
  b.addEventListener("click", () => selectTool(b.dataset.tool));
}

const save = async () => {
  if (busy || !image) {
    return;
  }
  busy = true;
  updateButtons();
  setStatus(t(uploads ? "editor.uploading" : "editor.saving"));
  try {
    drawing = null;
    render();
    const blob = await new Promise((resolve, reject) =>
      canvas.toBlob((b) => (b ? resolve(b) : reject(new Error("toBlob"))), "image/png"),
    );
    await invoke("save_edit", new Uint8Array(await blob.arrayBuffer()));
  } catch (err) {
    busy = false;
    updateButtons();
    setStatus(t("editor.error", err));
  }
};

const discard = () => {
  if (!busy) {
    invoke("discard_edit");
  }
};

undoButton.addEventListener("click", undo);
saveButton.addEventListener("click", save);
document.getElementById("discard").addEventListener("click", discard);

// --- Input ---------------------------------------------------------------

canvas.addEventListener("pointerdown", (event) => {
  if (event.button !== 0 || !image || busy) {
    return;
  }
  canvas.setPointerCapture(event.pointerId);
  const p = toImage(event);
  drawing = { tool, x1: p.x, y1: p.y, x2: p.x, y2: p.y };
  if (tool === "arrow") {
    drawing.color = color;
    drawing.width = lineWidth();
  } else if (tool === "blur") {
    drawing.block = blurBlock();
  }
});

canvas.addEventListener("pointermove", (event) => {
  if (!drawing) {
    return;
  }
  const p = toImage(event);
  drawing.x2 = p.x;
  drawing.y2 = p.y;
  render();
});

const endDrawing = () => {
  if (!drawing) {
    return;
  }
  const shape = drawing;
  drawing = null;
  if (!tooSmall(shape)) {
    shapes.push(shape);
  }
  render();
  updateButtons();
};

canvas.addEventListener("pointerup", endDrawing);
canvas.addEventListener("pointercancel", () => {
  drawing = null;
  render();
});

window.addEventListener("keydown", (event) => {
  if (event.ctrlKey && event.key.toLowerCase() === "z") {
    event.preventDefault();
    undo();
  } else if (event.ctrlKey && event.key.toLowerCase() === "s") {
    event.preventDefault();
    save();
  } else if (event.key === "Enter" && !event.target.closest?.("button")) {
    save();
  } else if (event.key === "Escape") {
    // With edits, Esc has to be pressed twice so no work is lost by accident.
    if (shapes.length === 0 || Date.now() - discardArmed < 2000) {
      discard();
    } else {
      discardArmed = Date.now();
      setStatus(t("editor.discard_again"));
    }
  } else if (!event.ctrlKey && !event.altKey && ["1", "2", "3"].includes(event.key)) {
    selectTool(TOOLS[Number(event.key) - 1]);
  }
});

window.addEventListener("resize", fit);

// --- Start ---------------------------------------------------------------

const note = (line) => invoke("note", { line }).catch(() => {});

// The IPC hands raw bytes over as ArrayBuffer, or as a plain array of
// numbers when it falls back to JSON.
const toBytes = (data) => {
  if (data instanceof ArrayBuffer) {
    return new Uint8Array(data);
  }
  if (ArrayBuffer.isView(data)) {
    return new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
  }
  return Uint8Array.from(data);
};

// Decodes the PNG. WebKitGTK cannot always decode a Blob with
// createImageBitmap, so an <img> from a blob: URL is the fallback; both
// stay same-origin and keep the canvas exportable.
const decode = async (data) => {
  const kind = Object.prototype.toString.call(data);
  const bytes = toBytes(data);
  note(`editor image: ${kind}, ${bytes.length} bytes, head ${Array.from(bytes.slice(0, 4)).join(",")}`);
  const blob = new Blob([bytes], { type: "image/png" });
  try {
    return await createImageBitmap(blob);
  } catch (err) {
    note(`createImageBitmap failed: ${err}; using <img>`);
  }
  const url = URL.createObjectURL(blob);
  try {
    const img = new Image();
    img.src = url;
    await img.decode();
    return img;
  } finally {
    URL.revokeObjectURL(url);
  }
};

translate();
selectTool("arrow");
selectColor(color);
updateButtons();

(async () => {
  try {
    uploads = (await invoke("editor_config")).upload;
    saveButton.textContent = t(uploads ? "editor.upload" : "editor.save");
    image = await decode(await invoke("edit_image"));
    canvas.width = image.width;
    canvas.height = image.height;
    fit();
    render();
    updateButtons();
  } catch (err) {
    setStatus(t("editor.error", err));
  }
})();
