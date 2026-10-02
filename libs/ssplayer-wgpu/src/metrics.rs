//! What a frame cost: counts that are always available, plus GPU pass time when
//! the device supports timestamp queries.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// What one frame of a [`crate::SSScene`] cost.
///
/// Counts are exact and always populated. `gpu_pass_time_ns` needs
/// `Features::TIMESTAMP_QUERY` on the device and lags by a frame or two, so it
/// is a profiling counter rather than this frame's number.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SceneStats {
    /// Players that contributed geometry (hidden and culled ones do not).
    pub players: u32,
    /// Primitives in the frame's table — parts, shape fans and effect particles.
    pub prims: u32,
    /// Draw calls recorded.
    pub draw_calls: u32,
    /// Bytes written to the SoA arena, the primitive table and the mesh stream.
    pub uploaded_bytes: u64,
    /// Draw calls that could be folded into the one before them as they stand:
    /// same pipeline, same texture, and a vertex range that carries straight on.
    pub mergeable_draws: u32,
    /// Texture bind-group sets that repeat the previous draw's texture.
    pub redundant_texture_binds: u32,
    /// Coverage bitmaps rasterised this frame — one per masking player, and one
    /// more for each further 24 mask writers in its instance tree, each its own
    /// render pass. Always zero under the stencil backend, which is the point
    /// of having the number: it is what the coverage backend costs.
    pub coverage_layers: u32,
    /// GPU time for the last render pass whose timestamps came back, in
    /// nanoseconds. `None` until one has, or always without the feature.
    pub gpu_pass_time_ns: Option<u64>,
}

/// Sentinel for "no measurement yet"; real durations never reach it.
const NO_TIME: u64 = u64::MAX;

/// GPU-side timing for one render target.
///
/// Times the render pass as a whole (`beginning_of_pass` / `end_of_pass`).
/// Per-draw timing needs `TIMESTAMP_QUERY_INSIDE_PASSES`, which is native-only.
pub(crate) struct GpuTimer {
    /// Four slots: two pairs, used on alternate frames. See `resolve`.
    query_set: wgpu::QuerySet,
    /// `resolve_query_set` target. Cannot be mapped, hence the copy below.
    resolve: wgpu::Buffer,
    /// Mappable copy of `resolve`.
    readback: wgpu::Buffer,
    /// Nanoseconds per timestamp tick, from `Queue::get_timestamp_period`.
    period_ns: f32,
    /// Written by the map callback, read by `last_ns`.
    last_ns: Arc<AtomicU64>,
    /// Raised by the map callback to signal that a new sample arrived.
    landed: Arc<AtomicBool>,
    /// Which query pair the next pass writes.
    pair: usize,
    /// A pair written by an earlier frame, waiting to be resolved.
    pending: Option<usize>,
    /// A resolve was encoded and is waiting to be mapped.
    resolved: bool,
    /// A map is in flight; starting a second one on the same buffer is an error.
    mapping: bool,
}

impl GpuTimer {
    /// Build the timing resources, or `None` if the device was not created with
    /// `Features::TIMESTAMP_QUERY`.
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
    /// Never this frame's: a pair resolved in the command buffer that wrote it
    /// reads counters that have not retired. The two pairs alternate, so the
    /// resolve encoded here reads one the GPU has already finished.
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

    /// Start reading back whatever the previous frame resolved. Call at the top
    /// of a frame: at `resolve` time the host has not submitted the encoder yet.
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
                let view = buffer.slice(..).get_mapped_range().expect("the read map just succeeded, and nothing else views it");
                let mut ticks = [0u64; 2];
                bytemuck::cast_slice_mut(&mut ticks).copy_from_slice(&view[..16]);
                drop(view);
                buffer.unmap();
                // A backwards or absent pair means the driver dropped the query.
                if let Some(delta) = ticks[1].checked_sub(ticks[0]) {
                    slot.store((delta as f64 * period as f64) as u64, Ordering::Relaxed);
                }
            }
            landed.store(true, Ordering::Release);
        });
    }

    /// Clear the way for the next measurement once the in-flight map has landed.
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
