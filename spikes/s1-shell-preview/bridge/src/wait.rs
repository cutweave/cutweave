//! Wake-up primitive for transport (b) (WP-0C.06): `RingReader.waitForFrame()`.
//!
//! The utility process pushes every published frame to the renderer as soon as it lands. Node
//! has no way to block on shared memory, and on Windows a JS `setTimeout(1)` loop wakes about
//! every 15.6 ms (the default timer resolution is per process since Windows 10 2004), which
//! would add up to a frame of latency and jitter. Instead a libuv worker thread polls the
//! ring's write index with a sub-millisecond sleep (Rust's `thread::sleep` uses a
//! high-resolution waitable timer on Windows) and resolves a promise on the JS thread.
//!
//! The product version would use an OS event/futex signalled by the engine (WP-1H.02).

use std::sync::Arc;
use std::time::{Duration, Instant};

use napi::{Env, Result, Task};
use s1_ring::{RingReader as Reader, SharedMemory};

/// Polls `write_index` until it exceeds `after` or the timeout expires.
pub struct WaitTask {
    reader: Reader<Arc<SharedMemory>>,
    after: u64,
    timeout: Duration,
    poll: Duration,
}

impl WaitTask {
    pub fn new(
        reader: Reader<Arc<SharedMemory>>,
        after: u64,
        timeout: Duration,
        poll: Duration,
    ) -> Self {
        Self {
            reader,
            after,
            timeout,
            poll,
        }
    }
}

impl Task for WaitTask {
    type Output = u64;
    type JsValue = f64;

    fn compute(&mut self) -> Result<u64> {
        let deadline = Instant::now() + self.timeout;
        loop {
            let wi = self.reader.write_index();
            if wi > self.after || Instant::now() >= deadline {
                return Ok(wi);
            }
            std::thread::sleep(self.poll);
        }
    }

    #[allow(clippy::cast_precision_loss)]
    fn resolve(&mut self, _env: Env, wi: u64) -> Result<f64> {
        Ok(wi as f64)
    }
}
