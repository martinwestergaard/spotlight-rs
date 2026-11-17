//! Drawing into a premultiplied ARGB8888 buffer (bytes B, G, R, A on little-endian,
//! which is wl_shm Argb8888). No Wayland types here, so it is testable with pixel checks.
//!
//! Modes: Highlight dims the screen and leaves a clear circle, Circle draws a plain ring,
//! Magnify draws the zoomed window frame inside a ring (see lens.rs), Laser draws a glowing dot.

use crate::config::{parse_color, Config, ModeConfig};
use crate::lens::{draw_lens, Frame, Geometry};
use crate::state_machine::Mode;

pub fn radius_px(m: &ModeConfig, height: usize) -> f64 {
    (m.size_percent / 100.0 * height as f64).max(1.0)
}

fn premul(rgba: [u8; 4], coverage: f64) -> [u8; 4] {
    let a = (f64::from(rgba[3]) / 255.0 * coverage.clamp(0.0, 1.0)).clamp(0.0, 1.0);
    let c = |v: u8| (f64::from(v) * a).round() as u8;
    [c(rgba[2]), c(rgba[1]), c(rgba[0]), (a * 255.0).round() as u8] // B, G, R, A
}

/// Coverage of a ring of `width` centered on radius `r`, anti-aliased, for a pixel at distance `d`.
fn ring_coverage(d: f64, r: f64, width: f64) -> f64 {
    (width / 2.0 + 0.5 - (d - r).abs()).clamp(0.0, 1.0)
}

fn disc_coverage(d: f64, r: f64) -> f64 {
    (r + 0.5 - d).clamp(0.0, 1.0)
}

fn smoothstep(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn put(buf: &mut [u8], w: usize, x: usize, y: usize, px: [u8; 4]) {
    let i = (y * w + x) * 4;
    buf[i..i + 4].copy_from_slice(&px);
}

/// Source-over of a premultiplied pixel onto the existing premultiplied pixel.
fn blend(buf: &mut [u8], w: usize, x: usize, y: usize, src: [u8; 4]) {
    let i = (y * w + x) * 4;
    let inv = 255 - u32::from(src[3]);
    for k in 0..4 {
        let d = u32::from(buf[i + k]);
        buf[i + k] = (u32::from(src[k]) + (d * inv + 127) / 255).min(255) as u8;
    }
}

/// Render one frame. `pos` is the pointer position in buffer pixels; without it nothing is drawn.
pub fn render(buf: &mut [u8], w: usize, h: usize, mode: Mode, pos: Option<(f64, f64)>, cfg: &Config) {
    render_with(buf, w, h, mode, pos, cfg, None, 1.0);
}

/// Like `render`, with the latest captured window frame for Magnify (without one, Magnify is just its
/// ring) and a `scale`: the buffer holds `scale` pixels per logical pixel, so a 3072x1728 screen at
/// scale 1.25 is drawn into a 3840x2160 buffer. `pos` and the capture's geometry are in logical
/// pixels, as are the pixel sizes in the config (ring width, softness, dot radii); mode sizes are
/// percentages of the height and follow the buffer by themselves.
pub fn render_with(buf: &mut [u8], w: usize, h: usize, mode: Mode, pos: Option<(f64, f64)>, cfg: &Config, capture: Option<(&Frame, &Geometry)>, scale: f64) {
    let scaled_cfg;
    let scaled_geom;
    let (cfg, pos, capture) = if (scale - 1.0).abs() < f64::EPSILON {
        (cfg, pos, capture)
    } else {
        let mut c = cfg.clone();
        c.look.ring_width *= scale;
        c.look.edge_softness *= scale;
        c.look.dot_radius *= scale;
        c.look.laser_radius *= scale;
        scaled_cfg = c;
        scaled_geom = capture.map(|(_, g)| Geometry { x: g.x * scale, y: g.y * scale, w: g.w * scale, h: g.h * scale });
        let capture = capture.zip(scaled_geom.as_ref()).map(|((f, _), g)| (f, g));
        (&scaled_cfg, pos.map(|(x, y)| (x * scale, y * scale)), capture)
    };
    assert_eq!(buf.len(), w * h * 4, "buffer size does not match width and height");
    buf.fill(0);
    let Some((cx, cy)) = pos else { return };
    let color = parse_color(&cfg.look.color).unwrap_or([255, 255, 255, 255]);

    match mode {
        Mode::Highlight => highlight(buf, w, h, cx, cy, radius_px(&cfg.modes.highlight, h), cfg),
        Mode::Circle => ring(buf, w, h, cx, cy, radius_px(&cfg.modes.circle, h), cfg, color),
        Mode::Magnify => {
            let r = radius_px(&cfg.modes.magnify, h);
            if let Some((frame, geom)) = capture {
                draw_lens(buf, w, h, cx, cy, r, cfg.look.zoom, frame, geom);
            }
            ring(buf, w, h, cx, cy, r, cfg, color)
        }
        Mode::Laser => {
            let laser = parse_color(&cfg.look.laser_color).unwrap_or([255, 0, 0, 255]);
            laser_dot(buf, w, h, cx, cy, cfg.look.laser_radius, laser);
            return; // the laser is only the dot, not the optional center dot of the other modes
        }
    }

    if cfg.look.dot_radius > 0.0 {
        disc(buf, w, h, cx, cy, cfg.look.dot_radius, color);
    }
}

/// Anti-aliased filled disc, blended over what is already in the buffer.
fn disc(buf: &mut [u8], w: usize, h: usize, cx: f64, cy: f64, r: f64, color: [u8; 4]) {
    let (x0, x1) = clamp_range(cx - r - 1.0, cx + r + 1.0, w);
    let (y0, y1) = clamp_range(cy - r - 1.0, cy + r + 1.0, h);
    for y in y0..y1 {
        for x in x0..x1 {
            let d = ((x as f64 + 0.5 - cx).powi(2) + (y as f64 + 0.5 - cy).powi(2)).sqrt();
            let cov = disc_coverage(d, r);
            if cov > 0.0 {
                blend(buf, w, x, y, premul(color, cov));
            }
        }
    }
}

/// A glowing laser dot of radius `r`: a soft halo out to 1.7 r, a body that is the laser color at the
/// rim and lighter toward the middle, and a near-white hot core. The color's alpha scales all of it.
fn laser_dot(buf: &mut [u8], w: usize, h: usize, cx: f64, cy: f64, r: f64, color: [u8; 4]) {
    let halo = r * 1.7;
    let (x0, x1) = clamp_range(cx - halo - 1.0, cx + halo + 1.0, w);
    let (y0, y1) = clamp_range(cy - halo - 1.0, cy + halo + 1.0, h);
    let mix = |a: f64, b: f64, t: f64| a + (b - a) * t;
    for y in y0..y1 {
        for x in x0..x1 {
            let d = ((x as f64 + 0.5 - cx).powi(2) + (y as f64 + 0.5 - cy).powi(2)).sqrt();
            let body = disc_coverage(d, r);
            // 0 at the center, 1 at the rim
            let u = (d / r).clamp(0.0, 1.0);
            // glow falls off smoothly from the rim to the halo edge, up to 45 percent opaque
            let glow = 0.45 * (1.0 - smoothstep((d - r) / (halo - r))) * (1.0 - body);
            // white-hot core: strong in the middle third, gone by the rim
            let hot = 1.0 - smoothstep((u - 0.1) / 0.7);
            // body brightens from the laser color at the rim toward a light tint in the middle
            let lift = 0.35 * (1.0 - u);
            let rgb: [f64; 3] = std::array::from_fn(|k| {
                let c = f64::from(color[k]);
                let lifted = mix(c, 255.0, lift);
                mix(lifted, 255.0, hot * 0.85)
            });
            let cover = body.max(glow);
            if cover <= 0.0 {
                continue;
            }
            let a = (f64::from(color[3]) / 255.0 * cover).clamp(0.0, 1.0);
            // premultiplied B, G, R, A; the glow keeps the pure laser color
            let (r_, g_, b_) = if body > 0.0 { (rgb[0], rgb[1], rgb[2]) } else { (f64::from(color[0]), f64::from(color[1]), f64::from(color[2])) };
            put(buf, w, x, y, [(b_ * a).round() as u8, (g_ * a).round() as u8, (r_ * a).round() as u8, (a * 255.0).round() as u8]);
        }
    }
}

pub(crate) fn clamp_range(lo: f64, hi: f64, max: usize) -> (usize, usize) {
    let a = lo.floor().max(0.0) as usize;
    let b = (hi.ceil().max(0.0) as usize).min(max);
    (a.min(b), b)
}

fn ring(buf: &mut [u8], w: usize, h: usize, cx: f64, cy: f64, r: f64, cfg: &Config, color: [u8; 4]) {
    let width = cfg.look.ring_width.max(0.5);
    let extent = r + width + 1.0;
    let (x0, x1) = clamp_range(cx - extent, cx + extent, w);
    let (y0, y1) = clamp_range(cy - extent, cy + extent, h);
    for y in y0..y1 {
        for x in x0..x1 {
            let d = ((x as f64 + 0.5 - cx).powi(2) + (y as f64 + 0.5 - cy).powi(2)).sqrt();
            let cov = ring_coverage(d, r, width);
            if cov > 0.0 {
                put(buf, w, x, y, premul(color, cov));
            }
        }
    }
}

fn highlight(buf: &mut [u8], w: usize, h: usize, cx: f64, cy: f64, r: f64, cfg: &Config) {
    let dim = premul([0, 0, 0, 255], cfg.look.dim_level);
    let soft = cfg.look.edge_softness.max(0.0);
    let outer = r + soft;

    // Constant dim row, copied for every row that does not touch the circle.
    let dim_row: Vec<u8> = dim.iter().copied().cycle().take(w * 4).collect();
    let top = cy - outer - 1.0;
    let bottom = cy + outer + 1.0;
    let (x0, x1) = clamp_range(cx - outer - 1.0, cx + outer + 1.0, w);

    for y in 0..h {
        let row = &mut buf[y * w * 4..(y + 1) * w * 4];
        row.copy_from_slice(&dim_row);
        let yf = y as f64 + 0.5;
        if yf < top || yf > bottom {
            continue;
        }
        for x in x0..x1 {
            let d = ((x as f64 + 0.5 - cx).powi(2) + (yf - cy).powi(2)).sqrt();
            // 0 inside the circle, 1 outside the soft edge.
            let t = if soft > 0.0 { smoothstep((d - r) / soft) } else if d > r { 1.0 } else { 0.0 };
            let px = premul([0, 0, 0, 255], cfg.look.dim_level * t);
            row[x * 4..x * 4 + 4].copy_from_slice(&px);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: usize = 400;
    const H: usize = 200;

    fn px(buf: &[u8], x: usize, y: usize) -> [u8; 4] {
        let i = (y * W + x) * 4;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    }

    fn cfg() -> Config {
        let mut c = Config::default();
        c.look.dim_level = 0.5;
        c.look.edge_softness = 4.0;
        c.look.ring_width = 4.0;
        c.modes.highlight.size_percent = 20.0; // radius 40 px at H = 200
        c.modes.circle.size_percent = 20.0;
        c
    }

    fn frame(mode: Mode, pos: Option<(f64, f64)>, c: &Config) -> Vec<u8> {
        let mut b = vec![1u8; W * H * 4]; // non-zero, so clearing is tested
        render(&mut b, W, H, mode, pos, c);
        b
    }

    #[test]
    fn no_pointer_draws_nothing() {
        let b = frame(Mode::Highlight, None, &cfg());
        assert!(b.iter().all(|v| *v == 0));
    }

    #[test]
    fn highlight_is_clear_inside_and_dim_outside() {
        let c = cfg();
        let b = frame(Mode::Highlight, Some((200.0, 100.0)), &c);
        assert_eq!(px(&b, 200, 100)[3], 0, "center is clear");
        assert_eq!(px(&b, 220, 100)[3], 0, "inside radius is clear");
        let far = px(&b, 5, 5);
        assert_eq!(far, [0, 0, 0, 128], "far outside is dimmed at dim_level 0.5");
        assert_eq!(px(&b, 395, 195), far);
    }

    #[test]
    fn highlight_edge_is_a_smooth_ramp() {
        let c = cfg();
        let b = frame(Mode::Highlight, Some((200.0, 100.0)), &c);
        // radius 40, softness 4: alpha rises from 0 at x=240 to 128 at x=244
        let a: Vec<u8> = (238..247).map(|x| px(&b, x, 100)[3]).collect();
        assert!(a.windows(2).all(|p| p[0] <= p[1]), "monotonic: {a:?}");
        assert_eq!(a[0], 0);
        assert_eq!(*a.last().unwrap(), 128);
        assert!(a.iter().any(|v| *v > 0 && *v < 128), "has intermediate values: {a:?}");
    }

    #[test]
    fn highlight_zero_softness_is_a_hard_edge() {
        let mut c = cfg();
        c.look.edge_softness = 0.0;
        let b = frame(Mode::Highlight, Some((200.0, 100.0)), &c);
        assert_eq!(px(&b, 239, 100)[3], 0);
        assert_eq!(px(&b, 241, 100)[3], 128);
    }

    #[test]
    fn highlight_dim_level_zero_draws_nothing_visible() {
        let mut c = cfg();
        c.look.dim_level = 0.0;
        let b = frame(Mode::Highlight, Some((200.0, 100.0)), &c);
        assert!(b.iter().all(|v| *v == 0));
    }

    #[test]
    fn circle_is_a_ring_with_transparent_middle_and_outside() {
        let mut c = cfg();
        c.look.color = "#ff0000".into();
        let b = frame(Mode::Circle, Some((200.0, 100.0)), &c);
        assert_eq!(px(&b, 200, 100), [0, 0, 0, 0], "middle is empty");
        assert_eq!(px(&b, 5, 5), [0, 0, 0, 0], "outside is empty");
        // on the ring: opaque red, premultiplied B,G,R,A
        assert_eq!(px(&b, 240, 100), [0, 0, 255, 255]);
        assert_eq!(px(&b, 160, 100), [0, 0, 255, 255]);
    }

    #[test]
    fn ring_color_with_alpha_is_premultiplied() {
        let mut c = cfg();
        c.look.color = "#ff000080".into();
        let b = frame(Mode::Circle, Some((200.0, 100.0)), &c);
        let p = px(&b, 240, 100);
        assert_eq!(p[3], 128);
        assert_eq!(p[2], 128, "red premultiplied by alpha");
        assert_eq!(p[0], 0);
    }

    #[test]
    fn dot_is_drawn_at_the_pointer_when_enabled() {
        let mut c = cfg();
        c.look.dot_radius = 5.0;
        c.look.color = "#00ff00".into();
        let b = frame(Mode::Circle, Some((200.0, 100.0)), &c);
        assert_eq!(px(&b, 200, 100), [0, 255, 0, 255]);
        assert_eq!(px(&b, 210, 100), [0, 0, 0, 0], "outside the dot, inside the ring");
    }

    #[test]
    fn laser_has_a_hot_core_a_colored_rim_and_a_fading_glow() {
        let mut c = cfg();
        c.look.laser_color = "#ff0000".into();
        c.look.laser_radius = 20.0;
        let b = frame(Mode::Laser, Some((200.0, 100.0)), &c);
        let core = px(&b, 200, 100);
        assert_eq!(core[3], 255, "core is opaque");
        assert!(core[0] > 150 && core[1] > 150, "core is near white: {core:?}");
        let rim = px(&b, 218, 100); // 18 px out, inside the body
        assert_eq!(rim[3], 255);
        assert!(rim[2] > rim[0] + 60 && rim[2] > rim[1] + 60, "rim is red: {rim:?}");
        let near = px(&b, 225, 100)[3]; // just outside the rim: glow
        let far = px(&b, 232, 100)[3];
        assert!(near > 0 && far > 0 && near > far, "glow fades outward: {near} {far}");
        assert!(near < 255, "glow is translucent");
        assert_eq!(px(&b, 260, 100), [0, 0, 0, 0], "nothing past the halo");
        assert_eq!(px(&b, 5, 5), [0, 0, 0, 0], "no dimming anywhere");
    }

    #[test]
    fn laser_glow_keeps_the_laser_color_and_is_premultiplied() {
        let mut c = cfg();
        c.look.laser_color = "#ff0000".into();
        c.look.laser_radius = 20.0;
        let b = frame(Mode::Laser, Some((200.0, 100.0)), &c);
        let g = px(&b, 230, 100);
        assert!(g[3] > 0 && g[2] == g[3] && g[0] == 0 && g[1] == 0, "{g:?}");
    }

    #[test]
    fn drawing_near_the_edges_does_not_panic() {
        let mut c = cfg();
        c.look.dot_radius = 5.0;
        for pos in [(0.0, 0.0), (399.0, 199.0), (-50.0, -50.0), (900.0, 900.0), (0.0, 199.0)] {
            for mode in [Mode::Highlight, Mode::Circle, Mode::Magnify, Mode::Laser] {
                let _ = frame(mode, Some(pos), &c);
            }
        }
    }

    #[test]
    fn magnify_with_a_frame_zooms_inside_the_ring() {
        let mut c = cfg();
        c.modes.magnify.size_percent = 20.0; // radius 40
        c.look.zoom = 2.0;
        c.look.color = "#ff0000".into();
        let mut data = vec![0u8; W * H * 4];
        for y in 0..H {
            for x in 0..W {
                let i = (y * W + x) * 4;
                data[i..i + 4].copy_from_slice(&[0, 200, x as u8, 255]); // B,G,R,A: red is the column
            }
        }
        let f = Frame { w: W, h: H, rgb_order: false, data };
        let g = Geometry { x: 0.0, y: 0.0, w: W as f64, h: H as f64 };
        let mut b = vec![1u8; W * H * 4];
        render_with(&mut b, W, H, Mode::Magnify, Some((200.0, 100.0)), &c, Some((&f, &g)), 1.0);
        let inside = px(&b, 220, 100); // 20 px right of center shows column 210
        assert_eq!(inside[3], 255);
        assert!((i32::from(inside[2]) - 210).abs() <= 1, "{inside:?}");
        assert_eq!(px(&b, 240, 100), [0, 0, 255, 255], "the ring is drawn over the lens");
        assert_eq!(px(&b, 5, 5), [0, 0, 0, 0], "nothing outside");
    }

    #[test]
    fn magnify_without_a_frame_is_only_the_ring() {
        let mut c = cfg();
        c.modes.magnify.size_percent = 20.0;
        let b = frame(Mode::Magnify, Some((200.0, 100.0)), &c);
        assert_eq!(px(&b, 200, 100), [0, 0, 0, 0]);
        assert_ne!(px(&b, 240, 100)[3], 0);
    }

    #[test]
    fn scale_enlarges_pixel_sizes_and_positions() {
        let mut c = cfg();
        c.look.laser_color = "#ff0000".into();
        c.look.laser_radius = 10.0;
        let (w, h) = (2 * W, 2 * H);
        let mut b = vec![0u8; w * h * 4];
        render_with(&mut b, w, h, Mode::Laser, Some((200.0, 100.0)), &c, None, 2.0);
        let at = |x: usize, y: usize| b[(y * w + x) * 4 + 3];
        assert_eq!(at(400, 200), 255, "the logical point (200,100) is buffer pixel (400,200)");
        assert_eq!(at(400 + 15, 200), 255, "radius 10 became 20: 15 px out is still the body");
        assert_eq!(at(5, 5), 0);
    }

    #[test]
    fn scale_one_matches_the_plain_render() {
        let c = cfg();
        let (mut a, mut b) = (vec![0u8; W * H * 4], vec![0u8; W * H * 4]);
        render(&mut a, W, H, Mode::Highlight, Some((200.0, 100.0)), &c);
        render_with(&mut b, W, H, Mode::Highlight, Some((200.0, 100.0)), &c, None, 1.0);
        assert_eq!(a, b);
    }

    #[test]
    fn scale_moves_the_magnified_window_with_it() {
        let mut c = cfg();
        c.modes.magnify.size_percent = 20.0;
        c.look.zoom = 1.0;
        let mut data = vec![0u8; 100 * 100 * 4];
        for y in 0..100 {
            for x in 0..100 {
                let i = (y * 100 + x) * 4;
                data[i..i + 4].copy_from_slice(&[0, 0, x as u8, 255]); // red is the frame column
            }
        }
        let f = Frame { w: 100, h: 100, rgb_order: false, data };
        let g = Geometry { x: 50.0, y: 0.0, w: 100.0, h: 100.0 }; // logical: window covers x 50..150
        let (w, h) = (2 * W, 2 * H);
        let mut b = vec![0u8; w * h * 4];
        render_with(&mut b, w, h, Mode::Magnify, Some((100.0, 50.0)), &c, Some((&f, &g)), 2.0);
        let p = (50 * w + 210) * 4; // buffer (210,100) = logical (105,50) = frame column 55
        assert_eq!(b[p + 3], 255);
        assert!((i32::from(b[p + 2]) - 55).abs() <= 1, "{}", b[p + 2]);
    }

    #[test]
    fn size_percent_scales_with_screen_height() {
        let m = ModeConfig { enabled: true, size_percent: 10.0 };
        assert_eq!(radius_px(&m, 1728), 172.8);
        assert_eq!(radius_px(&m, 100), 10.0);
    }
}
