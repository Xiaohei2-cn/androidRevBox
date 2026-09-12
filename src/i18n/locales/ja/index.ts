import common from "./common";
import dashboard from "./dashboard";
import devices from "./devices";
import nav from "./nav";
import settings from "./settings";

/** 日本語 */
export default {
  ...common,
  ...nav,
  ...dashboard,
  ...devices,
  ...settings,
} as Record<string, string>;
