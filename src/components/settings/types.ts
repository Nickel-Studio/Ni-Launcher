/* Re-export the frozen settings contract so panels import from one place. */
export type {
  Accent as AccentColor,
  AppSettings,
  DownloadSource,
  SourcePolicy,
  ThemeMode,
} from "../../lib/settings";
