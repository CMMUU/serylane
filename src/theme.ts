export const THEME_OPTIONS = [
  { id: "light", label: "浅色", labelKey: "ui-aa0819dfc4d8d7", description: "清爽浅底，清晰深字", descriptionKey: "ui-9deec466d219f0" },
  { id: "dark", label: "深色", labelKey: "ui-a6b75d0680322a", description: "中性炭灰，柔和低亮", descriptionKey: "ui-9ceab8507954fa" },
  { id: "purple", label: "深紫", labelKey: "ui-f48bebbe058511", description: "深紫底色，紫色光晕", descriptionKey: "ui-5bc4a37610dade" },
  { id: "system", label: "跟随系统", labelKey: "ui-217cfe7db1e3d1", description: "自动切换浅色与深色", descriptionKey: "ui-68450ce0d27dec" },
] as const;

export type ThemePreference = (typeof THEME_OPTIONS)[number]["id"];
export type ResolvedTheme = Exclude<ThemePreference, "system">;

export function isThemePreference(value: unknown): value is ThemePreference {
  return THEME_OPTIONS.some((option) => option.id === value);
}

export function normalizeTheme(value: unknown): ThemePreference {
  return isThemePreference(value) ? value : "system";
}

export function resolveTheme(preference: ThemePreference, systemDark: boolean): ResolvedTheme {
  return preference === "system" ? (systemDark ? "dark" : "light") : preference;
}

export function themeColorScheme(theme: ResolvedTheme): "light" | "dark" {
  return theme === "light" ? "light" : "dark";
}

export type ThemeSnapshot = Readonly<{
  preference: ThemePreference;
  selected: ThemePreference;
  resolved: ResolvedTheme;
  saving: boolean;
}>;

type ThemeControllerOptions = {
  systemDark: () => boolean;
  persist: (preference: ThemePreference) => Promise<unknown>;
  render: (snapshot: ThemeSnapshot) => void;
};

/** Serializes preference writes; failed saves restore the last confirmed theme. */
export class ThemeController {
  private preference: ThemePreference = "system";
  private pending: ThemePreference | null = null;
  private revision = 0;

  constructor(private readonly options: ThemeControllerOptions) {}

  get mutationRevision(): number {
    return this.revision;
  }

  get snapshot(): ThemeSnapshot {
    const selected = this.pending ?? this.preference;
    return {
      preference: this.preference,
      selected,
      resolved: resolveTheme(selected, this.options.systemDark()),
      saving: this.pending !== null,
    };
  }

  sync(preference: unknown, requestedRevision?: number): boolean {
    // Reject a read spanning a write, even if that write already completed.
    if (this.pending !== null || (requestedRevision !== undefined && requestedRevision !== this.revision)) return false;
    const next = normalizeTheme(preference);
    if (next !== this.preference) this.revision += 1;
    this.preference = next;
    this.refresh();
    return true;
  }

  refresh(): void {
    this.options.render(this.snapshot);
  }

  async select(preference: ThemePreference): Promise<boolean> {
    if (!isThemePreference(preference)) throw new Error("无效主题");
    if (this.pending !== null || preference === this.preference) return false;
    this.revision += 1;
    this.pending = preference;
    this.refresh();
    try {
      const saved = await this.options.persist(preference);
      if (!isThemePreference(saved)) throw new Error("保存主题返回了无效设置");
      this.preference = saved;
      return true;
    } finally {
      this.pending = null;
      this.revision += 1;
      this.refresh();
    }
  }
}
