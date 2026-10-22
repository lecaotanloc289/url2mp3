# url2mp3

[English](README.md) · **Tiếng Việt**

Tải nhạc từ YouTube, TikTok (và ~1800 trang yt-dlp hỗ trợ: SoundCloud, Facebook…) về máy
dưới dạng MP3 / M4A / Opus, kèm ảnh bìa và tên bài. Chạy trong terminal: không quảng cáo,
không hàng đợi, tải nhiều bài và nhiều kết nối cùng lúc.

## Cài đặt (macOS)

```bash
brew install rust yt-dlp ffmpeg        # yt-dlp tự kéo theo deno để giải chữ ký YouTube
cargo install --git https://github.com/lecaotanloc289/url2mp3 url2mp3
```

Từ bản clone: `cargo install --path crates/cli --root ~/.local` (cài lệnh `url2mp3` vào
`~/.local/bin`).

Linux/Windows: cài `yt-dlp`, `ffmpeg`, `deno` bằng trình quản lý gói của hệ điều hành, rồi
chạy dòng `cargo install` như trên.

## Dùng

```bash
url2mp3 https://www.youtube.com/watch?v=Llw9Q6akRo4      # một bài → ~/Downloads
url2mp3 "lạc trôi sơn tùng"                              # tìm trên YouTube, tải kết quả đầu
url2mp3 https://www.tiktok.com/@user/video/123 link2 …   # nhiều link cùng lúc
url2mp3 'https://www.youtube.com/playlist?list=…' -n 20  # 20 bài đầu của playlist
url2mp3 -a danh-sach.txt                                 # mỗi dòng một link
url2mp3                                                  # dán link vào, Enter dòng trống để tải
```

| Tuỳ chọn | Ý nghĩa |
|---|---|
| `-f mp3\|m4a\|opus` | Định dạng xuất. Mặc định `mp3` |
| `-b 320` | Bitrate khi phải nén lại. Mặc định MP3 VBR V0 (~245 kbps), M4A 256, Opus 160 |
| `-o DIR` | Thư mục lưu. Mặc định `~/Downloads`. Playlist vào thư mục con theo tên playlist |
| `-j N` | Số bài xử lý cùng lúc. Mặc định = số nhân CPU (tối đa 8) |
| `-c N` | Số kết nối song song cho mỗi bài. Mặc định 8 |
| `-p` | Link `watch?v=…&list=…`: tải cả playlist thay vì một bài |
| `-n N` | Chỉ lấy N bài đầu mỗi playlist |
| `--no-cover` | Không gắn ảnh bìa |
| `--force` | Tải lại bài đã có (mặc định bỏ qua file trùng tên; cùng một video dán 2 lần chỉ tải 1 lần) |
| `--cookies-from-browser chrome` | Dùng đăng nhập của trình duyệt: video giới hạn tuổi, luồng 256 kbps của Premium |

## Cách hoạt động

```
link ─▶ yt-dlp -J ─▶ URL luồng audio tốt nhất ─▶ tải song song 8 kết nối ─▶ ffmpeg ─▶ .mp3 + tag + ảnh bìa
        (giải chữ ký,                            (HTTP Range, ghi thẳng         (copy luồng nếu
         chọn luồng)                              vào đúng vị trí trong file)     cùng codec)
```

Toàn bộ quy trình nằm trong thư viện `crates/core` (`url2mp3-core`), báo tiến độ dưới dạng sự
kiện; lệnh `url2mp3` (`crates/cli`) chỉ vẽ các sự kiện đó ra terminal, và các app sẽ dùng chung
thư viện này.

- **Lấy link luồng** (`crates/core/src/resolve.rs`): gọi `yt-dlp -J` chỉ để lấy JSON. YouTube đổi thuật
  toán chữ ký vài tuần một lần, yt-dlp theo kịp, nên chỉ cần `brew upgrade yt-dlp`.
- **Tải** (`crates/core/src/download.rs`): chia file thành các đoạn byte, tải qua N kết nối HTTP/1.1 cùng
  lúc. Mỗi đoạn ≤ 8 MB (googlevideo bóp tốc độ request lớn hơn ~10 MB) và tự tải tiếp khi rớt
  mạng. Mỗi phản hồi được kiểm `Content-Range` (đúng vị trí, đúng tổng dung lượng) để không bao
  giờ ghi sai dữ liệu. Nếu CDN từ chối (403), trả sai đoạn, hoặc luồng là HLS/DASH, tự chuyển
  sang trình tải của yt-dlp.
- **Chuyển định dạng** (`crates/core/src/transcode.rs`): ffmpeg chạy một lượt để nén, gắn ảnh bìa (cắt
  vuông) và ghi tag. Nếu codec nguồn trùng codec đích (Opus→`.opus`, AAC→`.m4a`, MP3 TikTok→`.mp3`)
  thì chỉ copy luồng, không nén lại.

## Chất lượng & tốc độ

- YouTube miễn phí chỉ có Opus ~130–160 kbps hoặc AAC 128 kbps. **Nén sang MP3 320 kbps
  không làm âm thanh hay hơn bản gốc**, chỉ làm file nặng hơn. V0 là đủ trong suốt (transparent).
- Muốn giữ đúng 100% chất lượng gốc và nhanh nhất thì dùng `-f opus` (YouTube) hoặc `-f m4a`
  (chạy được mọi nơi, kể cả Apple Music). Không phải nén lại nên mất ~0,5 giây mỗi bài.
- Với MP3, phần lớn thời gian là LAME nén (~2,5 s cho bài 4 phút, chạy đơn luồng), nên `-j` mặc
  định bằng số nhân CPU để playlist dùng hết các nhân.

## Lỗi thường gặp

| Lỗi | Cách xử lý |
|---|---|
| `zsh: parse error near '&'` | Bọc link trong nháy đơn: `url2mp3 '…watch?v=…&list=…'`, hoặc chạy `url2mp3` rồi dán link |
| `Sign in to confirm you're not a bot` | Giảm `-j`, hoặc thêm `--cookies-from-browser chrome` |
| `dns error` / `Failed to resolve '…googlevideo.com'` | Mạng hoặc DNS mất tạm thời (Wi-Fi chập chờn, VPN/WARP). Chạy lại đúng lệnh: bài đã tải xong sẽ tự bỏ qua |
| Lỗi YouTube khác | `brew upgrade yt-dlp` (YouTube đổi API thường xuyên) |
| `tải trực tiếp lỗi (HTTP 403)` | Chỉ là cảnh báo: chương trình đã tự tải lại bằng yt-dlp |

## Phát triển

```bash
cargo test --workspace
cargo clippy --workspace --all-targets
cargo build --release   # target/release/url2mp3
```

Chỉ dùng cho nội dung bạn có quyền tải (dùng cá nhân, nội dung của bạn, Creative Commons).
Tải từ YouTube/TikTok vi phạm điều khoản dịch vụ của các nền tảng đó.

## Giấy phép

[MIT](LICENSE)
