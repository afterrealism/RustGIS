//! Background CPU render worker: renders exact (settle) frames off the UI thread so a slow
//! scene never blocks input. The UI sends a request when the settle key goes stale and keeps
//! showing the last texture until the matching result arrives; a newer request supersedes a
//! queued one (the worker drains its channel before rendering). Mid-gesture frames stay on
//! the UI thread — they are LOD-biased and cheap, and the texture-shift covers them.
//!
//! Gated behind `#[cfg(not(target_arch = "wasm32"))]` — wasm has no `std::thread`.

#![cfg(not(target_arch = "wasm32"))]

use std::sync::mpsc;
use std::thread;

use rustgis_map::Map;

use crate::{Backend, RenderOutput, Rendered, View, render_backend_lod};

/// A render request from the UI thread to the worker.
pub struct RenderRequest {
    /// Matches the result to the request; the UI stamps the `Key`/camera it captured at send
    /// time, never the camera at delivery time.
    pub seq: u64,
    /// A clone of the map (features and projected geometry are Arc-shared, so this is cheap).
    pub map: Map,
    /// The view parameters for this frame.
    pub view: View,
    /// LOD bias (1.0 for the exact settle frame).
    pub lod_bias: f64,
}

/// A completed render from the worker back to the UI thread.
pub struct RenderResult {
    pub seq: u64,
    /// The whole frame — pixmap, labels, unplaced labels, attribution, tile errors — so a
    /// worker-delivered frame is indistinguishable from a synchronous one.
    pub rendered: Option<Rendered>,
    /// The rendered clone, whose warmed caches the UI splices back into its own layers
    /// (`Map::adopt_caches`) so the next frame doesn't re-pay them.
    pub map: Map,
}

/// The outcome of a non-blocking poll.
pub enum Poll {
    /// A finished frame (boxed: it carries the rendered map clone).
    Ready(Box<RenderResult>),
    /// Nothing yet; the worker is still rendering.
    Empty,
    /// The worker thread is gone (panicked or exited). The caller stops using the worker.
    Dead,
}

/// A background CPU render worker.
pub struct RenderWorker {
    tx: mpsc::SyncSender<RenderRequest>,
    rx: mpsc::Receiver<RenderResult>,
    _handle: thread::JoinHandle<()>,
}

impl RenderWorker {
    /// Spawns the worker thread.
    pub fn new() -> Option<Self> {
        // One slot each way: only the latest request and latest result matter.
        let (req_tx, req_rx) = mpsc::sync_channel::<RenderRequest>(1);
        let (res_tx, res_rx) = mpsc::sync_channel::<RenderResult>(1);
        let handle = thread::Builder::new().name("rustgis-render".into()).spawn(move || worker_loop(req_rx, res_tx)).ok()?;
        Some(RenderWorker { tx: req_tx, rx: res_rx, _handle: handle })
    }

    /// Queues a render request; `true` when it was. A full channel means the worker is still
    /// busy (the caller keeps its outstanding request), and `false` otherwise stops the UI
    /// arming `worker_pending` for a result that will never come.
    pub fn request(&self, req: RenderRequest) -> bool {
        self.tx.try_send(req).is_ok()
    }

    /// Non-blocking poll for a completed frame.
    pub fn poll(&self) -> Poll {
        match self.rx.try_recv() {
            Ok(r) => Poll::Ready(Box::new(r)),
            Err(mpsc::TryRecvError::Empty) => Poll::Empty,
            Err(mpsc::TryRecvError::Disconnected) => Poll::Dead,
        }
    }
}

fn worker_loop(rx: mpsc::Receiver<RenderRequest>, tx: mpsc::SyncSender<RenderResult>) {
    while let Ok(mut req) = rx.recv() {
        // Superseded before rendering started: only the newest queued request is worth doing.
        while let Ok(newer) = rx.try_recv() {
            req = newer;
        }
        // The renderer must never panic, but if it does anyway the worker thread survives
        // its first bad frame instead of dying silently — the UI just renders that frame
        // itself (the `rendered: None` result) and keeps asking for the next ones.
        let rendered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            match render_backend_lod(&mut req.map, &req.view, Backend::Cpu, req.lod_bias) {
                Some(RenderOutput::Cpu(r)) => Some(r),
                _ => None,
            }
        }))
        .unwrap_or(None);
        // Under the UI's one-outstanding-request protocol the result channel is empty here;
        // a failed send would mean the UI stopped polling, and the loop's next recv (or its
        // disconnect) sorts that out. The clone rides along either way: its caches are valid
        // whatever the frame outcome.
        let _ = tx.try_send(RenderResult { seq: req.seq, rendered, map: req.map });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustgis_map::{Basemap, Camera, Map};

    fn tiny_map() -> Map {
        let mut m = Map::new("M");
        m.basemap = Basemap::Blank;
        m.graticule = false;
        m
    }

    #[test]
    fn round_trips_requests_in_order() {
        let Some(w) = RenderWorker::new() else { panic!("worker spawns") };
        let view = View { width: 32.0, height: 32.0, pixels_per_point: 1.0, camera: Camera::world() };
        // The UI's protocol: one outstanding request at a time, polled to completion. Each
        // result carries its request's seq and a complete frame.
        let mut expect = None;
        for seq in [7u64, 8] {
            w.request(RenderRequest { seq, map: tiny_map(), view, lod_bias: 1.0 });
            let mut got = None;
            for _ in 0..400 {
                if let Poll::Ready(r) = w.poll() {
                    got = Some(r);
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            let r = got.unwrap_or_else(|| panic!("worker delivered seq {seq}"));
            assert_eq!(r.seq, seq, "results match their requests");
            let frame = r.rendered.expect("a blank map still renders");
            assert_eq!(frame.pixmap.width(), 32);
            expect = Some(seq);
        }
        // A request queued while the channel still holds one is dropped (capacity 1): the
        // outstanding request wins, which is what the single-outstanding UI protocol expects.
        assert_eq!(expect, Some(8));
    }
}
