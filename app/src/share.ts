// Text shared to the app from another Android app → what to download. YouTube shares the bare
// link; TikTok and browsers often wrap it in a sentence.

export function sharedInput(text: string): string | null {
  const link = /https?:\/\/[^\s"'<>]+/.exec(text)?.[0];
  if (link) return link.replace(/[.,;:!?)\]]+$/, "");
  const phrase = text.trim();
  return phrase ? phrase : null;
}
