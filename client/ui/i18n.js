// UI strings of the client. German is the default; English is used when
// the system language is not German.

const STRINGS = {
  de: {
    "overlay.hint": "Bereich mit der Maus aufziehen · Esc bricht ab",
    "editor.tools": "Werkzeuge",
    "editor.arrow": "Pfeil",
    "editor.mark": "Markieren",
    "editor.blur": "Unkenntlich",
    "editor.color": "Farbe",
    "editor.undo": "Rückgängig",
    "editor.discard": "Verwerfen",
    "editor.save": "Speichern",
    "editor.saving": "Wird gespeichert …",
    "editor.discard_again": "Noch einmal Esc drücken, um zu verwerfen",
    "editor.error": "Fehler: {0}",
    "color.red": "Rot",
    "color.orange": "Orange",
    "color.green": "Grün",
    "color.blue": "Blau",
    "color.black": "Schwarz",
    "color.white": "Weiß",
  },
  en: {
    "overlay.hint": "Drag to select an area · Esc cancels",
    "editor.tools": "Tools",
    "editor.arrow": "Arrow",
    "editor.mark": "Highlight",
    "editor.blur": "Blur",
    "editor.color": "Color",
    "editor.undo": "Undo",
    "editor.discard": "Discard",
    "editor.save": "Save",
    "editor.saving": "Saving …",
    "editor.discard_again": "Press Esc again to discard",
    "editor.error": "Error: {0}",
    "color.red": "Red",
    "color.orange": "Orange",
    "color.green": "Green",
    "color.blue": "Blue",
    "color.black": "Black",
    "color.white": "White",
  },
};

const LANG = (navigator.language || "de").toLowerCase().startsWith("de") ? "de" : "en";

export const t = (key, ...args) =>
  (STRINGS[LANG][key] ?? key).replace(/\{(\d)\}/g, (_, i) => String(args[Number(i)] ?? ""));

// Fills every element with `data-t` (text) or `data-t-label` (aria-label
// and title) from the catalog.
export const translate = (root = document) => {
  document.documentElement.lang = LANG;
  for (const el of root.querySelectorAll("[data-t]")) {
    el.textContent = t(el.dataset.t);
  }
  for (const el of root.querySelectorAll("[data-t-label]")) {
    el.setAttribute("aria-label", t(el.dataset.tLabel));
    el.title = t(el.dataset.tLabel);
  }
};
