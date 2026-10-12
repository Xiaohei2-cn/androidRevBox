import common from "./common";
import apps from "./apps";
import dashboard from "./dashboard";
import devices from "./devices";
import hook from "./hook";
import nav from "./nav";
import chef from "./chef";
import settings from "./settings";

/** Português (Brasil) */
export default {
  ...common,
  ...apps,
  ...nav,
  ...chef,
  ...dashboard,
  ...devices,
  ...settings,
  ...hook,
} as Record<string, string>;
