const formatter = new Intl.NumberFormat("en-US", {
  style: "currency",
  currency: "USD",
});

export function formatCents(cents: number): string {
  return formatter.format(cents / 100);
}

// Parses user-typed dollar input ("1,234.56", "-42", "") into integer cents.
// Returns null for empty/unparseable input so callers can show a validation
// error instead of silently coercing to 0.
export function parseDollarsToCents(input: string): number | null {
  const trimmed = input.trim().replace(/,/g, "");
  if (trimmed === "") return null;
  const dollars = Number(trimmed);
  if (!Number.isFinite(dollars)) return null;
  return Math.round(dollars * 100);
}

export function centsToDollarInput(cents: number): string {
  return (cents / 100).toFixed(2);
}
