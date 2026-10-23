import { expect, it } from "vitest";
import { sharedInput } from "./share";

it("takes the link out of shared text", () => {
  expect(sharedInput("https://youtu.be/Llw9Q6akRo4?si=abc")).toBe("https://youtu.be/Llw9Q6akRo4?si=abc");
  expect(sharedInput("Check out this video! https://vt.tiktok.com/ZSabc/ #fyp")).toBe(
    "https://vt.tiktok.com/ZSabc/",
  );
  expect(sharedInput("Nghe bài này (https://youtu.be/x).")).toBe("https://youtu.be/x");
});

it("falls back to the text as a search, or nothing", () => {
  expect(sharedInput("  lạc trôi  ")).toBe("lạc trôi");
  expect(sharedInput("   ")).toBeNull();
});
