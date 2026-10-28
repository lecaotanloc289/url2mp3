import { describe, expect, it } from "vitest";
import { category, dictionaries, hint, pickLanguage, setLanguage, t } from "./i18n";

describe("i18n", () => {
  it("both languages have the same keys", () => {
    expect(Object.keys(dictionaries.en).sort()).toEqual(Object.keys(dictionaries.vi).sort());
  });

  it("every singular variant has its plural, with the same placeholders", () => {
    for (const dict of Object.values(dictionaries)) {
      for (const [key, text] of Object.entries(dict)) {
        if (!key.endsWith("_one")) continue;
        const base = key.slice(0, -"_one".length) as keyof typeof dict;
        expect(dict[base], key).toBeDefined();
        const names = (s: string) => [...s.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort();
        expect(names(text), key).toEqual(names(dict[base]));
        expect(names(text), key).toContain("n");
      }
    }
  });

  it("uses the singular for one in English, the same words in Vietnamese", () => {
    setLanguage("en");
    expect(t("saveAll", { n: 1 })).toBe("● Download 1 track");
    expect(t("saveAll", { n: 2 })).toBe("● Download 2 tracks");
    expect(t("saveAll", { n: 0 })).toBe("● Download 0 tracks");
    expect(t("tag.removed", { n: 1 })).toBe("1 part removed");
    expect(t("tag.removed", { n: 3 })).toBe("3 parts removed");
    expect(t("playlistReady", { n: 1 })).toBe("1 track · ready");
    expect(t("keepAlive", { n: 1 })).toBe("Downloading 1 track…");
    setLanguage("vi");
    expect(t("saveAll", { n: 1 })).toBe("● Tải 1 bài");
    expect(t("saveAll", { n: 4 })).toBe("● Tải 4 bài");
  });

  it("credits SponsorBlock on its suggestions", () => {
    setLanguage("en");
    expect(t("trim.suggest", { n: 1, list: "0:00–0:28 (intro)" })).toBe("Found 1 non-music part: 0:00–0:28 (intro) · SponsorBlock");
    expect(t("trim.suggest", { n: 2, list: "…" })).toBe("Found 2 non-music parts: … · SponsorBlock");
    setLanguage("vi");
    expect(t("trim.suggest", { n: 2, list: "…" })).toBe("Tìm thấy 2 đoạn không phải nhạc: … · SponsorBlock");
  });

  it("follows the system unless the user chose", () => {
    expect(pickLanguage(null, ["vi-VN", "en-US"])).toBe("vi");
    expect(pickLanguage(null, ["en-US", "vi"])).toBe("vi");
    expect(pickLanguage(null, ["fr-FR"])).toBe("en");
    expect(pickLanguage("en", ["vi-VN"])).toBe("en");
  });

  it("fills placeholders", () => {
    setLanguage("en");
    expect(t("status.converting", { ext: "mp3" })).toBe("Converting to mp3…");
    setLanguage("vi");
    expect(t("playlistCount", { finished: 2, count: 5 })).toBe("2/5 bài");
  });

  it("names SponsorBlock categories, unknown ones as sent", () => {
    setLanguage("en");
    expect(category("intro")).toBe("intro");
    expect(category("music_offtopic")).toBe("non-music");
    expect(category("filler")).toBe("filler");
    setLanguage("vi");
    expect(category("outro")).toBe("kết thúc");
    expect(t("tag.removed", { n: 2 })).toBe("Đã bỏ 2 đoạn");
  });

  it("explains common failures", () => {
    expect(hint("dns error", true)).toBe("hint.network");
    expect(hint("Sign in to confirm you're not a bot", false)).toBe("hint.bot");
    expect(hint("Video unavailable. This video is private", false)).toBe("hint.unavailable");
    expect(hint("Sign in to confirm your age", false)).toBe("hint.age");
    expect(hint("ffmpeg: boom", false)).toBe("hint.generic");
  });
});
