//! What a frame actually cost (SPEC §14.2 item 10).
//!
//! Every remaining "written like WebGL" item in §14.2 is a performance claim —
//! bindless would merge draws, render bundles would skip re-encoding, parallel
//! updates would use the other cores. None of them can be judged, because until
//! now this player reported nothing about a frame at all. WebGL is why: its
//! timer query extension (`EXT_disjoint_timer_query`) was withdrawn from browsers
//! over Spectre, so the reference implementation had no GPU clock to read and
//! never grew the habit of reading one.
//!
//! WebGPU gives the clock back. This module is the smallest thing that turns
//! §14.2's proposals into measurable ones:
//!
//! * **counts** — primitives, draw calls, bytes uploaded. Free, always available,
//!   and the direct read-out for the items that are about issuing less work.
//! * **GPU pass time** — from a timestamp query, when the device has the feature.
//!
//! CPU time is deliberately absent: the host can wrap `SSScene::update` in its
//! own clock in one line, and `std::time::Instant` panics on
//! `wasm32-unknown-unknown`, so measuring it here would cost a dependency to
//! serve a caller that does not need the help.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// What one frame of a [`crate::SSScene`] cost.
///
/// Counts are exact and always populated. `gpu_pass_time_ns` needs
/// `Features::TIMESTAMP_QUERY` on the device **and** lags by a frame or two —
/// reading it back without stalling the pipeline means reading it late. For a
/// profiling counter that is the right trade; do not treat it as this frame's.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SceneStats {
    /// Players that contributed geometry (invisible ones are skipped entirely).
    pub players: u32,
    /// Primitives in the frame's table — parts, shape fans and effect particles.
    pub prims: u32,
    /// Draw calls recorded. The gap between this and `prims` is what draw-call
    /// merging (SPEC §14.2 item 4/§12.1 7-8) has to close.
    pub draw_calls: u32,
    /// Bytes written to the SoA arena, the primitive table and the mesh stream.
    pub uploaded_bytes: u64,
    /// Draw calls that could be folded into the one before them as they stand:
    /// same pipeline, same texture, and a vertex range that carries straight on.
    ///
    /// The ceiling for draw-call merging (SPEC §12.1 7-8), measurable before
    /// writing it. `draw_calls - mergeable_draws` is what the frame would cost
    /// with merging in place.
    pub mergeable_draws: u32,
    /// Texture bind-group sets that repeat the previous draw's texture, and so
    /// need not be issued at all (SPEC §14.2 item 7).
    ///
    /// Counted separately from `mergeable_draws` because it is a strictly easier
    /// win: it needs no change to how draws are built, only to how they are
    /// recorded.
    pub redundant_texture_binds: u32,
    /// GPU time for the last render pass whose timestamps came back, in
    /// nanoseconds. `None` until one has (or forever, without the feature).
    pub gpu_pass_time_ns: Option<u64>,
}

/// Sentinel for "no measurement yet" in the shared slot. Real durations never
/// reach it, and it keeps the slot to one lock-free word.
const NO_TIME: u64 = u64::MAX;

/// GPU-side timing for one render target: a two-slot query set, the buffers to
/// get it back to the CPU, and where the answer lands.
///
/// Only pass-level timing (`beginning_of_pass` / `end_of_pass`) is used, because
/// that is what plain WebGPU supports; per-draw timing needs
/// `TIMESTAMP_QUERY_INSIDE_PASSES`, which is native-only. Pass level is enough to
/// answer "did that change make the frame cheaper".
pub(crate) struct GpuTimer {
    /// Four slots: two pairs, used on alternate frames. See `resolve`.
    query_set: wgpu::QuerySet,
    /// `resolve_query_set` target. Cannot be mapped, hence the copy below.
    resolve: wgpu::Buffer,
    /// Mappable copy of `resolve`.
    readback: wgpu::Buffer,
    /// Nanoseconds per timestamp tick, from `Queue::get_timestamp_period`.
    period_ns: f32,
    /// Written by the map callback, read by `stats()`. Shared because the
    /// callback outlives the call that registered it.
    last_ns: Arc<AtomicU64>,
    /// Raised by the map callback. Separate from `last_ns` because that keeps its
    /// value between samples, so it cannot also say "a new one arrived".
    landed: Arc<AtomicBool>,
    /// Which query pair the next pass writes.
    pair: usize,
    /// A pair written by an earlier frame, waiting to be resolved.
    pending: Option<usize>,
    /// A resolve was encoded and is waiting to be mapped. Mapping has to wait for
    /// the host's submit, so it happens on the following frame.
    resolved: bool,
    /// A map is in flight; starting a second one on the same buffer is an error.
    mapping: bool,
}

impl GpuTimer {
    /// Build the timing resources, or `None` if the device cannot time.
    ///
    /// The device decides, not us: `Features::TIMESTAMP_QUERY` has to be asked
    /// for at `request_device` time, so a host that wants numbers opts in and
    /// everything here stays absent for one that does not.
    pub(crate) fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Option<Self> {
        if !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            return None;
        }
        let query_set = device.create_query_set(&wgpu::QuerySetDescriptor { label: Some("SpriteStudio Frame Timer"), ty: wgpu::QueryType::Timestamp, count: 4 });
        // Two u64 timestamps.
        const BYTES: u64 = 16;
        let resolve = device.create_buffer(&wgpu::BufferDescriptor { label: Some("SpriteStudio Timestamp Resolve"), size: BYTES, usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false });
        let readback = device.create_buffer(&wgpu::BufferDescriptor { label: Some("SpriteStudio Timestamp Readback"), size: BYTES, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        Some(Self { query_set, resolve, readback, period_ns: queue.get_timestamp_period(), last_ns: Arc::new(AtomicU64::new(NO_TIME)), landed: Arc::new(AtomicBool::new(false)), pair: 0, pending: None, resolved: false, mapping: false })
    }

    /// The timestamp writes a host attaches to its render pass descriptor.
    pub(crate) fn writes(&self) -> wgpu::RenderPassTimestampWrites<'_> {
        let base = (self.pair * 2) as u32;
        wgpu::RenderPassTimestampWrites { query_set: &self.query_set, beginning_of_pass_write_index: Some(base), end_of_pass_write_index: Some(base + 1) }
    }

    /// Copy an **earlier** frame's timestamps somewhere the CPU can reach.
    ///
    /// Not this frame's. Resolving a pair in the same command buffer that wrote
    /// it reads the counters before they have retired, which comes back as
    /// `[start, 0]` or `[0, 0]` — often enough to make the whole measurement
    /// untrustworthy. So the pairs alternate: this frame's pass writes one pair
    /// while the resolve encoded here reads the other, whose command buffer the
    /// GPU has already finished.
    pub(crate) fn resolve(&mut self, encoder: &mut wgpu::CommandEncoder) {
        if !self.mapping
            && let Some(pair) = self.pending.take()
        {
            let base = (pair * 2) as u32;
            encoder.resolve_query_set(&self.query_set, base..base + 2, &self.resolve, 0);
            encoder.copy_buffer_to_buffer(&self.resolve, 0, &self.readback, 0, self.readback.size());
            self.resolved = true;
        }
        // Whatever the pass just wrote becomes the next candidate, and the next
        // pass writes the other pair.
        self.pending = Some(self.pair);
        self.pair ^= 1;
    }

    /// Start reading back whatever the previous frame resolved.
    ///
    /// Called at the top of a frame rather than straight after `resolve`, because
    /// the host has not submitted that encoder yet at the point `resolve` runs.
    pub(crate) fn poll(&mut self) {
        if !self.resolved || self.mapping {
            return;
        }
        self.resolved = false;
        self.mapping = true;

        let slot = Arc::clone(&self.last_ns);
        let landed = Arc::clone(&self.landed);
        let buffer = self.readback.clone();
        let period = self.period_ns;
        self.readback.slice(..).map_async(wgpu::MapMode::Read, move |result| {
            if result.is_ok() {
                let view = buffer.slice(..).get_mapped_range();
                let mut ticks = [0u64; 2];
                bytemuck::cast_slice_mut(&mut ticks).copy_from_slice(&view[..16]);
                drop(view);
                buffer.unmap();
                // A backwards or absent pair means the driver dropped the query;
                // report nothing rather than a nonsense duration.
                if let Some(delta) = ticks[1].checked_sub(ticks[0]) {
                    slot.store((delta as f64 * period as f64) as u64, Ordering::Relaxed);
                }
            }
            landed.store(true, Ordering::Release);
        });
    }

    /// Clear the way for the next measurement once the in-flight map has landed.
    /// Cheap enough to check every frame.
    pub(crate) fn settle(&mut self) {
        if self.mapping && self.landed.swap(false, Ordering::Acquire) {
            self.mapping = false;
        }
    }

    pub(crate) fn last_ns(&self) -> Option<u64> {
        match self.last_ns.load(Ordering::Relaxed) {
            NO_TIME => None,
            ns => Some(ns),
        }
    }
}
