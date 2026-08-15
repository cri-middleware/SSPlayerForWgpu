//! Bulk atlas decoding for the loading path.
//!
//! A load decodes every atlas at once, on the thread the host drives its frame
//! from, and a 4096² PNG costs tens of milliseconds. This splits the batch
//! across short-lived threads and returns the results **in request order**.
//!
//! The threads come from a `std::thread::scope` that is gone before the call
//! returns; nothing persistent is created and no dependency is added.

use crate::renderer::premultiply;

/// One decoded atlas: RGBA8 pixels and the size they cover.
pub(crate) struct Decoded {
    pub(crate) rgba: Vec<u8>,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

/// Decode every entry of `encoded`, in parallel where that is worth it.
///
/// The result is positional: `out[i]` is `encoded[i]` decoded, or `None` if it was
/// not a readable image. Completion order never reaches the caller, whose own
/// registration order decides which hash wins.
///
/// `premultiplied` moves the premultiply pass onto the worker as well;
/// the caller then hands the pixels to
/// [`SSRenderer::register_premultiplied`](crate::SSRenderer::register_premultiplied)
/// and the host's thread does nothing but create the texture and write it.
pub(crate) fn decode_all(encoded: &[&[u8]], premultiplied: bool) -> Vec<Option<Decoded>> {
    // One image is the common case, and a thread joined immediately costs more
    // than it saves.
    if encoded.len() < 2 || threads_for(encoded.len()) < 2 {
        return encoded.iter().map(|bytes| decode_one(bytes, premultiplied)).collect();
    }

    // Filled by index, never appended to, so neither completion order nor a
    // failed chunk can shift the results.
    let mut out: Vec<Option<Decoded>> = encoded.iter().map(|_| None).collect();
    // `scope` rather than `spawn`: the workers borrow `encoded`, so the bytes
    // need not be copied to hand them over.
    std::thread::scope(|scope| {
        let workers: Vec<_> = split(encoded.len(), threads_for(encoded.len()))
            .map(|range| {
                let chunk = &encoded[range.clone()];
                (range, scope.spawn(move || chunk.iter().map(|bytes| decode_one(bytes, premultiplied)).collect::<Vec<_>>()))
            })
            .collect();
        for (range, worker) in workers {
            // A panicking decoder must not take the host's frame with it: its chunk
            // stays `None`, so those hashes fall back to the 1x1 white texture
            // exactly as an unreadable file would.
            let Ok(chunk) = worker.join() else { continue };
            for (slot, decoded) in out[range].iter_mut().zip(chunk) {
                *slot = decoded;
            }
        }
    });
    out
}

fn decode_one(encoded: &[u8], premultiplied: bool) -> Option<Decoded> {
    let decoded = image::load_from_memory(encoded).ok()?;
    let rgba = decoded.to_rgba8();
    let (width, height) = rgba.dimensions();
    let mut rgba = rgba.into_raw();
    if premultiplied {
        premultiply(&mut rgba);
    }
    Some(Decoded { rgba, width, height })
}

/// How many workers to split `jobs` across: never more than there are images, and
/// never more than the machine will actually run at once.
fn threads_for(jobs: usize) -> usize {
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    jobs.min(cores)
}

/// Contiguous ranges covering `0..len`, as evenly as `parts` allows.
///
/// Contiguous and not round-robin so each worker's slice of `encoded` is one
/// subslice, which is what lets the chunks be concatenated back into order.
fn split(len: usize, parts: usize) -> impl Iterator<Item = std::ops::Range<usize>> {
    let base = len / parts;
    let extra = len % parts;
    (0..parts).scan(0, move |start, i| {
        let take = base + usize::from(i < extra);
        let range = *start..*start + take;
        *start = range.end;
        Some(range)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2x1 straight-alpha RGBA PNG: opaque red, then half-transparent white.
    fn png_2x1() -> Vec<u8> {
        let mut out = Vec::new();
        let pixels: [u8; 8] = [255, 0, 0, 255, 255, 255, 255, 128];
        image::write_buffer_with_format(&mut std::io::Cursor::new(&mut out), &pixels, 2, 1, image::ExtendedColorType::Rgba8, image::ImageFormat::Png).unwrap();
        out
    }

    #[test]
    fn results_keep_request_order_and_report_failures_positionally() {
        let png = png_2x1();
        let bad = b"not a png".as_slice();
        let encoded: Vec<&[u8]> = vec![&png, bad, &png, &png, bad];
        let out = decode_all(&encoded, false);
        assert_eq!(out.len(), encoded.len());
        assert!(out[0].is_some() && out[2].is_some() && out[3].is_some());
        assert!(out[1].is_none() && out[4].is_none(), "an unreadable image must not shift the ones after it");
        assert_eq!(out[0].as_ref().unwrap().rgba, [255, 0, 0, 255, 255, 255, 255, 128]);
    }

    #[test]
    fn premultiplying_on_the_worker_matches_premultiplying_after() {
        let png = png_2x1();
        let encoded: Vec<&[u8]> = vec![&png, &png];
        let worker = decode_all(&encoded, true);
        let mut straight = decode_all(&encoded, false).remove(0).unwrap().rgba;
        premultiply(&mut straight);
        assert_eq!(worker[0].as_ref().unwrap().rgba, straight);
    }

    #[test]
    fn split_covers_every_index_once() {
        for len in 0..12 {
            for parts in 1..6 {
                let ranges: Vec<_> = split(len, parts).collect();
                assert_eq!(ranges.iter().map(|r| r.len()).sum::<usize>(), len);
                assert_eq!(ranges.last().map(|r| r.end).unwrap_or(0), len);
            }
        }
    }
}
