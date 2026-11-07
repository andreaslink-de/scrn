// Asks once for the screenshot permission of the desktop portal. The app
// stores it and then takes the screenshot.

import { t, translate } from "./i18n.js";

const { invoke } = window.__TAURI__.core;

const allow = document.getElementById("allow");
const result = document.getElementById("result");

allow.addEventListener("click", async () => {
  allow.disabled = true;
  result.textContent = "";
  result.className = "result";
  try {
    await invoke("allow_screenshots");
  } catch (err) {
    result.textContent = t("permission.failed", String(err));
    result.className = "result error";
    allow.disabled = false;
  }
});

document.getElementById("cancel").addEventListener("click", () => window.close());
window.addEventListener("keydown", (event) => {
  if (event.key === "Escape") {
    window.close();
  }
});

translate();
document.title = `${t("permission.title")} · SCRN`;
allow.focus();
