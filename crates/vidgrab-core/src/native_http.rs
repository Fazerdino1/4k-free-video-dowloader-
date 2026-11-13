//! Native multi-connection HTTP(S) range downloader for direct media URLs.
//!
//! Bypasses `yt-dlp` process startup overhead when downloading direct `.mp4`, `.mkv`,
//! `.webm`, `.mp3`, `.flac`, or `.wav` files from servers supporting `Accept-Ranges: bytes`.

use crate::error::{CoreError, CoreResult};
use futures_util::StreamExt;
use std::path::Path;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio::sync::mpsc;

const DEFAULT_SEGMENTS: usize = 4;
const MIN_RANGE_BYTES: u64 = 1024 * 512; // 512 KiB

/// Returns `true` if the URL points to a direct downloadable media file extension.
#[must_use]
pub fn is_direct_media_url(url: &str) -> bool {
    let lower = url.split('?').next().unwrap_or(url).to_ascii_lowercase();
    [".mp4", ".mkv", ".webm", ".mp3", ".m4a", ".flac", ".wav", ".ogg"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

/// Downloads `url` to `dest_path` using parallel HTTP byte-range requests when supported,
/// or a single streaming GET fallback. Reports `(downloaded_bytes, total_bytes)` progress.
pub async fn download_direct_http<F>(
    url: &str,
    dest_path: &Path,
    segments: usize,
    on_progress: F,
) -> CoreResult<u64>
where
    F: Fn(u64, Option<u64>) + Send + Sync + 'static,
{
    let client = crate::shared_http_client();
    let head = client.head(url).send().await;

    let (total_len, supports_range) = match head {
        Ok(resp) if resp.status().is_success() => {
            let len = resp
                .headers()
                .get(reqwest::header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok());
            let accept_ranges = resp
                .headers()
                .get(reqwest::header::ACCEPT_RANGES)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.eq_ignore_ascii_case("bytes"))
                .unwrap_or(false);
            (len, accept_ranges)
        }
        _ => (None, false),
    };

    if let Some(parent) = dest_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let seg_count = segments.clamp(1, 8).max(DEFAULT_SEGMENTS.min(segments.max(1)));
    if supports_range && total_len.unwrap_or(0) >= MIN_RANGE_BYTES && seg_count > 1 {
        let total = total_len.unwrap_or(0);
        let file = tokio::fs::File::create(dest_path).await?;
        file.set_len(total).await?;
        drop(file);

        let chunk_size = total / seg_count as u64;
        let (tx, mut rx) = mpsc::unbounded_channel::<u64>();
        let mut handles = Vec::with_capacity(seg_count);

        for idx in 0..seg_count {
            let start = idx as u64 * chunk_size;
            let end = if idx + 1 == seg_count {
                total - 1
            } else {
                (idx as u64 + 1) * chunk_size - 1
            };
            let url_c = url.to_string();
            let path_c = dest_path.to_path_buf();
            let tx_c = tx.clone();
            handles.push(tokio::spawn(async move {
                let resp = crate::shared_http_client()
                    .get(&url_c)
                    .header(reqwest::header::RANGE, format!("bytes={start}-{end}"))
                    .send()
                    .await?
                    .error_for_status()?;
                let mut f = tokio::fs::OpenOptions::new().write(true).open(&path_c).await?;
                f.seek(std::io::SeekFrom::Start(start)).await?;
                let mut stream = resp.bytes_stream();
                while let Some(chunk_res) = stream.next().await {
                    let chunk = chunk_res?;
                    f.write_all(&chunk).await?;
                    let _ = tx_c.send(chunk.len() as u64);
                }
                f.flush().await?;
                Ok::<(), CoreError>(())
            }));
        }
        drop(tx);

        let mut downloaded = 0u64;
        while let Some(n) = rx.recv().await {
            downloaded += n;
            on_progress(downloaded, Some(total));
        }

        for h in handles {
            match h.await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => return Err(e),
                Err(e) => return Err(CoreError::YtDlpFailed(format!("HTTP task panic: {e}"))),
            }
        }
        return Ok(total);
    }

    // Single-stream fallback
    let resp = client.get(url).send().await?.error_for_status()?;
    let total = resp.content_length().or(total_len);
    let mut f = tokio::fs::File::create(dest_path).await?;
    let mut stream = resp.bytes_stream();
    let mut downloaded = 0u64;
    while let Some(chunk_res) = stream.next().await {
        let chunk = chunk_res?;
        f.write_all(&chunk).await?;
        downloaded += chunk.len() as u64;
        on_progress(downloaded, total);
    }
    f.flush().await?;
    Ok(downloaded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn test_is_direct_media_url() {
        assert!(is_direct_media_url("https://cdn.example.com/video.mp4?token=1"));
        assert!(is_direct_media_url("https://cdn.example.com/audio.FLAC"));
        assert!(!is_direct_media_url("https://www.youtube.com/watch?v=dQw4w9WgXcQ"));
    }

    #[tokio::test]
    async fn test_download_direct_http_local_server() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let payload = b"VIDGRAB_NATIVE_HTTP_TEST_PAYLOAD";

        let srv = tokio::spawn(async move {
            for _ in 0..2 {
                if let Ok((mut sock, _)) = listener.accept().await {
                    let mut buf = [0u8; 1024];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    if req.starts_with("HEAD ") {
                        let hdr = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            payload.len()
                        );
                        let _ = sock.write_all(hdr.as_bytes()).await;
                    } else {
                        let hdr = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            payload.len()
                        );
                        let _ = sock.write_all(hdr.as_bytes()).await;
                        let _ = sock.write_all(payload).await;
                    }
                }
            }
        });

        let tmp = tempfile::tempdir().expect("tempdir");
        let dest = tmp.path().join("sample.mp4");
        let url = format!("http://{addr}/sample.mp4");
        let bytes = download_direct_http(&url, &dest, 2, |_, _| {})
            .await
            .expect("download");
        assert_eq!(bytes, payload.len() as u64);
        assert_eq!(std::fs::read(&dest).expect("read"), payload);
        let _ = srv.await;
    }
}
