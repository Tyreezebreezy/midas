// Native SVG spending-composition donut (Phase 6.6), extracted out of
// ReportsPage.tsx so Insights' Spending tab (Batch 8) can reuse the exact
// same chart instead of a second implementation - zero charting dependency,
// per the project's own established direction.

import type { CategoryTotal } from "./api/midas";
import { formatCents, useMasked } from "./lib/currency";

const CHART_COLORS = [
  "var(--chart-1)",
  "var(--chart-2)",
  "var(--chart-3)",
  "var(--chart-4)",
  "var(--chart-5)",
  "var(--chart-6)",
];
const OTHER_COLOR = "var(--midas-border)";
const MAX_DONUT_SLICES = 6;

export interface DonutSlice {
  category: string;
  spent_cents: number;
  color: string;
  isOther: boolean;
}

export function buildDonutSlices(categories: CategoryTotal[]): DonutSlice[] {
  if (categories.length <= MAX_DONUT_SLICES) {
    return categories.map((c, i) => ({
      ...c,
      color: CHART_COLORS[i % CHART_COLORS.length],
      isOther: false,
    }));
  }
  const visible = categories.slice(0, MAX_DONUT_SLICES - 1);
  const otherTotal = categories
    .slice(MAX_DONUT_SLICES - 1)
    .reduce((sum, c) => sum + c.spent_cents, 0);
  return [
    ...visible.map((c, i) => ({
      ...c,
      color: CHART_COLORS[i % CHART_COLORS.length],
      isOther: false,
    })),
    {
      category: "Other",
      spent_cents: otherTotal,
      color: OTHER_COLOR,
      isOther: true,
    },
  ];
}

export { OTHER_COLOR };

function CategoryDonut({
  slices,
  total,
  selected,
  onSelect,
  centerLabel = "spent",
}: {
  slices: DonutSlice[];
  total: number;
  selected: string | null;
  onSelect: (category: string) => void;
  centerLabel?: string;
}) {
  useMasked();
  const size = 180;
  const strokeWidth = 30;
  const radius = (size - strokeWidth) / 2;
  const circumference = 2 * Math.PI * radius;
  let cumulative = 0;

  return (
    <svg
      viewBox={`0 0 ${size} ${size}`}
      className="category-donut"
      role="img"
      aria-label={`Spending by category, ${formatCents(total)} total`}
    >
      <g transform={`rotate(-90 ${size / 2} ${size / 2})`}>
        {slices.map((s) => {
          const fraction = total > 0 ? s.spent_cents / total : 0;
          const dash = fraction * circumference;
          const dashArray = `${dash} ${circumference - dash}`;
          const dashOffset = -cumulative;
          cumulative += dash;
          const dimmed = selected !== null && selected !== s.category;
          return (
            <circle
              key={s.category}
              cx={size / 2}
              cy={size / 2}
              r={radius}
              fill="none"
              stroke={s.color}
              strokeWidth={strokeWidth}
              strokeDasharray={dashArray}
              strokeDashoffset={dashOffset}
              opacity={dimmed ? 0.4 : 1}
              style={{ cursor: s.isOther ? "default" : "pointer" }}
              onClick={s.isOther ? undefined : () => onSelect(s.category)}
            >
              <title>
                {s.category}: {formatCents(s.spent_cents)} (
                {Math.round(fraction * 100)}%)
              </title>
            </circle>
          );
        })}
      </g>
      <g className="category-donut-total">
        <text x={size / 2} y={size / 2 - 8}>
          {formatCents(total)}
        </text>
        <text x={size / 2} y={size / 2 + 12} className="donut-total-label">
          {centerLabel}
        </text>
      </g>
    </svg>
  );
}

export default CategoryDonut;
