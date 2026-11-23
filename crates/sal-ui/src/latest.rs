//! One worker thread doing the latest of what was asked of it: at most one
//! piece of work runs and at most one waits; asking again replaces the one
//! waiting and gives up the one running (it is told to stop, and its result
//! is never taken). For a search run as text is typed: however fast the
//! typing, one thread, never a pile of them.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Condvar, Mutex};

/// What the worker shares with its owner.
struct State<Q> {
    /// The work waiting, and its number.
    waiting: Option<(u64, Q)>,
    /// Tells the work running to stop.
    stop: Arc<AtomicBool>,
    /// The owner is gone: the thread ends.
    quit: bool,
}

/// A worker doing `Q`s into `R`s, the latest asked only.
pub struct Latest<Q: Send + 'static, R: Send + 'static> {
    shared: Arc<(Mutex<State<Q>>, Condvar)>,
    results: Receiver<(u64, R)>,
    /// The number of the last work asked for.
    asked: u64,
}

impl<Q: Send + 'static, R: Send + 'static> Latest<Q, R> {
    /// Start the worker: `work` does a `Q` (and returns `None` once its
    /// flag says stop); `done` is called after a result is ready (to draw
    /// the window again).
    pub fn new(name: &str, work: impl Fn(Q, &AtomicBool) -> Option<R> + Send + 'static, done: impl Fn() + Send + 'static) -> Self {
        let shared = Arc::new((Mutex::new(State { waiting: None, stop: Arc::new(AtomicBool::new(false)), quit: false }), Condvar::new()));
        let (tx, results) = channel();
        let theirs = shared.clone();
        let _ = std::thread::Builder::new().name(name.into()).spawn(move || loop {
            let (number, q, stop) = {
                let (lock, cv) = &*theirs;
                let mut s = lock.lock().unwrap_or_else(|e| e.into_inner());
                while s.waiting.is_none() && !s.quit {
                    s = cv.wait(s).unwrap_or_else(|e| e.into_inner());
                }
                if s.quit {
                    return;
                }
                let (number, q) = s.waiting.take().expect("waiting");
                s.stop = Arc::new(AtomicBool::new(false));
                (number, q, s.stop.clone())
            };
            if let Some(r) = work(q, &stop) {
                if tx.send((number, r)).is_err() {
                    return;
                }
                done();
            }
        });
        Latest { shared, results, asked: 0 }
    }

    /// Ask for `q`: it replaces what waits, and what runs is told to stop.
    /// Its number, as [`Latest::take`] gives it back.
    pub fn ask(&mut self, q: Q) -> u64 {
        self.asked += 1;
        let (lock, cv) = &*self.shared;
        let mut s = lock.lock().unwrap_or_else(|e| e.into_inner());
        s.waiting = Some((self.asked, q));
        s.stop.store(true, Ordering::Relaxed);
        cv.notify_one();
        self.asked
    }

    /// Give up what waits and what runs.
    pub fn cancel(&mut self) {
        self.asked += 1;
        let (lock, _) = &*self.shared;
        let mut s = lock.lock().unwrap_or_else(|e| e.into_inner());
        s.waiting = None;
        s.stop.store(true, Ordering::Relaxed);
    }

    /// The result of the last work asked for, once it is done; results of
    /// work asked for before are thrown away.
    pub fn take(&self) -> Option<R> {
        let mut last = None;
        while let Ok((number, r)) = self.results.try_recv() {
            if number == self.asked {
                last = Some(r);
            }
        }
        last
    }
}

impl<Q: Send + 'static, R: Send + 'static> Drop for Latest<Q, R> {
    fn drop(&mut self) {
        let (lock, cv) = &*self.shared;
        let mut s = lock.lock().unwrap_or_else(|e| e.into_inner());
        s.quit = true;
        s.waiting = None;
        s.stop.store(true, Ordering::Relaxed);
        cv.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::time::{Duration, Instant};

    fn wait_for<R: Send + 'static, Q: Send + 'static>(w: &Latest<Q, R>) -> R {
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(r) = w.take() {
                return r;
            }
            assert!(Instant::now() < until, "no result");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Many asks while one runs: one thread, the running one stopped, only
    /// the last one's result taken.
    #[test]
    fn only_the_latest_is_done() {
        let running = Arc::new(AtomicUsize::new(0));
        let most = Arc::new(AtomicUsize::new(0));
        let (r, m) = (running.clone(), most.clone());
        let mut w = Latest::new(
            "test",
            move |n: u32, stop: &AtomicBool| {
                let now = r.fetch_add(1, Ordering::SeqCst) + 1;
                m.fetch_max(now, Ordering::SeqCst);
                // slow work that looks at its flag now and then
                let mut out = None;
                for _ in 0..50 {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(2));
                    out = Some(n * 10);
                }
                r.fetch_sub(1, Ordering::SeqCst);
                (!stop.load(Ordering::Relaxed)).then_some(out).flatten()
            },
            || {},
        );
        for n in 1..=200 {
            w.ask(n);
        }
        assert_eq!(wait_for(&w), 2000, "the last one");
        assert_eq!(most.load(Ordering::SeqCst), 1, "never two at once");
        // after a cancel, nothing more comes
        w.ask(7);
        w.cancel();
        std::thread::sleep(Duration::from_millis(200));
        assert!(w.take().is_none());
        w.ask(8);
        assert_eq!(wait_for(&w), 80);
    }
}
