import assert from "node:assert/strict";
import test from "node:test";
import {
  formatCents,
  isPrivacyModeOn,
  maskMoneyInText,
  setPrivacyMode,
} from "../src/lib/currency.ts";

test("Privacy Mode masks formatCents to a fixed-length placeholder regardless of magnitude", () => {
  setPrivacyMode(true);
  try {
    assert.equal(formatCents(1254), formatCents(128452987));
    assert.equal(formatCents(-450000), formatCents(5));
  } finally {
    setPrivacyMode(false);
  }
});

test("formatCents is real currency when Privacy Mode is off", () => {
  assert.equal(isPrivacyModeOn(), false);
  assert.equal(formatCents(150000), "$1,500.00");
  assert.equal(formatCents(-450), "-$4.50");
});

test("maskMoneyInText replaces every embedded dollar figure in a server-composed sentence, never the sentence itself", () => {
  const evidence = "Dining is over budget by $57.00 this month.";
  assert.equal(maskMoneyInText(evidence), evidence);
  setPrivacyMode(true);
  try {
    const masked = maskMoneyInText(evidence);
    assert.ok(!masked.includes("57.00"));
    assert.ok(masked.startsWith("Dining is over budget by "));
    assert.ok(masked.endsWith(" this month."));
  } finally {
    setPrivacyMode(false);
  }
});

test("maskMoneyInText masks multiple dollar figures in one sentence", () => {
  setPrivacyMode(true);
  try {
    const masked = maskMoneyInText("From $1,234.56 down to $99.00.");
    assert.ok(!masked.includes("1,234.56"));
    assert.ok(!masked.includes("99.00"));
  } finally {
    setPrivacyMode(false);
  }
});
