import common from "./common";
import apps from "./apps";
import dashboard from "./dashboard";
import devices from "./devices";
import hook from "./hook";
import nav from "./nav";
import settings from "./settings";

/** English (fallback target after zh-CN) */
export default {
  ...common,
  ...apps,
  ...nav,
  ...dashboard,
  ...devices,
  ...settings,
  ...hook,
} as Record<string, string>;
