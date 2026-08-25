import assert from "node:assert/strict";
import test from "node:test";
import { parseThemePreference, resolveTheme } from "../src/theme.ts";

test("theme preferences are validated and resolved", () => {
  assert.equal(parseThemePreference("dark"), "dark");
  assert.equal(parseThemePreference("unexpected"), "system");
  assert.equal(resolveTheme("system", true), "dark");
  assert.equal(resolveTheme("system", false), "light");
  assert.equal(resolveTheme("light", true), "light");
});
