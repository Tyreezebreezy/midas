import { useSyncExternalStore } from "react";

const formatter = new Intl.NumberFormat("en-US", {
  style: "currency",
  currency: "USD",
});

// Privacy Mode (Batch 10): a single reactive flag, checked directly inside
// formatCents, so every one of its existing callers gets masking for free
// with no per-call-site rewrite. A component only needs one useMasked()
// subscription (anywhere in its body) to re-render when the flag flips -
// every formatCents() call in that render then reflects the new state.
// Deliberately session-only (never persisted - see docs/settings), so a
// fresh launch never surprises the user with numbers already hidden.
let masked = false;
const listeners = new Set<() => void>();

export function isPrivacyModeOn(): boolean {
  return masked;
}

export function setPrivacyMode(next: boolean): void {
  if (next === masked) return;
  masked = next;
  listeners.forEach((fn) => fn());
}

export function useMasked(): boolean {
  return useSyncExternalStore(
    (fn) => {
      listeners.add(fn);
      return () => listeners.delete(fn);
    },
    () => masked,
  );
}

// A fixed-length placeholder - never derived from the real value - so
// $12.54 and $1,284,529.87 mask to the exact same string and neither
// length nor digits leak the real magnitude.
const MASK = "$••••.••";

export function formatCents(cents: number): string {
  if (masked) return MASK;
  return formatter.format(cents / 100);
}

// Review/Signal evidence sentences ("Dining is over budget by $57.00 this
// month") are composed server-side as plain strings, not built from
// formatCents - masking formatCents alone can't reach them. This finds and
// replaces every embedded dollar figure so the same Privacy Mode covers
// them too, without the backend needing any notion of display privacy.
const MONEY_IN_TEXT = /\$\d+(?:,\d{3})*\.\d{2}/g;

export function maskMoneyInText(text: string): string {
  if (!masked) return text;
  return text.replace(MONEY_IN_TEXT, MASK);
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

// Used only to populate an editable amount <input> - never masked, even
// with Privacy Mode on, since a form the user has explicitly opened to
// edit a figure needs to show that figure (section 47).
export function centsToDollarInput(cents: number): string {
  return (cents / 100).toFixed(2).replace("-0.00", "0.00");
}
