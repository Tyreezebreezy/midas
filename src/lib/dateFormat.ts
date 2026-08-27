// Human-readable display for the ISO date strings ("YYYY-MM-DD") the
// backend returns. UTC is used for parsing/formatting (not the browser's
// local zone) because these are calendar dates, not instants - "2026-09-10"
// must always read as September 10th, regardless of the viewer's timezone.
const dayFormatter = new Intl.DateTimeFormat("en-US", {
  month: "short",
  day: "numeric",
  timeZone: "UTC",
});
const dayFormatterWithYear = new Intl.DateTimeFormat("en-US", {
  month: "short",
  day: "numeric",
  year: "numeric",
  timeZone: "UTC",
});

// "Sep 10", or "Sep 10, 2026" when the year isn't the current one.
export function formatDate(isoDate: string): string {
  const date = new Date(isoDate);
  const currentYear = new Date().getUTCFullYear();
  const formatter =
    date.getUTCFullYear() === currentYear ? dayFormatter : dayFormatterWithYear;
  return formatter.format(date);
}
