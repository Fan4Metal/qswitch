//! Procedurally drawn application icon: a key cap split by a diagonal,
//! blue above it with a white Latin A, white below it with a graphite
//! Cyrillic Ф: one key, two layouts. Dependency-free so that `build.rs`
//! can include this file (`#[path = "src/icon.rs"]`) to produce the `.ico`
//! embedded in the executable and the raw RGBA the app shows (window icon,
//! About), so that nothing is rasterised at start-up.
//!
//! Layers of 24 px and less keep the cap and the split only: letters that
//! small are noise. Edges are anti-aliased over a pixel of the layer being
//! drawn (`px`), so the large layer stays crisp.

// build.rs and the app use different parts.
#![allow(dead_code)]

/// Sizes in the `.ico` of the executable.
pub const ICO_SIZES: [u32; 8] = [16, 20, 24, 32, 40, 48, 64, 256];

/// Straight RGBA, channels 0..1.
type Rgba = [f32; 4];

fn hex(c: u32) -> Rgba {
    let ch = |s: u32| ((c >> s) & 0xff) as f32 / 255.0;
    [ch(16), ch(8), ch(0), 1.0]
}

fn with_alpha(c: Rgba, a: f32) -> Rgba {
    [c[0], c[1], c[2], c[3] * a.clamp(0.0, 1.0)]
}

fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
        a[3] + (b[3] - a[3]) * t,
    ]
}

/// Paint `top` over `under`.
fn over(under: Rgba, top: Rgba) -> Rgba {
    let a = top[3] + under[3] * (1.0 - top[3]);
    if a <= 0.0 {
        return [0.0; 4];
    }
    let mut out = [0.0, 0.0, 0.0, a];
    for i in 0..3 {
        out[i] = (top[i] * top[3] + under[i] * under[3] * (1.0 - top[3])) / a;
    }
    out
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Signed distance from `(x, y)` to a box from `min` to `max` with corners
/// rounded by `r` (negative inside).
fn rounded_box(x: f32, y: f32, min: (f32, f32), max: (f32, f32), r: f32) -> f32 {
    let (cx, cy) = ((min.0 + max.0) / 2.0, (min.1 + max.1) / 2.0);
    let (hx, hy) = ((max.0 - min.0) / 2.0 - r, (max.1 - min.1) / 2.0 - r);
    let qx = (x - cx).abs() - hx;
    let qy = (y - cy).abs() - hy;
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outside + qx.max(qy).min(0.0) - r
}

/// Distance from `(x, y)` to the segment `a`–`b`.
fn segment(x: f32, y: f32, a: (f32, f32), b: (f32, f32)) -> f32 {
    let (px, py) = (x - a.0, y - a.1);
    let (bx, by) = (b.0 - a.0, b.1 - a.1);
    let len2 = bx * bx + by * by;
    let t = if len2 > 0.0 {
        ((px * bx + py * by) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    ((px - bx * t).powi(2) + (py - by * t).powi(2)).sqrt()
}

/// Distance to a stroke of width `w` along the segments of `path`.
fn stroke(x: f32, y: f32, path: &[(f32, f32)], w: f32) -> f32 {
    let mut d = f32::MAX;
    for pair in path.windows(2) {
        d = d.min(segment(x, y, pair[0], pair[1]));
    }
    d - w / 2.0
}

/// Distance to an elliptical ring of radii `r` and stroke `w` around `c`,
/// to the first order (the level set's value over its gradient), which is
/// exact enough for an ellipse this round.
fn oval_ring(x: f32, y: f32, c: (f32, f32), r: (f32, f32), w: f32) -> f32 {
    let (dx, dy) = (x - c.0, y - c.1);
    let f = ((dx / r.0).powi(2) + (dy / r.1).powi(2)).sqrt();
    let grad = ((dx / (r.0 * r.0)).powi(2) + (dy / (r.1 * r.1)).powi(2)).sqrt() / f.max(1e-6);
    ((f - 1.0) / grad.max(1e-6)).abs() - w / 2.0
}

/// Coverage of a shape at distance `d` with an edge of `px`.
fn fill(d: f32, px: f32) -> f32 {
    smoothstep(px / 2.0, -px / 2.0, d)
}

/// The colour of the icon at `(x, y)`, both in 0..1 of the icon's side;
/// `px` is the size of a pixel of the layer being drawn.
fn sample(x: f32, y: f32, px: f32) -> Rgba {
    let small = px > 1.0 / 28.0;
    let mut c = [0.0; 4];
    // The cap's side, a darker slab below the top.
    let side = rounded_box(x, y, (0.07, 0.11), (0.93, 0.95), 0.17);
    c = over(c, with_alpha(hex(0x7d8794), fill(side, px)));
    // The top: the diagonal from the bottom left to the top right splits
    // it; above it blue, below it white, both lit from the top.
    let top = rounded_box(x, y, (0.07, 0.05), (0.93, 0.89), 0.17);
    let diagonal = (x + y - 1.0) / 2f32.sqrt();
    let blue = mix(hex(0x4a86e8), hex(0x2a62c4), (y - 0.05) / 0.84);
    let white = mix(hex(0xfbfcfe), hex(0xe1e6ee), (y - 0.05) / 0.84);
    let face = mix(blue, white, fill(-diagonal, px));
    c = over(c, with_alpha(face, fill(top, px)));
    // A hairline where the halves meet, inside the top.
    let seam = diagonal.abs() - 0.006;
    c = over(
        c,
        with_alpha(hex(0x1f4a9a), fill(seam.max(top + 0.01), px) * 0.35),
    );
    if small {
        return c;
    }
    // A, white, in the blue half: two legs and a bar.
    let w = 0.055;
    let a_legs = stroke(x, y, &[(0.17, 0.57), (0.32, 0.18), (0.47, 0.57)], w);
    let a_bar = stroke(x, y, &[(0.235, 0.455), (0.405, 0.455)], w * 0.9);
    let a = a_legs.min(a_bar);
    c = over(c, with_alpha(hex(0xffffff), fill(a, px)));
    // Ф, graphite, in the white half: a stem with an oval around its
    // middle.
    let stem = stroke(x, y, &[(0.70, 0.42), (0.70, 0.84)], w);
    let oval = oval_ring(x, y, (0.70, 0.63), (0.155, 0.135), w * 0.95);
    let ef = stem.min(oval);
    c = over(c, with_alpha(hex(0x2b3340), fill(ef, px)));
    c
}

/// Straight (non-premultiplied) RGBA pixels, `size * size * 4` bytes, row
/// major from the top. 4x4 supersampling gives smooth edges.
pub fn rgba(size: u32) -> Vec<u8> {
    const SS: u32 = 4;
    let n = size as usize;
    let px = 1.0 / size as f32;
    let mut out = vec![0u8; n * n * 4];
    for py in 0..size {
        for pxi in 0..size {
            // Averaged premultiplied, so transparent samples add no colour.
            let mut acc = [0.0f32; 4];
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = (pxi as f32 + (sx as f32 + 0.5) / SS as f32) / size as f32;
                    let y = (py as f32 + (sy as f32 + 0.5) / SS as f32) / size as f32;
                    let c = sample(x, y, px);
                    for i in 0..3 {
                        acc[i] += c[i] * c[3];
                    }
                    acc[3] += c[3];
                }
            }
            let count = (SS * SS) as f32;
            let alpha = acc[3] / count;
            let i = (py as usize * n + pxi as usize) * 4;
            if alpha > 0.0 {
                for ch in 0..3 {
                    out[i + ch] = (acc[ch] / acc[3] * 255.0).round().clamp(0.0, 255.0) as u8;
                }
                out[i + 3] = (alpha * 255.0).round() as u8;
            }
        }
    }
    out
}

/// Encode the icon as a multi-resolution `.ico` (32-bit BMP entries).
pub fn ico(sizes: &[u32]) -> Vec<u8> {
    let images: Vec<Vec<u8>> = sizes.iter().map(|&s| bmp_entry(s)).collect();
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]); // reserved, type = icon
    out.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (&s, img) in sizes.iter().zip(&images) {
        let dim = if s >= 256 { 0 } else { s as u8 };
        out.extend_from_slice(&[dim, dim, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes()); // planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bpp
        out.extend_from_slice(&(img.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += img.len() as u32;
    }
    for img in images {
        out.extend_from_slice(&img);
    }
    out
}

/// BITMAPINFOHEADER + bottom-up BGRA pixels + 1-bpp AND mask.
fn bmp_entry(size: u32) -> Vec<u8> {
    let px = rgba(size);
    let n = size as usize;
    let mask_stride = n.div_ceil(32) * 4;
    let mut out = Vec::with_capacity(40 + n * n * 4 + mask_stride * n);
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(size as i32).to_le_bytes());
    out.extend_from_slice(&(2 * size as i32).to_le_bytes()); // XOR + AND
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&[0u8; 24]); // compression .. colours important
    for y in (0..n).rev() {
        for x in 0..n {
            let i = (y * n + x) * 4;
            out.extend_from_slice(&[px[i + 2], px[i + 1], px[i], px[i + 3]]);
        }
    }
    // Alpha channel carries transparency; an all-zero AND mask is correct.
    out.resize(out.len() + mask_stride * n, 0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_shape() {
        let px = rgba(64);
        assert_eq!(px.len(), 64 * 64 * 4);
        // The corner is transparent, the centre opaque.
        assert_eq!(px[3], 0);
        let c = ((32 * 64) + 32) * 4;
        assert_eq!(px[c + 3], 255);
        // Blue above the diagonal, white below (beside the Ф), the Ф's
        // stem graphite.
        let at = |x: usize, y: usize| &px[(y * 64 + x) * 4..(y * 64 + x) * 4 + 3];
        assert!(at(12, 8)[2] > at(12, 8)[0] + 60, "{:?}", at(12, 8));
        assert!(at(57, 35).iter().all(|&v| v > 200), "{:?}", at(57, 35));
        assert!(at(45, 50).iter().all(|&v| v < 80), "{:?}", at(45, 50));
        let ico = ico(&[16, 32]);
        assert_eq!(&ico[..6], &[0, 0, 1, 0, 2, 0]);
    }
}
