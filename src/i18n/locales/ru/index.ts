import common from "./common";
import dashboard from "./dashboard";
import devices from "./devices";
import hook from "./hook";
import nav from "./nav";
import settings from "./settings";

/** Русский */
export default {
  ...common,
  ...nav,
  ...dashboard,
  ...devices,
  ...settings,
  ...hook,
} as Record<string, string>;
