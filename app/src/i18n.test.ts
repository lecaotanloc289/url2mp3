import { describe, expect, it } from "vitest";
import { category, dictionaries, hint, pickLanguage, setLanguage, t } from "./i18n";

describe("i18n", () => {
  it("both languages have the same keys", () => {
    expect(Object.keys(dictionaries.en).sort()).toEqual(Object.keys(dictionaries.vi).sort());
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
