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
    "editor.upload": "Hochladen",
    "editor.uploading": "Wird hochgeladen …",
    "permission.title": "Screenshots erlauben",
    "permission.text": "SCRN braucht einmalig deine Erlaubnis, um Bildschirmfotos aufzunehmen. Danach klappt jeder Screenshot direkt.",
    "permission.hint": "Die Erlaubnis gilt nur für SCRN und wird in deinem Benutzerkonto gespeichert.",
    "permission.allow": "Erlauben",
    "permission.cancel": "Abbrechen",
    "permission.failed": "Das hat nicht geklappt: {0}",
    "settings.title": "Einstellungen",
    "settings.server": "Server",
    "settings.server_url": "Adresse des Servers",
    "settings.server_hint": "z. B. https://scrn.example.org",
    "settings.insecure": "Ohne HTTPS gehen Token und Bilder unverschlüsselt durchs Netz.",
    "settings.token": "API-Token",
    "settings.token_hint": "Erstellen unter Einstellungen → API-Tokens in der Web-Oberfläche. Wird im Schlüsselbund des Systems gespeichert.",
    "settings.token_stored": "Ein Token ist gespeichert. Leer lassen, um ihn zu behalten.",
    "settings.token_none": "Noch kein Token gespeichert.",
    "settings.forget": "Token entfernen",
    "settings.test": "Verbindung testen",
    "settings.testing": "Wird geprüft …",
    "settings.connected": "Verbunden: {0}",
    "settings.uploads": "Uploads",
    "settings.visibility": "Sichtbarkeit neuer Screenshots",
    "settings.private": "Privat (nur in deiner Galerie)",
    "settings.public": "Öffentlich (mit Kurzlink)",
    "settings.expiry": "Ablauf öffentlicher Links",
    "settings.expiry_server": "Standard deines Kontos",
    "settings.expiry_never": "Nie",
    "settings.expiry_1": "1 Tag",
    "settings.expiry_7": "7 Tage",
    "settings.expiry_30": "30 Tage",
    "settings.keep_copy": "Zusätzlich eine Kopie unter Bilder/SCRN speichern",
    "settings.system": "System",
    "settings.autostart": "Beim Anmelden starten",
    "settings.shortcut": "Tastenkürzel für einen Screenshot",
    "settings.shortcut_hint": "Z. B. PrintScreen oder Ctrl+Shift+S, leer für keins. Für die Druck-Taste in Windows unter Einstellungen → Barrierefreiheit → Tastatur die Option ausschalten, mit der die Druck-Taste die Bildschirmaufnahme öffnet.",
    "settings.shortcut_failed": "Das Tastenkürzel ließ sich nicht registrieren: {0}",
    "settings.save": "Speichern",
    "settings.version": "SCRN-Client {0}",
    "settings.project": "Projekt auf GitHub",
    "settings.saved": "Gespeichert.",
    "settings.files": "Einstellungen: {0} · Log: {1}",
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
    "editor.upload": "Upload",
    "editor.uploading": "Uploading …",
    "permission.title": "Allow screenshots",
    "permission.text": "SCRN needs your permission once to take screenshots. After that, every screenshot works right away.",
    "permission.hint": "The permission only applies to SCRN and is stored in your user account.",
    "permission.allow": "Allow",
    "permission.cancel": "Cancel",
    "permission.failed": "That did not work: {0}",
    "settings.title": "Settings",
    "settings.server": "Server",
    "settings.server_url": "Server address",
    "settings.server_hint": "e.g. https://scrn.example.org",
    "settings.insecure": "Without HTTPS, token and images travel unencrypted.",
    "settings.token": "API token",
    "settings.token_hint": "Create one under Settings → API tokens in the web UI. It is stored in the system keyring.",
    "settings.token_stored": "A token is stored. Leave empty to keep it.",
    "settings.token_none": "No token stored yet.",
    "settings.forget": "Remove token",
    "settings.test": "Test connection",
    "settings.testing": "Checking …",
    "settings.connected": "Connected: {0}",
    "settings.uploads": "Uploads",
    "settings.visibility": "Visibility of new screenshots",
    "settings.private": "Private (only in your gallery)",
    "settings.public": "Public (with short link)",
    "settings.expiry": "Expiry of public links",
    "settings.expiry_server": "Your account default",
    "settings.expiry_never": "Never",
    "settings.expiry_1": "1 day",
    "settings.expiry_7": "7 days",
    "settings.expiry_30": "30 days",
    "settings.keep_copy": "Also save a copy under Pictures/SCRN",
    "settings.system": "System",
    "settings.autostart": "Start on login",
    "settings.shortcut": "Keyboard shortcut for a screenshot",
    "settings.shortcut_hint": "E.g. PrintScreen or Ctrl+Shift+S, empty for none. For the Print Screen key, turn off “Use the Print screen key to open screen capture” in Windows under Settings → Accessibility → Keyboard.",
    "settings.shortcut_failed": "The shortcut could not be registered: {0}",
    "settings.save": "Save",
    "settings.version": "SCRN client {0}",
    "settings.project": "Project on GitHub",
    "settings.saved": "Saved.",
    "settings.files": "Settings: {0} · Log: {1}",
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

// Fills `placeholder` from `data-t-placeholder`.
const placeholders = (root) => {
  for (const el of root.querySelectorAll("[data-t-placeholder]")) {
    el.placeholder = t(el.dataset.tPlaceholder);
  }
};

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
  placeholders(root);
};
