use std::convert::Infallible;
use std::time::Duration;

use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, Full, StreamBody};
use hyper::body::{Bytes, Frame};

/// The response body type served to clients: a boxed, `Send` (not `Sync`) body
/// that is either fully buffered or a paced stream.
pub(crate) type ResponseBody = UnsyncBoxBody<Bytes, Infallible>;

/// Wrap an in-memory body as a single buffered frame.
pub(crate) fn buffered_body(data: Bytes) -> ResponseBody {
    Full::new(data).boxed_unsync()
}

/// A response body that emits `data` in chunks paced to `bps` bytes/second, so
/// the client receives a trickle rather than the whole body at once. `bps == 0`
/// (the unlimited default) or empty `data` yields a single buffered frame with
/// no pacing.
#[allow(
    clippy::cast_precision_loss,
    reason = "chunk length and rate are small enough that f64 pacing is exact in practice"
)]
pub(crate) fn throttled_body(data: Bytes, bps: u64) -> ResponseBody {
    if bps == 0 || data.is_empty() {
        return buffered_body(data);
    }

    // Aim for ~10 chunks per second so the trickle is smooth without bursty
    // sleeps; never below one byte per chunk.
    let chunk_size = usize::try_from(bps / 10).unwrap_or(usize::MAX).max(1);

    let stream = futures_util::stream::unfold((data, 0usize), move |(data, offset)| async move {
        if offset >= data.len() {
            return None;
        }
        let end = (offset + chunk_size).min(data.len());
        let chunk = data.slice(offset..end);
        // This chunk represents `chunk.len()` bytes delivered at `bps` bytes/s.
        let seconds = chunk.len() as f64 / bps as f64;
        tokio::time::sleep(Duration::from_secs_f64(seconds)).await;
        Some((
            Ok::<Frame<Bytes>, Infallible>(Frame::data(chunk)),
            (data, end),
        ))
    });

    StreamBody::new(stream).boxed_unsync()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn buffered_body_is_a_single_frame() {
        let data = Bytes::from(vec![1u8; 1024]);
        let mut body = buffered_body(data.clone());
        let mut frames = 0;
        let mut collected = Vec::new();
        while let Some(frame) = body.frame().await {
            let frame = frame.unwrap();
            if let Some(chunk) = frame.data_ref() {
                collected.extend_from_slice(chunk);
            }
            frames += 1;
        }
        assert_eq!(frames, 1);
        assert_eq!(collected, data.to_vec());
    }

    #[tokio::test]
    async fn zero_bps_is_unthrottled_single_frame() {
        let data = Bytes::from(vec![2u8; 4096]);
        let start = std::time::Instant::now();
        let mut body = throttled_body(data.clone(), 0);
        let mut frames = 0;
        while let Some(frame) = body.frame().await {
            frame.unwrap();
            frames += 1;
        }
        assert_eq!(frames, 1, "bps=0 must not pace or split the body");
        assert!(start.elapsed().as_millis() < 50);
    }

    #[tokio::test]
    async fn throttled_body_delivers_all_bytes_intact() {
        let data = Bytes::from((0u8..=255).cycle().take(1000).collect::<Vec<u8>>());
        let body = throttled_body(data.clone(), 100_000);
        let collected = body.collect().await.unwrap().to_bytes();
        assert_eq!(collected, data);
    }

    #[tokio::test]
    async fn throttle_emits_multiple_paced_frames() {
        // 300 bytes at 1000 B/s => chunk_size 100 => 3 frames, ~0.1s apart.
        let data = Bytes::from(vec![9u8; 300]);
        let mut body = throttled_body(data, 1000);
        let start = std::time::Instant::now();
        let mut frames = 0;
        while let Some(frame) = body.frame().await {
            frame.unwrap();
            frames += 1;
        }
        let elapsed = start.elapsed();
        assert_eq!(
            frames, 3,
            "client should receive a paced trickle, not one blob"
        );
        assert!(
            elapsed.as_millis() >= 250,
            "throttle should pace delivery to ~bps; took {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn empty_body_is_handled() {
        let body = throttled_body(Bytes::new(), 1000);
        let collected = body.collect().await.unwrap().to_bytes();
        assert!(collected.is_empty());
    }
}
