// SPDX-License-Identifier: GPL-3.0-or-later
//! Asynchronous frame readback (arch §10 step 1): rendered RGBA8 frames are copied into a small
//! pool of mappable staging buffers and mapped without waiting, so the render thread never
//! stalls on the GPU. The caller collects finished frames later, oldest first, and copies the
//! pixels straight out of the mapped buffer (into the SHM ring, an encoder, or an [`Image`]
//! whose allocation is reused).
//!
//! * The pool holds at most [`CompositorOptions::readback_slots`](super::CompositorOptions)
//!   buffers (3 by default: one being written, one in flight, one being read). Buffers are
//!   created only when the pool grows or the output size changes (a quality-ladder step), never
//!   per frame; when every buffer is in flight, a submission fails with
//!   [`GpuError::ReadbackBusy`] instead of waiting (the transport drops that frame).
//! * Completion is tracked by an atomic per buffer, set by wgpu's map callback during a
//!   non-blocking device poll.
//! * A lost device drops the pool with whatever was in flight; those frames are never
//!   delivered (the transport renders the next one).

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use cutweave_model::ItemId;

use super::{FrameStats, GpuError};
use crate::frame::FrameError;
use crate::hash::CacheKey;
use crate::image::Image;

const IDLE: u8 = 0;
const PENDING: u8 = 1;
const MAPPED: u8 = 2;
const FAILED: u8 = 3;

/// Identifies a frame submitted with [`Compositor::submit`](super::Compositor::submit).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FrameTicket(pub u64);

/// A frame whose readback finished.
#[derive(Clone, Debug, PartialEq)]
pub struct ReadbackFrame {
    /// The submission it answers.
    pub ticket: FrameTicket,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// The frame's subgraph key.
    pub key: CacheKey,
    /// Layers skipped because their frame was unavailable.
    pub missing: Vec<(ItemId, FrameError)>,
    /// Execution statistics.
    pub stats: FrameStats,
}

impl ReadbackFrame {
    /// Copies `pixels` into `out`, reusing its allocation.
    pub fn copy_into(&self, pixels: &[u8], out: &mut Image) {
        out.width = self.width;
        out.height = self.height;
        out.data.clear();
        out.data.extend_from_slice(pixels);
    }
}

struct Slot {
    buffer: wgpu::Buffer,
    size: u64,
    state: Arc<AtomicU8>,
    busy: bool,
}

struct InFlight {
    slot: usize,
    bytes: u64,
    frame: ReadbackFrame,
    discarded: bool,
}

/// The staging buffers and the frames in flight, oldest first.
pub(crate) struct ReadbackPool {
    slots: Vec<Slot>,
    capacity: usize,
    in_flight: VecDeque<InFlight>,
    next_ticket: u64,
    /// Buffers created (growth and resizes), for tests.
    pub(crate) buffers_created: u64,
}

impl ReadbackPool {
    pub(crate) fn new(capacity: usize) -> ReadbackPool {
        let capacity = capacity.max(1);
        ReadbackPool {
            slots: Vec::with_capacity(capacity),
            capacity,
            in_flight: VecDeque::with_capacity(capacity),
            next_ticket: 1,
            buffers_created: 0,
        }
    }

    pub(crate) fn in_flight(&self) -> usize {
        self.in_flight.len()
    }

    fn create(device: &wgpu::Device, size: u64) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback staging"),
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// A free staging buffer of `size` bytes: one of that size if free, else a free one
    /// recreated at that size, else a new one while below capacity.
    pub(crate) fn acquire(
        &mut self,
        device: &wgpu::Device,
        size: u64,
    ) -> Result<(usize, wgpu::Buffer), GpuError> {
        let index = if let Some(i) = self.slots.iter().position(|s| !s.busy && s.size == size) {
            i
        } else if let Some(i) = self.slots.iter().position(|s| !s.busy) {
            self.slots[i].buffer.destroy();
            self.slots[i].buffer = Self::create(device, size);
            self.slots[i].size = size;
            self.buffers_created += 1;
            i
        } else if self.slots.len() < self.capacity {
            self.slots.push(Slot {
                buffer: Self::create(device, size),
                size,
                state: Arc::new(AtomicU8::new(IDLE)),
                busy: false,
            });
            self.buffers_created += 1;
            self.slots.len() - 1
        } else {
            return Err(GpuError::ReadbackBusy(self.capacity));
        };
        let slot = &mut self.slots[index];
        slot.busy = true;
        slot.state.store(IDLE, Ordering::Release);
        Ok((index, slot.buffer.clone()))
    }

    /// Returns a slot that will not be submitted (the frame failed before submission).
    pub(crate) fn release(&mut self, index: usize) {
        if let Some(slot) = self.slots.get_mut(index) {
            slot.busy = false;
            slot.state.store(IDLE, Ordering::Release);
        }
    }

    /// Starts mapping a submitted slot and queues the frame.
    pub(crate) fn start(
        &mut self,
        index: usize,
        bytes: u64,
        mut frame: ReadbackFrame,
    ) -> FrameTicket {
        let ticket = FrameTicket(self.next_ticket);
        self.next_ticket += 1;
        frame.ticket = ticket;
        let slot = &self.slots[index];
        slot.state.store(PENDING, Ordering::Release);
        let state = Arc::clone(&slot.state);
        slot.buffer
            .slice(..bytes)
            .map_async(wgpu::MapMode::Read, move |result| {
                state.store(
                    if result.is_ok() { MAPPED } else { FAILED },
                    Ordering::Release,
                );
            });
        self.in_flight.push_back(InFlight {
            slot: index,
            bytes,
            frame,
            discarded: false,
        });
        ticket
    }

    /// Marks every frame in flight as unwanted (a seek: latest wins); their buffers are
    /// recycled as they complete.
    pub(crate) fn discard_all(&mut self) {
        for f in &mut self.in_flight {
            f.discarded = true;
        }
    }

    fn finish_slot(&mut self, index: usize, mapped: bool) {
        let slot = &mut self.slots[index];
        if mapped {
            slot.buffer.unmap();
        }
        slot.busy = false;
        slot.state.store(IDLE, Ordering::Release);
    }

    /// Delivers the oldest frame if its readback finished: `f` (taken only then) gets its
    /// description and the mapped pixels (tightly packed RGBA8 rows). Discarded frames that
    /// finished are recycled on the way. `Ok(None)` when nothing is ready.
    pub(crate) fn take_ready<R, F: FnOnce(&ReadbackFrame, &[u8]) -> R>(
        &mut self,
        f: &mut Option<F>,
    ) -> Result<Option<R>, GpuError> {
        if f.is_none() {
            return Ok(None);
        }
        loop {
            let Some(front) = self.in_flight.front() else {
                return Ok(None);
            };
            let state = self.slots[front.slot].state.load(Ordering::Acquire);
            match state {
                MAPPED | FAILED => {}
                _ => return Ok(None),
            }
            let Some(done) = self.in_flight.pop_front() else {
                return Ok(None);
            };
            if state == FAILED {
                self.finish_slot(done.slot, false);
                if done.discarded {
                    continue;
                }
                return Err(GpuError::Readback(format!(
                    "mapping frame {} failed",
                    done.frame.ticket.0
                )));
            }
            let deliver = if done.discarded { None } else { f.take() };
            let Some(deliver) = deliver else {
                self.finish_slot(done.slot, true);
                continue;
            };
            let result = {
                let buffer = &self.slots[done.slot].buffer;
                let range = buffer
                    .get_mapped_range(..done.bytes)
                    .map_err(|e| GpuError::Readback(format!("{e:?}")));
                match range {
                    Ok(view) => Ok(deliver(&done.frame, &view)),
                    Err(e) => Err(e),
                }
            };
            self.finish_slot(done.slot, true);
            return result.map(Some);
        }
    }
}
