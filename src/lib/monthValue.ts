// A "month value" is the YYYY-MM the month <input> element uses. Shared
// between Budgets and Reports, which both need to page through months and
// turn one into the ISO date range accountingApi.getTotalSpending expects.

export function currentMonthValue(): string {
  return new Date().toISOString().slice(0, 7); // YYYY-MM
}

export function monthToIso(monthValue: string): string {
  return `${monthValue}-01`;
}

export function isoToMonthValue(monthIso: string): string {
  return monthIso.slice(0, 7);
}

export function shiftMonth(monthValue: string, delta: number): string {
  const [year, month] = monthValue.split("-").map(Number);
  const d = new Date(Date.UTC(year, month - 1 + delta, 1));
  return d.toISOString().slice(0, 7);
}

export function monthRange(monthValue: string): { from: string; to: string } {
  const [year, month] = monthValue.split("-").map(Number);
  const lastDay = new Date(Date.UTC(year, month, 0)).getUTCDate();
  return {
    from: monthToIso(monthValue),
    to: `${monthValue}-${String(lastDay).padStart(2, "0")}`,
  };
}
