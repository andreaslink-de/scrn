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
  document.getElementById("check_updates").checked = s.check_updates;
  token.value = "";
  tokenState.textContent = t(view.has_token ? "settings.token_stored" : "settings.token_none");
  forgetButton.hidden = !view.has_token;
  insecure.hidden = !isInsecure(s.server_url);
  document.getElementById("shortcut-field").hidden = !view.shortcut_supported;
  document.getElementById("shortcut").value = s.shortcut;
  document.getElementById("shortcut-error").textContent = view.shortcut_error
    ? t("settings.shortcut_failed", view.shortcut_error)
    : "";
  document.getElementById("files").textContent = t("settings.files", view.config_path, view.log_path);
  document.getElementById("version").textContent = t("settings.version", view.version);
  updateExpiry();
};

const collect = () => ({
  server_url: serverUrl.value,
  visibility: form.elements.visibility.value || "private",
  expires_in_days: expiry.value === "" ? null : Number(expiry.value),
  keep_copy: document.getElementById("keep_copy").checked,
  autostart: document.getElementById("autostart").checked,
  check_updates: document.getElementById("check_updates").checked,
  shortcut: document.getElementById("shortcut").value,
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

// Links open in the browser, not in this window.
for (const link of document.querySelectorAll("a[data-page]")) {
  link.addEventListener("click", (event) => {
    event.preventDefault();
    invoke("open_project", { page: link.dataset.page }).catch(() => {});
  });
}

// Recording a shortcut: the next key combination fills the field. The
// registered shortcut is released meanwhile, or it would swallow the keys.
const shortcutInput = document.getElementById("shortcut");
const recordButton = document.getElementById("record");
const MODIFIERS = ["Control", "Alt", "Shift", "Meta", "AltGraph", "OS"];
let recording = null;

// `KeyboardEvent.code` as the shortcut parser reads it: "KeyS" -> "S".
const accelerator = (event) => {
  const parts = [];
  if (event.ctrlKey) parts.push("Ctrl");
  if (event.altKey) parts.push("Alt");
  if (event.shiftKey) parts.push("Shift");
  if (event.metaKey) parts.push("Super");
  parts.push(event.code.replace(/^Key(?=[A-Z]$)/, "").replace(/^Digit(?=[0-9]$)/, ""));
  return parts.join("+");
};

const stopRecording = (value) => {
  if (!recording) return;
  window.removeEventListener("keydown", recording, true);
  window.removeEventListener("keyup", recording, true);
  window.removeEventListener("blur", cancelRecording);
  recording = null;
  shortcutInput.classList.remove("recording");
  shortcutInput.placeholder = "PrintScreen";
  shortcutInput.readOnly = false;
  if (value !== undefined) shortcutInput.value = value;
  invoke("resume_shortcut").catch(() => {});
};

const cancelRecording = () => stopRecording();

recordButton.addEventListener("click", async () => {
  if (recording) {
    stopRecording();
    return;
  }
  try {
    await invoke("pause_shortcut");
  } catch {
    return;
  }
  const previous = shortcutInput.value;
  shortcutInput.value = "";
  shortcutInput.placeholder = t("settings.recording");
  shortcutInput.readOnly = true;
  shortcutInput.classList.add("recording");
  recording = (event) => {
    event.preventDefault();
    event.stopPropagation();
    if (event.code === "Escape" && event.type === "keydown") {
      stopRecording(previous);
      return;
    }
    // Windows reports Print Screen only when it is released.
    const printScreen = event.code === "PrintScreen";
    if (event.type === "keyup" && !printScreen) return;
    if (MODIFIERS.includes(event.key) || !event.code) return;
    stopRecording(accelerator(event));
  };
  window.addEventListener("keydown", recording, true);
  window.addEventListener("keyup", recording, true);
  window.addEventListener("blur", cancelRecording);
});
