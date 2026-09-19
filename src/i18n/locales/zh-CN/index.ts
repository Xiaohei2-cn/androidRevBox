import common from "./common";
import apps from "./apps";
import dashboard from "./dashboard";
import devices from "./devices";
import hook from "./hook";
import nav from "./nav";
import settings from "./settings";

/** zh-CN（源语言，必须最全；其他语言回退到此） */
export default {
  ...common,
  ...apps,
  ...nav,
  ...dashboard,
  ...devices,
  ...settings,
  ...hook,
} as Record<string, string>;
