// SCRN web UI helpers. Loaded as a file because the CSP forbids inline
// scripts. Everything here is an enhancement; the pages work without it.

// Copy buttons: <button data-copy="input-id" …> copies the value of an
// input, <button data-copy-text="…" …> copies the given text; both carry
// data-copied="…" and data-failed="…". The Clipboard API needs HTTPS, so
// plain HTTP falls back to the copy command on a temporary text field.
const copyByCommand = (text) => {
  const area = document.createElement("textarea");
  area.value = text;
  area.setAttribute("readonly", "");
  area.className = "copy-buffer";
  document.body.append(area);
  area.select();
  let copied = false;
  try {
    copied = document.execCommand("copy");
  } catch (_) {
    copied = false;
  }
  area.remove();
  return copied;
};

document.addEventListener("click", async (event) => {
  const button = event.target.closest("button[data-copy], button[data-copy-text]");
  if (!button) {
    return;
  }
  const input = button.dataset.copy ? document.getElementById(button.dataset.copy) : null;
  const text = button.dataset.copyText ?? input?.value;
  if (text === undefined) {
    return;
  }
  let copied = false;
  if (navigator.clipboard && window.isSecureContext) {
    try {
      await navigator.clipboard.writeText(text);
      copied = true;
    } catch (_) {
      copied = false;
    }
  }
  if (!copied) {
    copied = copyByCommand(text);
  }
  if (!copied && input) {
    // Last resort: select the text so it can be copied by hand.
    input.focus();
    input.select();
  }
  const label = button.dataset.label || button.textContent;
  button.dataset.label = label;
  button.textContent = copied ? button.dataset.copied : button.dataset.failed;
  window.setTimeout(() => {
    button.textContent = label;
  }, 2000);
});

// Read-only link fields select their whole content on focus.
document.addEventListener("focusin", (event) => {
  const input = event.target;
  if (input instanceof HTMLInputElement && input.readOnly) {
    input.select();
  }
});

// --- Passkeys (WebAuthn) ----------------------------------------------------
//
// The server sends the ceremony options as JSON with base64url-encoded
// binary fields and expects the credential back in the same encoding (the
// format of PublicKeyCredential.toJSON()). Newer browsers convert natively;
// the helpers below cover the others.

const b64urlToBuffer = (value) => {
  const base64 = value.replace(/-/g, "+").replace(/_/g, "/");
  const padded = base64 + "=".repeat((4 - (base64.length % 4)) % 4);
  return Uint8Array.from(atob(padded), (c) => c.charCodeAt(0)).buffer;
};

const bufferToB64url = (buffer) => {
  let binary = "";
  for (const byte of new Uint8Array(buffer)) {
    binary += String.fromCharCode(byte);
  }
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
};

const creationOptions = (json) => {
  if (PublicKeyCredential.parseCreationOptionsFromJSON) {
    return PublicKeyCredential.parseCreationOptionsFromJSON(json);
  }
  return {
    ...json,
    challenge: b64urlToBuffer(json.challenge),
    user: { ...json.user, id: b64urlToBuffer(json.user.id) },
    excludeCredentials: (json.excludeCredentials || []).map((c) => ({ ...c, id: b64urlToBuffer(c.id) })),
  };
};

const requestOptions = (json) => {
  if (PublicKeyCredential.parseRequestOptionsFromJSON) {
    return PublicKeyCredential.parseRequestOptionsFromJSON(json);
  }
  return {
    ...json,
    challenge: b64urlToBuffer(json.challenge),
    allowCredentials: (json.allowCredentials || []).map((c) => ({ ...c, id: b64urlToBuffer(c.id) })),
  };
};

const credentialJson = (credential) => {
  if (typeof credential.toJSON === "function") {
    return credential.toJSON();
  }
  const r = credential.response;
  const response = { clientDataJSON: bufferToB64url(r.clientDataJSON) };
  if (r.attestationObject) {
    response.attestationObject = bufferToB64url(r.attestationObject);
    response.transports = r.getTransports ? r.getTransports() : [];
  } else {
    response.authenticatorData = bufferToB64url(r.authenticatorData);
    response.signature = bufferToB64url(r.signature);
    response.userHandle = r.userHandle ? bufferToB64url(r.userHandle) : null;
  }
  return {
    id: credential.id,
    rawId: bufferToB64url(credential.rawId),
    type: credential.type,
    response,
    clientExtensionResults: credential.getClientExtensionResults(),
  };
};

const postJson = async (url, csrf, body) => {
  const response = await fetch(url, {
    method: "POST",
    headers: { "Content-Type": "application/json", "X-CSRF-Token": csrf },
    body: JSON.stringify(body),
    credentials: "same-origin",
  });
  const data = await response.json().catch(() => ({}));
  if (!response.ok) {
    throw new Error(data.message || response.statusText);
  }
  return data;
};

const showMessage = (container, text) => {
  const box = container.querySelector("[data-passkey-message]") || document.querySelector("[data-passkey-message]");
  if (box) {
    box.textContent = text;
    box.hidden = false;
  }
};

const supported = () => window.PublicKeyCredential && navigator.credentials;

// Settings: register a new passkey.
document.addEventListener("submit", async (event) => {
  const form = event.target.closest("form[data-passkey-register]");
  if (!form) {
    return;
  }
  event.preventDefault();
  const section = form.closest("section") || document;
  if (!supported()) {
    showMessage(section, form.dataset.unsupported);
    return;
  }
  const csrf = form.dataset.csrf;
  try {
    const start = await postJson("/app/passkeys/register/start", csrf, {});
    const credential = await navigator.credentials.create({ publicKey: creationOptions(start.options.publicKey) });
    const done = await postJson("/app/passkeys/register/finish", csrf, {
      ceremony: start.ceremony,
      name: form.elements.name.value,
      credential: credentialJson(credential),
    });
    window.location.assign(done.redirect);
  } catch (error) {
    showMessage(section, error.name === "NotAllowedError" ? form.dataset.cancelled : error.message);
  }
});

// Login: sign in with a passkey for the entered username.
document.addEventListener("click", async (event) => {
  const button = event.target.closest("button[data-passkey-login-button]");
  if (!button) {
    return;
  }
  const box = button.closest("[data-passkey-login]");
  const form = document.querySelector('form[action="/login"]');
  if (!supported()) {
    showMessage(box, box.dataset.unsupported);
    return;
  }
  const username = form.elements.username.value.trim();
  if (!username) {
    showMessage(box, box.dataset.needUsername);
    form.elements.username.focus();
    return;
  }
  const csrf = form.elements.csrf.value;
  try {
    const start = await postJson("/login/passkey/start", csrf, { username });
    const credential = await navigator.credentials.get({ publicKey: requestOptions(start.options.publicKey) });
    const done = await postJson("/login/passkey/finish", csrf, {
      ceremony: start.ceremony,
      credential: credentialJson(credential),
    });
    window.location.assign(done.redirect);
  } catch (error) {
    showMessage(box, error.name === "NotAllowedError" ? box.dataset.cancelled : error.message);
  }
});

// Settings: confirm with a passkey instead of the current password.
document.addEventListener("click", async (event) => {
  const button = event.target.closest("button[data-passkey-confirm-button]");
  if (!button) {
    return;
  }
  const box = button.closest("[data-passkey-confirm]");
  if (!supported()) {
    showMessage(box, box.dataset.unsupported);
    return;
  }
  const csrf = box.dataset.csrf;
  try {
    const start = await postJson("/app/passkeys/confirm/start", csrf, {});
    const credential = await navigator.credentials.get({ publicKey: requestOptions(start.options.publicKey) });
    const done = await postJson("/app/passkeys/confirm/finish", csrf, {
      ceremony: start.ceremony,
      credential: credentialJson(credential),
    });
    window.location.assign(done.redirect);
  } catch (error) {
    showMessage(box, error.name === "NotAllowedError" ? box.dataset.cancelled : error.message);
  }
});

// --- Local time -------------------------------------------------------------
//
// The server renders times in UTC inside <time datetime="…Z">. They are
// shown in the browser's time zone instead, in the same YYYY-MM-DD HH:MM
// format; the zone name appears as a tooltip.

const pad = (n) => String(n).padStart(2, "0");
const localDate = (d) => `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
const localTime = (d) => `${localDate(d)} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
const zoneName = () => {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || "";
  } catch (_) {
    return "";
  }
};

const localize = (root) => {
  const zone = zoneName();
  for (const el of root.querySelectorAll("time[datetime]")) {
    const date = new Date(el.getAttribute("datetime"));
    if (Number.isNaN(date.getTime())) {
      continue;
    }
    el.textContent = localTime(date);
    if (zone) {
      el.title = zone;
    }
  }
  // The expiry date field shows the local day the link ends on, and
  // starts at the local today.
  for (const input of root.querySelectorAll("input[name=custom_date]")) {
    const end = Number(input.dataset.expiresAt);
    if (input.dataset.expiresAt && Number.isFinite(end)) {
      input.value = localDate(new Date((end - 1) * 1000));
    }
    if (input.min) {
      input.min = localDate(new Date());
    }
  }
};

localize(document);
// Cards loaded later ("show more") are localized as well.
document.addEventListener("htmx:load", (event) => localize(event.detail.elt));

// A chosen expiry date ends at local midnight, not midnight UTC.
document.addEventListener("submit", (event) => {
  const form = event.target;
  const date = form.querySelector("input[name=custom_date]");
  const end = form.querySelector("input[name=custom_end]");
  if (!date || !end) {
    return;
  }
  const parts = date.value.split("-").map(Number);
  if (parts.length !== 3 || parts.some(Number.isNaN)) {
    end.value = "";
    return;
  }
  const [y, m, d] = parts;
  end.value = String(new Date(y, m - 1, d + 1).getTime() / 1000);
});
