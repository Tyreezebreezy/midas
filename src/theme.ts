export type ThemePreference = "system" | "light" | "dark";
export type ResolvedTheme = Exclude<ThemePreference, "system">;

export function parseThemePreference(value: string | null): ThemePreference {
  return value === "light" || value === "dark" ? value : "system";
}

export function resolveTheme(
  preference: ThemePreference,
  systemDark: boolean,
): ResolvedTheme {
  return preference === "system" ? (systemDark ? "dark" : "light") : preference;
}
