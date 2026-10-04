//! Segment Anything (MobileSAM): the mask of the object under a few clicks,
//! without a runtime.
//!
//! SAM splits in two (Kirillov et al., 2023; MobileSAM: Zhang et al., 2023,
//! Apache-2.0). The **image encoder** turns a frame into an embedding,
//! `[1, 256, 64, 64]`, once per frame; the **mask decoder** turns the
//! embedding and a prompt (points, a box) into a mask in a few milliseconds.
//! So a user who clicks five times on one frame pays for one encode.
//!
//! The export used here (Acly's, `huggingface.co/Acly/MobileSAM`) takes the
//! encoder input as the frame itself: HWC, RGB, `0..255` floats, resized so
//! its long side is 1024; the graph normalises and pads it. The decoder is
//! Segment Anything's own ONNX export (`return_single_mask`):
//!
//! - `point_coords` `[1, N, 2]`, in pixels of the **resized** frame;
//! - `point_labels` `[1, N]`: 1 on the object, 0 off it, 2 and 3 a box's
//!   top-left and bottom-right corners, -1 a padding point (needed when
//!   there is no box, or the decoder reads the last point as one);
//! - `mask_input` `[1, 1, 256, 256]` and `has_mask_input` `[1]`: a previous
//!   low-resolution mask, unused here (zeros, 0);
//! - `orig_im_size` `[2]`: height and width the mask comes back at.
//!
//! and answers `masks` `[1, 1, H, W]` (logits, > 0 inside), its
//! `iou_predictions` (the model's own estimate of its quality) and
//! `low_res_masks`.

/// The encoder's long side.
pub const LONG_SIDE: usize = 1024;

/// One click, in pixels of the frame sent with the request.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Click {
    pub x: f32,
    pub y: f32,
    /// On the object, or a part to leave out.
    pub keep: bool,
}

/// The size the encoder sees a `width × height` frame at: the long side at
/// [`LONG_SIDE`], the other in proportion, rounded like SAM's own
/// `ResizeLongestSide`.
pub fn resized_size(width: usize, height: usize) -> (usize, usize) {
    let scale = LONG_SIDE as f32 / width.max(height).max(1) as f32;
    (
        ((width as f32 * scale) + 0.5) as usize,
        ((height as f32 * scale) + 0.5) as usize,
    )
}

/// The encoder's input: the frame resized (bilinear) to [`resized_size`],
/// HWC RGB in 0..255. Returns the tensor and its height and width.
pub fn encoder_input(rgba: &[u8], width: usize, height: usize) -> (Vec<f32>, usize, usize) {
    let (rw, rh) = resized_size(width, height);
    let (sx, sy) = (width as f32 / rw as f32, height as f32 / rh as f32);
    let mut out = vec![0.0f32; rw * rh * 3];
    let at = |x: usize, y: usize, c: usize| rgba[(y * width + x) * 4 + c] as f32;
    for y in 0..rh {
        let fy = ((y as f32 + 0.5) * sy - 0.5).clamp(0.0, (height - 1) as f32);
        let (y0, ty) = (fy as usize, fy.fract());
        let y1 = (y0 + 1).min(height - 1);
        for x in 0..rw {
            let fx = ((x as f32 + 0.5) * sx - 0.5).clamp(0.0, (width - 1) as f32);
            let (x0, tx) = (fx as usize, fx.fract());
            let x1 = (x0 + 1).min(width - 1);
            for c in 0..3 {
                let top = at(x0, y0, c) * (1.0 - tx) + at(x1, y0, c) * tx;
                let bottom = at(x0, y1, c) * (1.0 - tx) + at(x1, y1, c) * tx;
                out[(y * rw + x) * 3 + c] = top * (1.0 - ty) + bottom * ty;
            }
        }
    }
    (out, rh, rw)
}

/// The decoder's `point_coords` and `point_labels` for `clicks` and an
/// optional box (x, y, w, h), all in pixels of a `width × height` frame.
/// Returns the coordinates (flattened x, y pairs), the labels and the count.
pub fn prompt(
    clicks: &[Click],
    bbox: Option<[f32; 4]>,
    width: usize,
    height: usize,
) -> (Vec<f32>, Vec<f32>, usize) {
    let (rw, _) = resized_size(width, height);
    let scale = rw as f32 / width.max(1) as f32;
    let mut coords = Vec::new();
    let mut labels = Vec::new();
    for click in clicks {
        coords.extend([click.x * scale, click.y * scale]);
        labels.push(if click.keep { 1.0 } else { 0.0 });
    }
    match bbox {
        Some([x, y, w, h]) => {
            coords.extend([x * scale, y * scale, (x + w) * scale, (y + h) * scale]);
            labels.extend([2.0, 3.0]);
        }
        None => {
            coords.extend([0.0, 0.0]);
            labels.push(-1.0);
        }
    }
    let n = labels.len();
    (coords, labels, n)
}

/// The decoder's mask logits as bytes: the sigmoid, so the edge keeps the
/// one or two pixels of softness the logits have.
pub fn mask_bytes(logits: &[f32]) -> Vec<u8> {
    logits
        .iter()
        .map(|&v| ((1.0 / (1.0 + (-v).exp())) * 255.0).round() as u8)
        .collect()
}

/// The bounding box (x, y, w, h) of the pixels of `mask` at or above 128,
/// `None` when there are none.
pub fn mask_box(mask: &[u8], width: usize, height: usize) -> Option<[f32; 4]> {
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for y in 0..height {
        let row = &mask[y * width..(y + 1) * width];
        if let Some(first) = row.iter().position(|&a| a >= 128) {
            let last = row.iter().rposition(|&a| a >= 128).unwrap_or(first);
            x0 = x0.min(first);
            x1 = x1.max(last);
            y0 = y0.min(y);
            y1 = y1.max(y);
        }
    }
    (x0 != usize::MAX).then(|| {
        [
            x0 as f32,
            y0 as f32,
            (x1 - x0 + 1) as f32,
            (y1 - y0 + 1) as f32,
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_long_side_goes_to_1024() {
        assert_eq!(resized_size(640, 360), (1024, 576));
        assert_eq!(resized_size(540, 960), (576, 1024));
        assert_eq!(resized_size(1024, 1024), (1024, 1024));
    }

    #[test]
    fn a_prompt_without_a_box_gets_its_padding_point() {
        let clicks = [
            Click {
                x: 320.0,
                y: 180.0,
                keep: true,
            },
            Click {
                x: 10.0,
                y: 20.0,
                keep: false,
            },
        ];
        let (coords, labels, n) = prompt(&clicks, None, 640, 360);
        assert_eq!(n, 3);
        assert_eq!(labels, [1.0, 0.0, -1.0]);
        assert_eq!(&coords[..2], &[512.0, 288.0]);
        let (coords, labels, n) = prompt(&clicks[..1], Some([100.0, 50.0, 200.0, 100.0]), 640, 360);
        assert_eq!(n, 3);
        assert_eq!(labels, [1.0, 2.0, 3.0]);
        assert_eq!(&coords[2..], &[160.0, 80.0, 480.0, 240.0]);
    }

    #[test]
    fn the_encoder_input_is_hwc_in_byte_range() {
        let rgba: Vec<u8> = [10, 20, 30, 255].repeat(4 * 2);
        let (t, h, w) = encoder_input(&rgba, 4, 2);
        assert_eq!((h, w), (512, 1024));
        assert_eq!(t.len(), 512 * 1024 * 3);
        assert_eq!(&t[..3], &[10.0, 20.0, 30.0]);
    }

    #[test]
    fn the_mask_box_covers_the_set_pixels() {
        let mut mask = vec![0u8; 8 * 4];
        mask[8 + 2] = 255;
        mask[2 * 8 + 5] = 200;
        assert_eq!(mask_box(&mask, 8, 4), Some([2.0, 1.0, 4.0, 2.0]));
        assert_eq!(mask_box(&[0; 4], 2, 2), None);
        assert_eq!(mask_bytes(&[-20.0, 0.0, 20.0]), [0, 128, 255]);
    }
}
