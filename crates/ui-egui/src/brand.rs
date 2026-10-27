//! The app icon, drawn in code (a violet tile with a lightning bolt between
//! curly braces), so the window icon, the Windows .exe icon, the packaged
//! icons and the sidebar logo all come from one place. `web/favicon.svg` is
//! the same drawing by hand; keep the two in step.

const SAMPLES: u32 = 4;

/// The tile colour.
pub const TILE: [u8; 3] = [0x5f, 0x3d, 0xc4];

/// Half the brace stroke width, in the 24-unit design grid.
const BRACE_W: f32 = 1.25;

/// The left brace as cubic Béziers (start, control 1, control 2, end) and
/// straight runs; the right brace is its mirror image about x = 12.
const BRACE: [[(f32, f32); 4]; 6] = [
    [(8.5, 5.5), (6.9, 5.5), (6.0, 6.3), (6.0, 7.8)],
    [(6.0, 7.8), (6.0, 8.4), (6.0, 9.1), (6.0, 9.7)],
    [(6.0, 9.7), (6.0, 11.0), (5.3, 12.0), (4.0, 12.0)],
    [(4.0, 12.0), (5.3, 12.0), (6.0, 13.0), (6.0, 14.3)],
    [(6.0, 14.3), (6.0, 14.9), (6.0, 15.6), (6.0, 16.2)],
    [(6.0, 16.2), (6.0, 17.7), (6.9, 18.5), (8.5, 18.5)],
];

/// The bolt outline, filled, with corners softened by `BOLT_ROUND`.
const BOLT: [(f32, f32); 6] = [(13.2, 6.5), (9.4, 12.6), (12.4, 12.6), (11.3, 17.5), (15.2, 11.2), (12.1, 11.2)];
const BOLT_ROUND: f32 = 0.4;

fn dist_to_segment(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let t = (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    let (cx, cy) = (a.0 + t * dx, a.1 + t * dy);
    ((p.0 - cx).powi(2) + (p.1 - cy).powi(2)).sqrt()
}

fn bezier(c: &[(f32, f32); 4], t: f32) -> (f32, f32) {
    let u = 1.0 - t;
    let (a, b, cc, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    (a * c[0].0 + b * c[1].0 + cc * c[2].0 + d * c[3].0, a * c[0].1 + b * c[1].1 + cc * c[2].1 + d * c[3].1)
}

/// The left brace flattened into a polyline.
fn brace_polyline() -> Vec<(f32, f32)> {
    let steps = 12;
    let mut pts = vec![BRACE[0][0]];
    for c in &BRACE {
        pts.extend((1..=steps).map(|i| bezier(c, i as f32 / steps as f32)));
    }
    pts
}

fn in_polygon(p: (f32, f32), poly: &[(f32, f32)]) -> bool {
    let mut inside = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[j]);
        if (a.1 > p.1) != (b.1 > p.1) && p.0 < (b.0 - a.0) * (p.1 - a.1) / (b.1 - a.1) + a.0 {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Coverage of the tile (rounded square) and of the white mark at a point,
/// in a 24-unit design grid.
fn shape(x: f32, y: f32, brace: &[(f32, f32)]) -> (bool, bool) {
    let r = 6.0;
    let (cx, cy) = (x.clamp(r, 24.0 - r), y.clamp(r, 24.0 - r));
    let in_tile = (x - cx).powi(2) + (y - cy).powi(2) <= r * r && (0.0..=24.0).contains(&x) && (0.0..=24.0).contains(&y);
    // Fold the right half onto the left so one brace covers both.
    let folded = (x.min(24.0 - x), y);
    let near_brace = brace.windows(2).any(|s| dist_to_segment(folded, s[0], s[1]) < BRACE_W);
    let near_bolt = in_polygon((x, y), &BOLT) || (0..BOLT.len()).any(|i| dist_to_segment((x, y), BOLT[i], BOLT[(i + 1) % BOLT.len()]) < BOLT_ROUND);
    (in_tile, in_tile && (near_brace || near_bolt))
}

/// Straight (non-premultiplied) RGBA pixels of the icon at `size` x `size`.
pub fn icon_rgba(size: u32) -> Vec<u8> {
    let brace = brace_polyline();
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    let scale = 24.0 / size as f32;
    for py in 0..size {
        for px in 0..size {
            let (mut tile, mut mark) = (0u32, 0u32);
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let x = (px as f32 + (sx as f32 + 0.5) / SAMPLES as f32) * scale;
                    let y = (py as f32 + (sy as f32 + 0.5) / SAMPLES as f32) * scale;
                    let (t, m) = shape(x, y, &brace);
                    tile += t as u32;
                    mark += m as u32;
                }
            }
            let n = (SAMPLES * SAMPLES) as f32;
            let alpha = tile as f32 / n;
            let white = if tile > 0 { mark as f32 / tile as f32 } else { 0.0 };
            for c in TILE {
                rgba.push((c as f32 + (255.0 - c as f32) * white).round() as u8);
            }
            rgba.push((alpha * 255.0).round() as u8);
        }
    }
    rgba
}
