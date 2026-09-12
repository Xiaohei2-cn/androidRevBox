import common from "./common";
import dashboard from "./dashboard";
import devices from "./devices";
import nav from "./nav";
import settings from "./settings";

/** zh-CN（源语言，必须最全；其他语言回退到此） */
export default {
  ...common,
  ...nav,
  ...dashboard,
  ...devices,
  ...settings,
} as Record<string, string>;
