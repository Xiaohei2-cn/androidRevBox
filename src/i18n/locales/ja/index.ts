import common from "./common";
import dashboard from "./dashboard";
import nav from "./nav";
import settings from "./settings";

/** 日本語 */
export default {
  ...common,
  ...nav,
  ...dashboard,
  ...settings,
} as Record<string, string>;
