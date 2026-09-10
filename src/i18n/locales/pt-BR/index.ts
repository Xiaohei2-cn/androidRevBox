import common from "./common";
import dashboard from "./dashboard";
import nav from "./nav";
import settings from "./settings";

/** Português (Brasil) */
export default {
  ...common,
  ...nav,
  ...dashboard,
  ...settings,
} as Record<string, string>;
