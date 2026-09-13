import { useState } from "react";
import type { RenameSummary } from "./api/midas";
import { categoriesApi } from "./api/midas";
import { SUGGESTED_CATEGORIES } from "./lib/categories";

function summaryLine(summary: RenameSummary): string {
  const parts: string[] = [];
  if (summary.transactions_updated > 0) {
    parts.push(
      `${summary.transactions_updated} transaction${summary.transactions_updated === 1 ? "" : "s"}`,
    );
  }
  if (summary.splits_updated > 0) {
    parts.push(
      `${summary.splits_updated} split${summary.splits_updated === 1 ? "" : "s"}`,
    );
  }
  if (summary.rules_updated > 0) {
    parts.push(
      `${summary.rules_updated} rule${summary.rules_updated === 1 ? "" : "s"}`,
    );
  }
  if (summary.budgets_updated > 0) {
    parts.push(
      `${summary.budgets_updated} budget${summary.budgets_updated === 1 ? "" : "s"}`,
    );
  }
  if (parts.length === 0) {
    return "Nothing was using that category, so it was renamed with no other changes.";
  }
  let line = `Updated ${parts.join(", ")}.`;
  if (summary.budgets_skipped_due_to_conflict > 0) {
    line += ` ${summary.budgets_skipped_due_to_conflict} budget${summary.budgets_skipped_due_to_conflict === 1 ? "" : "s"} kept separate because the target category already had a budget for that month.`;
  }
  return line;
}

function CategoryCleanupPage() {
  const [from, setFrom] = useState("");
  const [to, setTo] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<RenameSummary | null>(null);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    setResult(null);
    if (from.trim() === "" || to.trim() === "") {
      setError("Enter both the category to rename and its new name.");
      return;
    }
    if (from.trim().toLowerCase() === to.trim().toLowerCase()) {
      setError("Those are the same category name.");
      return;
    }
    setSaving(true);
    try {
      const summary = await categoriesApi.rename(from.trim(), to.trim());
      setResult(summary);
      setFrom("");
      setTo("");
    } catch (err) {
      setError(String(err));
    } finally {
      setSaving(false);
    }
  }

  return (
    <section aria-labelledby="category-cleanup-heading">
      <h3 id="category-cleanup-heading">Clean up categories</h3>
      <p className="muted">
        Rename a category everywhere it's used at once — on transactions, split
        parts, rules, and budgets. Use this to merge two categories by renaming
        one to match the other exactly.
      </p>

      <form className="inline-form" onSubmit={handleSubmit}>
        <label htmlFor="category-from" className="sr-only">
          Existing category
        </label>
        <input
          id="category-from"
          value={from}
          onChange={(e) => setFrom(e.target.value)}
          placeholder="Existing category"
          list="category-cleanup-options"
        />
        <span aria-hidden="true">→</span>
        <label htmlFor="category-to" className="sr-only">
          New category name
        </label>
        <input
          id="category-to"
          value={to}
          onChange={(e) => setTo(e.target.value)}
          placeholder="New name"
          list="category-cleanup-options"
        />
        <datalist id="category-cleanup-options">
          {SUGGESTED_CATEGORIES.map((c) => (
            <option key={c} value={c} />
          ))}
        </datalist>
        <button type="submit" className="btn-primary" disabled={saving}>
          {saving ? "Renaming…" : "Rename"}
        </button>
      </form>

      {error && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}

      {result && (
        <p className="muted" role="status">
          {summaryLine(result)}
        </p>
      )}
    </section>
  );
}

export default CategoryCleanupPage;
