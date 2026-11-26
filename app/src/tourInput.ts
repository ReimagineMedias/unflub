const RESET_KEYS = new Set(["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End", "PageUp", "PageDown", "Delete"]);

/** What to send to the practice engine, and which key to apply if it passes. */
export function practiceInput(event: KeyboardEvent) {
  const { key, repeat, ctrlKey, metaKey, altKey, shiftKey, isComposing } = event;
  if (isComposing) return null;
  if (ctrlKey || metaKey || altKey || key === "Shift" || key === "Tab") {
    return { sent: "Reset", key: null, preventDefault: false };
  }
  const typed = key === "Backspace" ? key : key === "Enter" ? "\n" : [...key].length === 1 ? key : null;
  if (typed === null && !RESET_KEYS.has(key)) return null;
  // Reset releases a held Backspace without letting this key undo or start another hold.
  const sent = repeat || (shiftKey && key === "Backspace") ? "Reset" : typed ?? "Reset";
  return { sent, key: typed, preventDefault: true };
}
