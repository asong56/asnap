// stitch.rs — stitch vertically-scrolled screenshot frames into one long image.
//
// Algorithm:
//   For each consecutive pair (A, B), find how many rows at the *bottom* of A
//   also appear near the *top* of B (the "overlap").  Strip the duplicate rows
//   and concatenate the unique portion of B below A.

use image::{DynamicImage, GenericImageView, ImageBuffer, Rgba};

/// Stitch `frames` into a single tall PNG.
/// Frames must all have the same width.
pub fn stitch(frames: Vec<DynamicImage>) -> DynamicImage {
    if frames.is_empty() {
        panic!("[asnap/stitch] no frames supplied");
    }
    if frames.len() == 1 {
        return frames.into_iter().next().unwrap();
    }

    let width = frames[0].width();

    // Compute vertical offsets: how many *new* rows each frame contributes.
    let mut contributions: Vec<u32> = Vec::with_capacity(frames.len());
    contributions.push(frames[0].height()); // first frame: everything

    for i in 1..frames.len() {
        let overlap = find_overlap_rows(&frames[i - 1], &frames[i]);
        let new_rows = frames[i].height().saturating_sub(overlap);
        contributions.push(new_rows);
    }

    let total_height: u32 = contributions.iter().sum();
    let mut canvas: ImageBuffer<Rgba<u8>, Vec<u8>> =
        ImageBuffer::new(width, total_height);

    let mut y_cursor: u32 = 0;
    for (i, frame) in frames.iter().enumerate() {
        let skip = if i == 0 {
            0u32
        } else {
            let overlap = find_overlap_rows(&frames[i - 1], frame);
            overlap
        };

        for src_y in skip..frame.height() {
            for x in 0..width {
                if x < frame.width() {
                    let px = frame.get_pixel(x, src_y);
                    canvas.put_pixel(x, y_cursor + (src_y - skip), px);
                }
            }
        }
        y_cursor += contributions[i];
    }

    DynamicImage::ImageRgba8(canvas)
}

/// Find how many rows from the **bottom** of `a` match the top of `b`.
/// Uses a sampled mean-squared-error comparison for speed.
fn find_overlap_rows(a: &DynamicImage, b: &DynamicImage) -> u32 {
    let w = a.width().min(b.width());
    let h_a = a.height();
    let h_b = b.height();

    // We search for the overlap in the bottom quarter of frame `a`.
    let max_search = (h_a / 2).max(30).min(h_a);
    // Template strip height (from bottom of a that we try to find in b).
    let strip_h = (h_a / 10).max(8).min(max_search / 2);

    let template_y = h_a - strip_h; // start row in `a`

    let mut best_overlap = 0u32;
    let mut best_mse = u64::MAX;

    // Try placing the template strip at position `dy` in `b`.
    for dy in 0..max_search.min(h_b) {
        if dy + strip_h > h_b {
            break;
        }
        let mse = compare_strip(a, template_y, b, dy, strip_h, w);
        if mse < best_mse {
            best_mse = mse;
            // Overlap = rows of `a` from `template_y` onward that match `b` at `dy`.
            // If the template is found at `dy` in b, the overlap from the end of a
            // is (h_a - template_y) + (strip_h at dy ≡ dy rows into b are shared).
            // Simpler: overlap = h_a - template_y (the strip) but dy more rows are above it.
            // Conservative: overlap = strip_h + (h_a - template_y - strip_h) = h_a - template_y
            // We want to skip `h_a - template_y` rows from b if dy==0.
            // Actually: best_overlap = (h_a - template_y) - dy ... let me think again.
            //
            // The template strip lives at rows [template_y .. h_a) in frame a.
            // We found it at rows [dy .. dy+strip_h) in frame b.
            // That means row `template_y + k` in a ≈ row `dy + k` in b.
            // Rows [0..dy) in b are *already* visible in a (above template_y).
            // So number of rows in b that are duplicated = dy + strip_h (everything up to end of match).
            // Wait: rows [dy .. h_b) in b are new after alignment.
            // Overlap (rows to skip from b) = dy + strip_h? No: skip = dy? No...
            //
            // Easier model: after alignment the new content starts at row (dy+strip_h) in b.
            // Overlap rows to drop from b = (dy + strip_h) is wrong; let me just use
            // the simple: overlap_to_skip = (h_a - template_y) as the rows of b that
            // duplicate the tail of a.  dy just fine-tunes it:
            //   skip = (h_a - template_y) + dy  ← rows in b before new content
            // But that can exceed h_b if estimation is bad, so cap it.
            let skip = (h_a - template_y).saturating_add(dy).min(h_b);
            best_overlap = skip;
        }
    }

    // If MSE is too high (frames are very different) fall back to half-height overlap.
    if best_mse > 500 * 500 {
        return h_b / 2;
    }

    best_overlap
}

/// Sample-based MSE between a strip of `a` (starting at ay) and `b` (starting at by).
fn compare_strip(
    a: &DynamicImage, ay: u32,
    b: &DynamicImage, by: u32,
    h: u32, w: u32,
) -> u64 {
    let step = (w / 24).max(1) as u32;
    let mut sum: u64 = 0;
    let mut count: u64 = 0;

    for dx in (0..w).step_by(step as usize) {
        for dy in 0..h {
            let row_a = ay + dy;
            let row_b = by + dy;
            if row_a >= a.height() || row_b >= b.height() {
                continue;
            }
            let pa = a.get_pixel(dx, row_a);
            let pb = b.get_pixel(dx, row_b);
            let dr = (pa[0] as i32 - pb[0] as i32).pow(2);
            let dg = (pa[1] as i32 - pb[1] as i32).pow(2);
            let db = (pa[2] as i32 - pb[2] as i32).pow(2);
            sum += (dr + dg + db) as u64;
            count += 1;
        }
    }

    if count == 0 { u64::MAX } else { sum / count }
}
