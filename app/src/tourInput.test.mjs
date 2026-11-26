import assert from "node:assert/strict";
import { test } from "node:test";
import { practiceInput } from "./tourInput.ts";

test("a repeated Backspace releases the hold and deletes, while a fresh press can undo", () => {
  assert.deepEqual(practiceInput({ key: "Backspace", repeat: true }), {
    sent: "Reset", key: "Backspace", preventDefault: true,
  });
  assert.equal(practiceInput({ key: "Backspace", repeat: false }).sent, "Backspace");
  assert.deepEqual(practiceInput({ key: "x", repeat: true }), {
    sent: "Reset", key: "x", preventDefault: true,
  });
});

test("shortcuts and Tab reset the practice engine and keep their browser behavior", () => {
  for (const event of [
    { key: "Backspace", ctrlKey: true },
    { key: "x", altKey: true },
    { key: "a", metaKey: true },
    { key: "Tab" },
    { key: "Tab", shiftKey: true },
  ]) {
    assert.deepEqual(practiceInput(event), { sent: "Reset", key: null, preventDefault: false });
  }
  assert.equal(practiceInput({ key: "Backspace", shiftKey: true }).sent, "Reset");
  assert.equal(practiceInput({ key: "T", shiftKey: true }).sent, "T");
  assert.equal(practiceInput({ key: "Process", isComposing: true }), null);
});
