// SCRN web UI helpers. Loaded as a file because the CSP forbids inline
// scripts. Everything here is an enhancement; the pages work without it.

// Copy buttons: <button data-copy="input-id" data-copied="…" data-failed="…">.
// Uses the Clipboard API and falls back to selecting the text so it can be
// copied by hand.
document.addEventListener("click", async (event) => {
  const button = event.target.closest("button[data-copy]");
  if (!button) {
    return;
  }
  const input = document.getElementById(button.dataset.copy);
  if (!input) {
    return;
  }
  let copied = false;
  if (navigator.clipboard && window.isSecureContext) {
    try {
      await navigator.clipboard.writeText(input.value);
      copied = true;
    } catch (_) {
      copied = false;
    }
  }
  if (!copied) {
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
