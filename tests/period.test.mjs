import assert from "node:assert/strict";
import test from "node:test";
import {
  addDaysIso,
  currentPeriodValue,
  daysBetweenIso,
  periodComparison,
  periodLabel,
  periodRange,
  shiftPeriod,
  todayIso,
} from "../src/lib/period.ts";

test("periodRange computes correct bounds for month, quarter, and year, including leap February", () => {
  assert.deepEqual(periodRange("month", "2026-02"), {
    from: "2026-02-01",
    to: "2026-02-28",
  });
  assert.deepEqual(periodRange("month", "2024-02"), {
    from: "2024-02-01",
    to: "2024-02-29",
  });
  assert.deepEqual(periodRange("quarter", "2026-Q1"), {
    from: "2026-01-01",
    to: "2026-03-31",
  });
  assert.deepEqual(periodRange("quarter", "2026-Q4"), {
    from: "2026-10-01",
    to: "2026-12-31",
  });
  assert.deepEqual(periodRange("year", "2026"), {
    from: "2026-01-01",
    to: "2026-12-31",
  });
});

test("shiftPeriod moves forward and backward across year boundaries for every mode", () => {
  assert.equal(shiftPeriod("month", "2026-01", -1), "2025-12");
  assert.equal(shiftPeriod("month", "2026-12", 1), "2027-01");
  assert.equal(shiftPeriod("quarter", "2026-Q1", -1), "2025-Q4");
  assert.equal(shiftPeriod("quarter", "2026-Q4", 1), "2027-Q1");
  assert.equal(shiftPeriod("year", "2026", -1), "2025");
  assert.equal(shiftPeriod("year", "2026", 5), "2031");
});

test("currentPeriodValue matches the real wall-clock date for every mode", () => {
  const now = new Date();
  const year = now.getUTCFullYear();
  const month = now.getUTCMonth() + 1;
  assert.equal(
    currentPeriodValue("month"),
    `${year}-${String(month).padStart(2, "0")}`,
  );
  assert.equal(currentPeriodValue("year"), `${year}`);
  assert.equal(currentPeriodValue("quarter").startsWith(`${year}-Q`), true);
});

test("periodLabel renders a readable name for every mode", () => {
  assert.equal(periodLabel("month", "2026-09"), "September 2026");
  assert.equal(periodLabel("quarter", "2026-Q3"), "2026 Q3");
  assert.equal(periodLabel("year", "2026"), "2026");
});

test("a fully completed past period compares full period to full previous period", () => {
  for (const mode of ["month", "quarter", "year"]) {
    const value =
      mode === "month" ? "2020-06" : mode === "quarter" ? "2020-Q2" : "2020";
    const { from, to } = periodRange(mode, value);
    const comparison = periodComparison(mode, value);
    assert.equal(comparison.isPartial, false, mode);
    assert.equal(comparison.currentFrom, from, mode);
    assert.equal(comparison.currentTo, to, mode);
    const previous = shiftPeriod(mode, value, -1);
    const previousRange = periodRange(mode, previous);
    assert.equal(comparison.previousFrom, previousRange.from, mode);
    assert.equal(comparison.previousTo, previousRange.to, mode);
  }
});

test("the current, still-incomplete period uses a same-point comparison against the previous period (section 32)", () => {
  for (const mode of ["month", "quarter", "year"]) {
    const value = currentPeriodValue(mode);
    const comparison = periodComparison(mode, value);
    const { to } = periodRange(mode, value);
    if (todayIso() >= to) {
      // Today happens to be the period's very last day - a full-period
      // comparison is correct, not a same-point one. Astronomically rare,
      // but handled rather than asserted away.
      assert.equal(comparison.isPartial, false, mode);
      continue;
    }
    assert.equal(comparison.isPartial, true, mode);
    assert.equal(comparison.currentTo, todayIso(), mode);
    // The core fairness invariant (section 32): the same number of days
    // elapsed on both sides, regardless of which real day "today" is.
    assert.equal(
      daysBetweenIso(comparison.currentFrom, comparison.currentTo),
      daysBetweenIso(comparison.previousFrom, comparison.previousTo),
      mode,
    );
  }
});

test("addDaysIso and daysBetweenIso round-trip and cross month/year boundaries", () => {
  assert.equal(addDaysIso("2026-01-15", 30), "2026-02-14");
  assert.equal(daysBetweenIso("2026-01-15", addDaysIso("2026-01-15", 30)), 30);
  assert.equal(addDaysIso("2026-12-25", 10), "2027-01-04");
  assert.equal(daysBetweenIso("2026-01-01", "2026-01-01"), 0);
});
