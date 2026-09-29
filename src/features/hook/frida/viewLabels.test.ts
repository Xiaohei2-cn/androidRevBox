/**
 * 第九十三轮：那块屏的两种看法叫 终端 / 结构化终端。
 * 第二个名字必须含第一个的词根（同一块屏的两种看法），五语都钉。
 */
import { describe, expect, it } from "vitest";
import { DICTIONARIES, LOCALES } from "@/i18n/dictionaries";

const KEYS = ["hook.frida.viewTerminal", "hook.frida.viewTracks"] as const;

describe("frida 那块屏的两种看法", () => {
  it("两个名字成对：第二个含第一个的词根，且不相同（五语）", () => {
    for (const locale of LOCALES) {
      const dict = DICTIONARIES[locale];
      const [term, tracks] = KEYS.map((k) => dict[k]);
      expect(term, locale).toBeTruthy();
      expect(tracks, locale).toBeTruthy();
      expect(tracks.toLowerCase(), locale).toContain(term.toLowerCase());
      expect(tracks, locale).not.toBe(term);
    }
  });
});
