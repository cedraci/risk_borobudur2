import type { Category } from "./api";

export const pct = (x: number | null | undefined, digits = 2) =>
  x == null ? "–" : `${(x * 100).toFixed(digits)}%`;
export const num = (x: number | null | undefined, digits = 2) =>
  x == null ? "–" : x.toLocaleString("fr-FR", { maximumFractionDigits: digits, minimumFractionDigits: digits });
export const eur = (x: number | null | undefined) =>
  x == null ? "–" : x.toLocaleString("fr-FR", { style: "currency", currency: "EUR", maximumFractionDigits: 0 });

export const CATEGORY_LABELS: Record<Category, string> = {
  equity: "Equity",
  interest_rate: "Interest rate",
  fx: "Foreign exchange",
  credit: "Credit",
  commodity: "Commodity",
  other: "Other",
};
