import { describe, expect, it } from "vitest";
import { dictionaries, hint, pickLanguage, setLanguage, t } from "./i18n";

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

  it("explains common failures", () => {
    expect(hint("dns error", true)).toBe("hint.network");
    expect(hint("Sign in to confirm you're not a bot", false)).toBe("hint.bot");
    expect(hint("Video unavailable. This video is private", false)).toBe("hint.unavailable");
    expect(hint("Sign in to confirm your age", false)).toBe("hint.age");
    expect(hint("ffmpeg: boom", false)).toBe("hint.generic");
  });
});
