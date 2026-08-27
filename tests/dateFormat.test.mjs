import assert from "node:assert/strict";
import test from "node:test";
import { formatDate } from "../src/lib/dateFormat.ts";

test("dates format for humans, omitting the year only when it matches the current one", () => {
  const currentYear = new Date().getUTCFullYear();
  assert.equal(formatDate(`${currentYear}-09-10`), "Sep 10");
  assert.equal(
    formatDate(`${currentYear - 1}-09-10`),
    `Sep 10, ${currentYear - 1}`,
  );
  assert.equal(
    formatDate(`${currentYear + 1}-01-05`),
    `Jan 5, ${currentYear + 1}`,
  );
});
