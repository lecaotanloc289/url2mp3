// Vietnamese and English strings. Both dictionaries must have the same keys (tested).

import type { Language } from "./api";

const vi = {
  tagline: "YouTube · TikTok · SoundCloud → nhạc",
  placeholder: "Dán link hoặc gõ tên bài hát…\nNhiều link: mỗi dòng một link",
  download: "Tải về",
  shortcut: "⌘/Ctrl + Enter",
  settings: "Cài đặt",
  emptyTitle: "Chưa có bài nào",
  emptyHint: "Dán link YouTube, TikTok… hoặc gõ tên bài, rồi bấm Tải về.",
  "status.probing": "Đang đọc link…",
  "status.downloading": "Đang tải",
  "status.fallback": "yt-dlp đang tải…",
  "status.converting": "Đang chuyển sang {ext}…",
  "status.done": "Xong",
  "status.exists": "Đã có",
  "status.duplicate": "Trùng, bỏ qua",
  "status.failed": "Lỗi",
  "status.cancelled": "Đã huỷ",
  playlistCount: "{finished}/{count} bài",
  playlistFailed: "{failed} lỗi",
  cancel: "Huỷ",
  retry: "Thử lại",
  reveal: "Mở thư mục",
  remove: "Xoá khỏi danh sách",
  clearFinished: "Dọn bài đã xong",
  summary: "{done} xong · {failed} lỗi · {active} đang chạy",
  "tools.missing": "Lần đầu dùng: cần tải yt-dlp (~50 MB).",
  "tools.downloading": "Đang tải yt-dlp {version}…",
  "tools.error": "Không tải được yt-dlp: {message}",
  "tools.install": "Tải yt-dlp",
  "tools.retry": "Thử lại",
  ytdlpMissing: "Chờ tải xong yt-dlp rồi thử lại.",
  appUpdate: "Đã có url2mp3 {version}.",
  appUpdateOpen: "Tải bản mới",
  "hint.network": "Mạng chập chờn hoặc DNS lỗi (Wi-Fi, VPN/WARP). Thử lại sau giây lát.",
  "hint.bot": "YouTube nghi là bot: cập nhật yt-dlp, hoặc bật cookie trình duyệt trong Cài đặt.",
  "hint.unavailable": "Video không còn, đang để riêng tư, hoặc bị chặn ở khu vực của bạn.",
  "hint.age": "Video giới hạn độ tuổi: bật cookie trình duyệt (đã đăng nhập) trong Cài đặt.",
  "hint.generic": "Thử cập nhật yt-dlp trong Cài đặt rồi thử lại.",
  "set.title": "Cài đặt",
  "set.outDir": "Thư mục lưu",
  "set.choose": "Chọn…",
  "set.format": "Định dạng",
  "set.bitrate": "Bitrate khi nén lại",
  "set.bitrateAuto": "Tự động (MP3 V0 ~245, M4A 256, Opus 160 kbps)",
  "set.jobs": "Số bài tải cùng lúc",
  "set.connections": "Kết nối mỗi bài",
  "set.playlistLimit": "Tối đa số bài mỗi playlist",
  "set.playlistAll": "Không giới hạn",
  "set.cover": "Gắn ảnh bìa",
  "set.cookies": "Dùng đăng nhập của trình duyệt",
  "set.cookiesHint": "Cho video giới hạn tuổi, hoặc khi YouTube nghi là bot.",
  "set.none": "Không dùng",
  "set.language": "Ngôn ngữ",
  "set.auto": "Theo máy",
  "set.ytdlp": "yt-dlp",
  "set.ytdlpUpdate": "Cập nhật",
  "set.ytdlpNone": "chưa có",
  "set.copyLog": "Sao chép log",
  "set.copied": "Đã sao chép",
  "set.save": "Lưu",
  "set.close": "Đóng",
  "set.legal": "Chỉ tải nội dung bạn có quyền tải.",
  keepAlive: "Đang tải {active} bài…",
};

type Key = keyof typeof vi;

const en: Record<Key, string> = {
  tagline: "YouTube · TikTok · SoundCloud → music",
  placeholder: "Paste links or type a song name…\nSeveral links: one per line",
  download: "Download",
  shortcut: "⌘/Ctrl + Enter",
  settings: "Settings",
  emptyTitle: "Nothing here yet",
  emptyHint: "Paste a YouTube, TikTok… link or type a song name, then hit Download.",
  "status.probing": "Reading link…",
  "status.downloading": "Downloading",
  "status.fallback": "yt-dlp downloading…",
  "status.converting": "Converting to {ext}…",
  "status.done": "Done",
  "status.exists": "Already there",
  "status.duplicate": "Duplicate, skipped",
  "status.failed": "Failed",
  "status.cancelled": "Cancelled",
  playlistCount: "{finished}/{count} tracks",
  playlistFailed: "{failed} failed",
  cancel: "Cancel",
  retry: "Retry",
  reveal: "Show in folder",
  remove: "Remove from list",
  clearFinished: "Clear finished",
  summary: "{done} done · {failed} failed · {active} running",
  "tools.missing": "First run: yt-dlp needs to be downloaded (~50 MB).",
  "tools.downloading": "Downloading yt-dlp {version}…",
  "tools.error": "Couldn't download yt-dlp: {message}",
  "tools.install": "Get yt-dlp",
  "tools.retry": "Retry",
  ytdlpMissing: "Wait for yt-dlp to finish downloading, then try again.",
  appUpdate: "url2mp3 {version} is out.",
  appUpdateOpen: "Get it",
  "hint.network": "Network or DNS hiccup (Wi-Fi, VPN/WARP). Try again in a moment.",
  "hint.bot": "YouTube suspects a bot: update yt-dlp, or turn on browser cookies in Settings.",
  "hint.unavailable": "The video is gone, private, or blocked in your region.",
  "hint.age": "Age-restricted video: turn on browser cookies (signed in) in Settings.",
  "hint.generic": "Try updating yt-dlp in Settings, then retry.",
  "set.title": "Settings",
  "set.outDir": "Save to",
  "set.choose": "Choose…",
  "set.format": "Format",
  "set.bitrate": "Bitrate when re-encoding",
  "set.bitrateAuto": "Automatic (MP3 V0 ~245, M4A 256, Opus 160 kbps)",
  "set.jobs": "Tracks at once",
  "set.connections": "Connections per track",
  "set.playlistLimit": "Max tracks per playlist",
  "set.playlistAll": "No limit",
  "set.cover": "Embed cover art",
  "set.cookies": "Use your browser's login",
  "set.cookiesHint": "For age-restricted videos, or when YouTube suspects a bot.",
  "set.none": "Off",
  "set.language": "Language",
  "set.auto": "System",
  "set.ytdlp": "yt-dlp",
  "set.ytdlpUpdate": "Update",
  "set.ytdlpNone": "not installed",
  "set.copyLog": "Copy log",
  "set.copied": "Copied",
  "set.save": "Save",
  "set.close": "Close",
  "set.legal": "Only download content you have the right to.",
  keepAlive: "Downloading {active} tracks…",
};

export const dictionaries: Record<Language, Record<Key, string>> = { vi, en };

export function pickLanguage(setting: Language | null, system: readonly string[]): Language {
  if (setting) return setting;
  return system.some((l) => l.toLowerCase().startsWith("vi")) ? "vi" : "en";
}

let current: Language = "vi";
export const setLanguage = (lang: Language) => (current = lang);
export const language = () => current;

export function t(key: Key, vars: Record<string, string | number> = {}): string {
  return dictionaries[current][key].replace(/\{(\w+)\}/g, (_, name) => String(vars[name] ?? `{${name}}`));
}

/** What to tell the user about a failed track. */
export function hint(message: string, network: boolean): Key {
  if (network) return "hint.network";
  // Before the bot check: both start with "Sign in to confirm".
  if (/confirm your age|age.restricted|inappropriate/i.test(message)) return "hint.age";
  if (/not a bot|sign in to confirm/i.test(message)) return "hint.bot";
  if (/unavailable|private|removed|not available|blocked/i.test(message)) return "hint.unavailable";
  return "hint.generic";
}
