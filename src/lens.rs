//! Magnify lens: pure pixel code, no Wayland types. A captured window frame is zoomed into a
//! circle around the pointer. Positions are in the overlay's pixels, which are the output's
//! logical pixels; the window's place on the output comes from the compositor's geometry event.

/// One captured window frame in a wl_shm layout with 4 bytes per pixel and no row padding.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub w: usize,
    pub h: usize,
    /// true: bytes are R,G,B,A (Abgr8888/Xbgr8888); false: B,G,R,A (Argb8888/Xrgb8888)
    pub rgb_order: bool,
    pub data: Vec<u8>,
}

/// The window's rectangle on the output, in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geometry {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Frame {
    /// Color at integer pixel (x, y) as (r, g, b), clamped to the frame edges.
    fn rgb(&self, x: i64, y: i64) -> [f64; 3] {
        let x = x.clamp(0, self.w as i64 - 1) as usize;
        let y = y.clamp(0, self.h as i64 - 1) as usize;
        let i = (y * self.w + x) * 4;
        let p = &self.data[i..i + 4];
        let (r, b) = if self.rgb_order { (p[0], p[2]) } else { (p[2], p[0]) };
        [f64::from(r), f64::from(p[1]), f64::from(b)]
    }

    /// Bilinear sample at fractional frame position (pixel centers at +0.5).
    fn sample(&self, fx: f64, fy: f64) -> [f64; 3] {
        let (gx, gy) = (fx - 0.5, fy - 0.5);
        let (x0, y0) = (gx.floor(), gy.floor());
        let (tx, ty) = (gx - x0, gy - y0);
        let (x0, y0) = (x0 as i64, y0 as i64);
        let (a, b, c, d) = (self.rgb(x0, y0), self.rgb(x0 + 1, y0), self.rgb(x0, y0 + 1), self.rgb(x0 + 1, y0 + 1));
        let mut out = [0.0; 3];
        for k in 0..3 {
            let top = a[k] + (b[k] - a[k]) * tx;
            let bottom = c[k] + (d[k] - c[k]) * tx;
            out[k] = top + (bottom - top) * ty;
        }
        out
    }
}

/// Where on the frame the output position (sx, sy) falls, or None when it is outside the window.
pub fn to_frame(frame: &Frame, geom: &Geometry, sx: f64, sy: f64) -> Option<(f64, f64)> {
    if geom.w <= 0.0 || geom.h <= 0.0 {
        return None;
    }
    let fx = (sx - geom.x) * frame.w as f64 / geom.w;
    let fy = (sy - geom.y) * frame.h as f64 / geom.h;
    if fx < 0.0 || fy < 0.0 || fx >= frame.w as f64 || fy >= frame.h as f64 {
        return None;
    }
    Some((fx, fy))
}

/// Draw the zoomed circle into a premultiplied B,G,R,A buffer of `bw` x `bh`. The pixel at offset
/// (dx, dy) from the center `(cx, cy)` shows the content at offset (dx, dy) / `zoom`, so the content
/// under the center stays under the center. Pixels whose content lies outside the window stay empty.
pub fn draw_lens(buf: &mut [u8], bw: usize, bh: usize, cx: f64, cy: f64, r: f64, zoom: f64, frame: &Frame, geom: &Geometry) {
    assert_eq!(buf.len(), bw * bh * 4, "buffer size does not match width and height");
    if frame.w == 0 || frame.h == 0 || zoom <= 0.0 {
        return;
    }
    let (x0, x1) = crate::render::clamp_range(cx - r - 1.0, cx + r + 1.0, bw);
    let (y0, y1) = crate::render::clamp_range(cy - r - 1.0, cy + r + 1.0, bh);
    for y in y0..y1 {
        let dy = y as f64 + 0.5 - cy;
        for x in x0..x1 {
            let dx = x as f64 + 0.5 - cx;
            let cover = (r + 0.5 - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0);
            if cover <= 0.0 {
                continue;
            }
            let Some((fx, fy)) = to_frame(frame, geom, cx + dx / zoom, cy + dy / zoom) else { continue };
            let c = frame.sample(fx, fy);
            let i = (y * bw + x) * 4;
            buf[i] = (c[2] * cover).round() as u8;
            buf[i + 1] = (c[1] * cover).round() as u8;
            buf[i + 2] = (c[0] * cover).round() as u8;
            buf[i + 3] = (cover * 255.0).round() as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame whose red channel is the column, green the row (so a sample tells where it came from).
    fn gradient(w: usize, h: usize, rgb_order: bool) -> Frame {
        let mut data = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 4;
                let (r, g, b) = (x as u8, y as u8, 7u8);
                if rgb_order {
                    data[i..i + 4].copy_from_slice(&[r, g, b, 255]);
                } else {
                    data[i..i + 4].copy_from_slice(&[b, g, r, 255]);
                }
            }
        }
        Frame { w, h, rgb_order, data }
    }

    const BW: usize = 200;
    const BH: usize = 100;

    fn px(buf: &[u8], x: usize, y: usize) -> [u8; 4] {
        let i = (y * BW + x) * 4;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    }

    fn full(w: usize, h: usize) -> Geometry {
        Geometry { x: 0.0, y: 0.0, w: w as f64, h: h as f64 }
    }

    #[test]
    fn to_frame_maps_output_to_window_pixels_and_scales() {
        let f = gradient(100, 50, false);
        let g = Geometry { x: 10.0, y: 20.0, w: 200.0, h: 100.0 }; // frame is half the logical size
        assert_eq!(to_frame(&f, &g, 10.0, 20.0), Some((0.0, 0.0)));
        assert_eq!(to_frame(&f, &g, 110.0, 70.0), Some((50.0, 25.0)));
        assert_eq!(to_frame(&f, &g, 9.9, 30.0), None, "left of the window");
        assert_eq!(to_frame(&f, &g, 210.0, 30.0), None, "right edge is outside");
        assert_eq!(to_frame(&f, &g, 50.0, 120.0), None, "below the window");
    }

    #[test]
    fn to_frame_rejects_an_empty_geometry() {
        let f = gradient(10, 10, false);
        assert_eq!(to_frame(&f, &Geometry { x: 0.0, y: 0.0, w: 0.0, h: 10.0 }, 1.0, 1.0), None);
    }

    #[test]
    fn center_shows_the_content_under_the_pointer() {
        let f = gradient(200, 100, false);
        let mut b = vec![0u8; BW * BH * 4];
        draw_lens(&mut b, BW, BH, 100.0, 50.0, 30.0, 2.0, &f, &full(200, 100));
        let p = px(&b, 100, 50); // pixel center (100.5, 50.5) maps to itself
        assert_eq!(p[3], 255);
        assert!((i32::from(p[2]) - 100).abs() <= 1, "red is the column: {p:?}");
        assert!((i32::from(p[1]) - 50).abs() <= 1, "green is the row: {p:?}");
    }

    #[test]
    fn content_is_magnified_by_the_zoom_factor() {
        let f = gradient(200, 100, false);
        let mut b = vec![0u8; BW * BH * 4];
        draw_lens(&mut b, BW, BH, 100.0, 50.0, 30.0, 2.0, &f, &full(200, 100));
        // 20 px right of the center shows content 10 px right of the center
        let p = px(&b, 120, 50);
        assert!((i32::from(p[2]) - 110).abs() <= 1, "expected column 110, got {p:?}");
        let q = px(&b, 100, 30); // 20 px up shows 10 px up
        assert!((i32::from(q[1]) - 40).abs() <= 1, "expected row 40, got {q:?}");
    }

    #[test]
    fn nothing_is_drawn_outside_the_circle() {
        let f = gradient(200, 100, false);
        let mut b = vec![0u8; BW * BH * 4];
        draw_lens(&mut b, BW, BH, 100.0, 50.0, 30.0, 2.0, &f, &full(200, 100));
        assert_eq!(px(&b, 100, 90), [0, 0, 0, 0]);
        assert_eq!(px(&b, 135, 50), [0, 0, 0, 0]);
        assert_eq!(px(&b, 0, 0), [0, 0, 0, 0]);
    }

    #[test]
    fn rgb_order_frames_give_the_same_picture() {
        let (a, c) = (gradient(200, 100, false), gradient(200, 100, true));
        let (mut ba, mut bc) = (vec![0u8; BW * BH * 4], vec![0u8; BW * BH * 4]);
        draw_lens(&mut ba, BW, BH, 100.0, 50.0, 30.0, 2.0, &a, &full(200, 100));
        draw_lens(&mut bc, BW, BH, 100.0, 50.0, 30.0, 2.0, &c, &full(200, 100));
        assert_eq!(ba, bc);
    }

    #[test]
    fn window_offset_is_respected() {
        // window sits at x=50: output x=100 is frame column 50
        let f = gradient(100, 100, false);
        let g = Geometry { x: 50.0, y: 0.0, w: 100.0, h: 100.0 };
        let mut b = vec![0u8; BW * BH * 4];
        draw_lens(&mut b, BW, BH, 100.0, 50.0, 20.0, 1.0, &f, &g);
        let p = px(&b, 100, 50);
        assert!((i32::from(p[2]) - 50).abs() <= 1, "expected column 50, got {p:?}");
    }

    #[test]
    fn parts_outside_the_window_stay_empty() {
        let f = gradient(100, 100, false);
        let g = Geometry { x: 100.0, y: 0.0, w: 100.0, h: 100.0 }; // window covers x 100..200
        let mut b = vec![0u8; BW * BH * 4];
        draw_lens(&mut b, BW, BH, 100.0, 50.0, 30.0, 1.0, &f, &g);
        assert_eq!(px(&b, 80, 50)[3], 0, "left of the window");
        assert_eq!(px(&b, 120, 50)[3], 255, "inside the window");
    }

    #[test]
    fn edge_is_anti_aliased_and_premultiplied() {
        let f = gradient(200, 100, false);
        let mut b = vec![0u8; BW * BH * 4];
        draw_lens(&mut b, BW, BH, 100.0, 50.0, 30.5, 1.0, &f, &full(200, 100));
        let p = px(&b, 130, 50); // distance 30.0 from the center, radius 30.5: partial cover
        assert!(p[3] > 0 && p[3] < 255, "{p:?}");
        assert!(p[2] <= p[3], "premultiplied color never exceeds alpha: {p:?}");
    }

    #[test]
    fn drawing_near_the_edges_and_with_odd_input_does_not_panic() {
        let f = gradient(20, 20, false);
        let mut b = vec![0u8; BW * BH * 4];
        for (cx, cy) in [(0.0, 0.0), (199.0, 99.0), (-50.0, -50.0), (900.0, 900.0)] {
            draw_lens(&mut b, BW, BH, cx, cy, 40.0, 2.0, &f, &full(200, 100));
        }
        draw_lens(&mut b, BW, BH, 100.0, 50.0, 40.0, 0.0, &f, &full(200, 100));
        let empty = Frame { w: 0, h: 0, rgb_order: false, data: vec![] };
        draw_lens(&mut b, BW, BH, 100.0, 50.0, 40.0, 2.0, &empty, &full(200, 100));
    }
}
