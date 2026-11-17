//! The overlay: a full-screen layer-shell surface that is created on show and destroyed on hide,
//! draws the current pointer mode into a shared-memory buffer, and takes the pointer position
//! from the compositor. Also the Wayland event handlers for it.

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, Region},
    delegate_compositor, delegate_layer, delegate_output, delegate_pointer, delegate_registry,
    delegate_seat, delegate_shm,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
        Capability, SeatHandler, SeatState,
    },
    shell::{
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
        WaylandSurface,
    },
    shm::{slot::SlotPool, Shm, ShmHandler},
};
use spotlight_rs::{
    window_capture::Capturer,
    render,
    state_machine::{Effect, Mode, State},
};
use std::sync::atomic::Ordering;
use wayland_client::{
    protocol::{wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
    Connection, Dispatch, QueueHandle,
};
use wayland_protocols::wp::{
    fractional_scale::v1::client::{
        wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
        wp_fractional_scale_v1::{self, WpFractionalScaleV1},
    },
    viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter},
};
use crate::{remote::vibrate_async, STOP};

pub(crate) struct App {
    pub(crate) registry_state: RegistryState,
    pub(crate) output_state: OutputState,
    pub(crate) seat_state: SeatState,
    pub(crate) shm: Shm,
    pub(crate) pool: SlotPool,
    pub(crate) layer: Option<LayerSurface>,
    pub(crate) compositor: CompositorState,
    pub(crate) layer_shell: LayerShell,
    pub(crate) qh: QueueHandle<App>,
    pub(crate) empty_region: Region,
    pub(crate) pointer: Option<wl_pointer::WlPointer>,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) configured: bool,
    pub(crate) click_through: bool,
    pub(crate) pointer_override: Option<(f64, f64)>,
    pub(crate) mode_override: Option<Mode>,
    pub(crate) state: State,
    pub(crate) pending: Vec<Effect>,
    pub(crate) capturer: Option<Capturer>,
    /// Sharp drawing at fractional scales: wp_fractional_scale tells the scale, wp_viewporter lets the
    /// buffer be larger than the surface. Without both, the buffer is logical size and gets stretched.
    pub(crate) fractional: Option<(WpFractionalScaleManagerV1, WpViewporter)>,
    pub(crate) surface_scale: Option<(WpFractionalScaleV1, WpViewport)>,
    /// Pixels per logical pixel; 1.0 until the compositor says otherwise.
    pub(crate) scale: f64,
}

impl App {
    /// Create a fresh layer surface. COSMIC did not answer a re-map of an unmapped surface
    /// (no new configure), so hiding destroys the surface and showing creates a new one.
    pub(crate) fn show(&mut self) {
        if self.layer.is_some() {
            return;
        }
        self.configured = false;
        let surface = self.compositor.create_surface(&self.qh);
        let layer = self.layer_shell.create_layer_surface(&self.qh, surface, Layer::Overlay, Some("spotlight-rs"), None);
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        if self.click_through {
            layer.set_input_region(Some(self.empty_region.wl_region()));
        }
        if let Some((manager, viewporter)) = &self.fractional {
            let scale = manager.get_fractional_scale(layer.wl_surface(), &self.qh, ());
            let viewport = viewporter.get_viewport(layer.wl_surface(), &self.qh, ());
            self.surface_scale = Some((scale, viewport));
        }
        // Mapping happens when the compositor answers with a configure (see configure()).
        layer.commit();
        self.layer = Some(layer);
    }

    pub(crate) fn hide(&mut self) {
        self.configured = false;
        if let Some((scale, viewport)) = self.surface_scale.take() {
            scale.destroy();
            viewport.destroy();
        }
        self.layer = None; // dropping destroys the layer surface and its wl_surface
    }

    pub(crate) fn draw(&mut self) {
        if !self.configured {
            return;
        }
        let Some(layer) = self.layer.as_ref() else { return };
        // With a viewport the buffer holds physical pixels and is mapped onto the logical surface size.
        let scale = if self.surface_scale.is_some() { self.scale } else { 1.0 };
        let (w, h) = ((f64::from(self.width) * scale).round() as usize, (f64::from(self.height) * scale).round() as usize);
        let mode = self.current_mode();
        let shot = if mode == Mode::Magnify { self.capturer.as_ref().and_then(Capturer::latest) } else { None };
        let (buffer, canvas) = self
            .pool
            .create_buffer(w as i32, h as i32, (w * 4) as i32, wl_shm::Format::Argb8888)
            .expect("create buffer");
        let pos = self.pointer_override.or(self.state.pointer());
        render::render_with(canvas, w, h, mode, pos, self.state.config(), shot.as_ref().map(|(f, g)| (&**f, g)), scale);
        if let Some((_, viewport)) = &self.surface_scale {
            viewport.set_destination(self.width as i32, self.height as i32);
        }
        layer.wl_surface().damage_buffer(0, 0, w as i32, h as i32);
        buffer.attach_to(layer.wl_surface()).expect("attach");
        layer.commit();
    }

    /// What is drawn now: the laser during a laser hold, else the demo's mode if set, else the chosen mode.
    pub(crate) fn current_mode(&self) -> Mode {
        self.mode_override.filter(|_| !self.state.laser_held()).unwrap_or(self.state.effective_mode())
    }

    /// Capture windows only while Magnify is on screen.
    pub(crate) fn sync_capture(&self) {
        if let Some(c) = &self.capturer {
            c.set_active(self.layer.is_some() && self.current_mode() == Mode::Magnify);
        }
    }

    pub(crate) fn apply(&mut self, effects: Vec<Effect>) {
        for e in effects {
            match e {
                Effect::ShowOverlay => self.show(),
                Effect::HideOverlay => self.hide(),
                Effect::Redraw => self.draw(),
                Effect::ModeChanged(m) => eprintln!("mode: {m:?}"),
                Effect::TimerStarted => eprintln!("timer started"),
                Effect::VibrateWarning => {
                    let t = &self.state.config().timer;
                    vibrate_async("timer warning", t.warn_length, t.vibration_intensity);
                }
                Effect::VibrateEnd => {
                    let t = &self.state.config().timer;
                    vibrate_async("timer end", t.end_length, t.vibration_intensity);
                }
            }
        }
        self.sync_capture();
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: i32) {}
    fn transform_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: wl_output::Transform) {}
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}
    fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
    fn surface_leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState { &mut self.output_state }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {
        eprintln!("the compositor closed the overlay surface");
        STOP.store(true, Ordering::SeqCst);
    }

    fn configure(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface, configure: LayerSurfaceConfigure, _: u32) {
        self.width = configure.new_size.0.max(1);
        self.height = configure.new_size.1.max(1);
        self.configured = true;
        trace!("configured {}x{} (visible: {})", self.width, self.height, self.layer.is_some());
        if self.layer.is_some() {
            self.draw();
        }
    }
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm { &mut self.shm }
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState { &mut self.seat_state }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
    fn new_capability(&mut self, _: &Connection, qh: &QueueHandle<Self>, seat: wl_seat::WlSeat, capability: Capability) {
        if capability == Capability::Pointer && self.pointer.is_none() {
            self.pointer = self.seat_state.get_pointer(qh, &seat).ok();
        }
    }
    fn remove_capability(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat, capability: Capability) {
        if capability == Capability::Pointer {
            self.pointer = None;
        }
    }
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl PointerHandler for App {
    fn pointer_frame(&mut self, _: &Connection, _: &QueueHandle<Self>, pointer: &wl_pointer::WlPointer, events: &[PointerEvent]) {
        for e in events {
            // For modes listed in look.hide_cursor (the laser dot, the highlight) the drawing is the
            // pointer: ask the compositor to show no cursor over this surface (a null cursor surface).
            // Applied when the pointer enters, which happens on every show; a mode switch while the
            // overlay is up takes effect on the next show.
            if let PointerEventKind::Enter { serial } = e.kind {
                if self.state.hides_cursor() {
                    pointer.set_cursor(serial, None, 0, 0);
                }
            }
            if matches!(e.kind, PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. }) {
                let fx = self.state.pointer_at(e.position.0, e.position.1);
                self.pending.extend(fx);
            }
        }
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState { &mut self.registry_state }
    registry_handlers![OutputState];
}

impl Dispatch<WpFractionalScaleManagerV1, ()> for App {
    fn event(_: &mut Self, _: &WpFractionalScaleManagerV1, _: <WpFractionalScaleManagerV1 as wayland_client::Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WpViewporter, ()> for App {
    fn event(_: &mut Self, _: &WpViewporter, _: <WpViewporter as wayland_client::Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WpViewport, ()> for App {
    fn event(_: &mut Self, _: &WpViewport, _: <WpViewport as wayland_client::Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

/// The last scale that was logged (as f64 bits), so the log line appears once.
static LOGGED_SCALE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl Dispatch<WpFractionalScaleV1, ()> for App {
    fn event(app: &mut Self, _: &WpFractionalScaleV1, event: wp_fractional_scale_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        // The scale arrives in units of 1/120.
        let wp_fractional_scale_v1::Event::PreferredScale { scale } = event else { return };
        app.scale = f64::from(scale) / 120.0;
        // Once, and again if it changes: it tells whether drawing is sharp on this screen.
        let bits = app.scale.to_bits();
        if LOGGED_SCALE.swap(bits, std::sync::atomic::Ordering::Relaxed) != bits {
            eprintln!("scale {:.3}", app.scale);
        }
        if app.configured && app.layer.is_some() {
            app.draw();
        }
    }
}

delegate_compositor!(App);
delegate_output!(App);
delegate_shm!(App);
delegate_layer!(App);
delegate_registry!(App);
delegate_seat!(App);
delegate_pointer!(App);
