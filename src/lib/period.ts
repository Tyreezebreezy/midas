// The one shared period model every Insights tab uses (Batch 8, section 5) -
// Month/Quarter/Year navigation and a single comparison rule (section 30-32:
// same date math everywhere, same-point comparison for a still-incomplete
// current period, full-period comparison once a period has ended). Mirrors
// `lib/monthValue.ts`'s existing month-only helpers rather than replacing
// them (Budgets/Reports both still use those), just generalized to also
// cover Quarter and Year.

export type PeriodMode = "month" | "quarter" | "year";

// "YYYY-MM" for month, "YYYY-Q1".."YYYY-Q4" for quarter, "YYYY" for year.
export type PeriodValue = string;

function pad2(n: number): string {
  return String(n).padStart(2, "0");
}

function daysInMonth(year: number, month1to12: number): number {
  return new Date(Date.UTC(year, month1to12, 0)).getUTCDate();
}

export function addDaysIso(date: string, days: number): string {
  const [y, m, d] = date.split("-").map(Number);
  return new Date(Date.UTC(y, m - 1, d + days)).toISOString().slice(0, 10);
}

export function daysBetweenIso(earlier: string, later: string): number {
  const [ey, em, ed] = earlier.split("-").map(Number);
  const [ly, lm, ld] = later.split("-").map(Number);
  const a = Date.UTC(ey, em - 1, ed);
  const b = Date.UTC(ly, lm - 1, ld);
  return Math.round((b - a) / 86_400_000);
}

export function todayIso(): string {
  return new Date().toISOString().slice(0, 10);
}

function quarterOf(month1to12: number): number {
  return Math.floor((month1to12 - 1) / 3) + 1;
}

export function currentPeriodValue(mode: PeriodMode): PeriodValue {
  const now = new Date();
  const year = now.getUTCFullYear();
  const month = now.getUTCMonth() + 1;
  if (mode === "month") return `${year}-${pad2(month)}`;
  if (mode === "quarter") return `${year}-Q${quarterOf(month)}`;
  return `${year}`;
}

export function shiftPeriod(
  mode: PeriodMode,
  value: PeriodValue,
  delta: number,
): PeriodValue {
  if (mode === "month") {
    const [year, month] = value.split("-").map(Number);
    const d = new Date(Date.UTC(year, month - 1 + delta, 1));
    return `${d.getUTCFullYear()}-${pad2(d.getUTCMonth() + 1)}`;
  }
  if (mode === "quarter") {
    const [yearStr, qStr] = value.split("-Q");
    const totalQuarters = Number(yearStr) * 4 + (Number(qStr) - 1) + delta;
    const year = Math.floor(totalQuarters / 4);
    const quarter = (totalQuarters % 4) + 1;
    return `${year}-Q${quarter}`;
  }
  return `${Number(value) + delta}`;
}

export function periodRange(
  mode: PeriodMode,
  value: PeriodValue,
): { from: string; to: string } {
  if (mode === "month") {
    const [year, month] = value.split("-").map(Number);
    return {
      from: `${value}-01`,
      to: `${value}-${pad2(daysInMonth(year, month))}`,
    };
  }
  if (mode === "quarter") {
    const [yearStr, qStr] = value.split("-Q");
    const year = Number(yearStr);
    const firstMonth = (Number(qStr) - 1) * 3 + 1;
    const lastMonth = firstMonth + 2;
    return {
      from: `${year}-${pad2(firstMonth)}-01`,
      to: `${year}-${pad2(lastMonth)}-${pad2(daysInMonth(year, lastMonth))}`,
    };
  }
  return { from: `${value}-01-01`, to: `${value}-12-31` };
}

const monthNameFormatter = new Intl.DateTimeFormat("en-US", {
  month: "long",
  year: "numeric",
  timeZone: "UTC",
});

export function periodLabel(mode: PeriodMode, value: PeriodValue): string {
  if (mode === "month")
    return monthNameFormatter.format(new Date(`${value}-01`));
  if (mode === "quarter") return value.replace("-", " ");
  return value;
}

export interface PeriodComparison {
  currentFrom: string;
  currentTo: string;
  previousFrom: string;
  previousTo: string;
  // True when the current period hasn't finished yet, so `currentTo` stops
  // at today and `previousTo` is truncated to the same elapsed-day count
  // rather than the previous period's real end (section 32).
  isPartial: boolean;
}

export function periodComparison(
  mode: PeriodMode,
  value: PeriodValue,
): PeriodComparison {
  const { from, to } = periodRange(mode, value);
  const today = todayIso();
  const isCurrent = value === currentPeriodValue(mode);
  const previousValue = shiftPeriod(mode, value, -1);
  const { from: previousFrom, to: previousPeriodEnd } = periodRange(
    mode,
    previousValue,
  );

  if (isCurrent && today < to) {
    const elapsedDays = daysBetweenIso(from, today);
    return {
      currentFrom: from,
      currentTo: today,
      previousFrom,
      previousTo: addDaysIso(previousFrom, elapsedDays),
      isPartial: true,
    };
  }
  return {
    currentFrom: from,
    currentTo: to,
    previousFrom,
    previousTo: previousPeriodEnd,
    isPartial: false,
  };
}
