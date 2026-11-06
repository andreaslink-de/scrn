// Settings window: server address, API token (kept in the system
// keyring by the app), upload defaults and autostart.

import { t, translate } from "./i18n.js";

const { invoke } = window.__TAURI__.core;

const form = document.getElementById("form");
const serverUrl = document.getElementById("server_url");
const token = document.getElementById("token");
const tokenState = document.getElementById("token-state");
const insecure = document.getElementById("insecure");
const testButton = document.getElementById("test");
const forgetButton = document.getElementById("forget");
const testResult = document.getElementById("test-result");
const expiry = document.getElementById("expiry");
const status = document.getElementById("status");

const show = (el, text, kind = "") => {
  el.textContent = text;
  el.className = `result ${kind}`;
};

// Mirrors config::is_insecure for the live warning.
const isInsecure = (value) => {
  try {
    const url = new URL(value.trim());
    const host = url.hostname.replace(/^\[|\]$/g, "");
    const local = host === "localhost" || host === "::1" || /^127\./.test(host);
    return url.protocol === "http:" && !local;
  } catch {
    return false;
  }
};

const updateExpiry = () => {
  expiry.disabled = form.elements.visibility.value !== "public";
};

const fill = (view) => {
  const s = view.settings;
  serverUrl.value = s.server_url;
  form.elements.visibility.value = s.visibility;
  expiry.value = s.expires_in_days === null ? "" : String(s.expires_in_days);
  // Values set elsewhere (e.g. by hand in the file) stay selectable.
  if (expiry.value !== (s.expires_in_days === null ? "" : String(s.expires_in_days))) {
    const option = new Option(`${s.expires_in_days}`, String(s.expires_in_days), true, true);
    expiry.add(option);
  }
  document.getElementById("keep_copy").checked = s.keep_copy;
  document.getElementById("autostart").checked = s.autostart;
  token.value = "";
  tokenState.textContent = t(view.has_token ? "settings.token_stored" : "settings.token_none");
  forgetButton.hidden = !view.has_token;
  insecure.hidden = !isInsecure(s.server_url);
  document.getElementById("files").textContent = t("settings.files", view.config_path, view.log_path);
  updateExpiry();
};

const collect = () => ({
  server_url: serverUrl.value,
  visibility: form.elements.visibility.value || "private",
  expires_in_days: expiry.value === "" ? null : Number(expiry.value),
  keep_copy: document.getElementById("keep_copy").checked,
  autostart: document.getElementById("autostart").checked,
});

serverUrl.addEventListener("input", () => {
  insecure.hidden = !isInsecure(serverUrl.value);
});

for (const radio of form.elements.visibility) {
  radio.addEventListener("change", updateExpiry);
}

testButton.addEventListener("click", async () => {
  testButton.disabled = true;
  show(testResult, t("settings.testing"));
  try {
    const who = await invoke("test_connection", {
      serverUrl: serverUrl.value,
      token: token.value || null,
    });
    show(testResult, t("settings.connected", who), "ok");
  } catch (err) {
    show(testResult, String(err), "error");
  } finally {
    testButton.disabled = false;
  }
});

forgetButton.addEventListener("click", async () => {
  try {
    fill(await invoke("forget_token"));
    show(status, t("settings.saved"), "ok");
  } catch (err) {
    show(status, String(err), "error");
  }
});

form.addEventListener("submit", async (event) => {
  event.preventDefault();
  show(status, "");
  try {
    fill(await invoke("save_settings", { settings: collect(), token: token.value || null }));
    show(status, t("settings.saved"), "ok");
  } catch (err) {
    show(status, String(err), "error");
  }
});

window.addEventListener("keydown", (event) => {
  if (event.key === "Escape") {
    window.close();
  }
});

translate();
document.title = `${t("settings.title")} · SCRN`;
(async () => {
  try {
    fill(await invoke("get_settings"));
  } catch (err) {
    show(status, String(err), "error");
  }
})();
