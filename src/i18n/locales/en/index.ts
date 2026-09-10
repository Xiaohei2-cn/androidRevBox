import common from "./common";
import dashboard from "./dashboard";
import nav from "./nav";
import settings from "./settings";

/** English (fallback target after zh-CN) */
export default {
  ...common,
  ...nav,
  ...dashboard,
  ...settings,
} as Record<string, string>;
