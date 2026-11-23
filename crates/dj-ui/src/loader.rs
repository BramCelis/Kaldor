//! Background track loading: decode (progressively) and then build the
//! waveform and a deck's fine detail waveform, all off the UI and audio
//! threads.

use dj_analysis::Waveform;
use dj_decode::{LoadEvent, SourceInfo, TrackAudio};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

/// Events from a load job.
#[derive(Debug)]
pub enum LoaderEvent {
    Started {
        info: SourceInfo,
        audio: Arc<TrackAudio>,
    },
    Finished {
        frames: u64,
        truncated: bool,
    },
    Waveform(Arc<Waveform>),
    /// The fine waveform for a deck's scrolling view (the last event of a
    /// load that asked for it).
    Detail(Arc<Waveform>),
    Failed(String),
}

/// A running load job. Dropping it cancels the decode.
pub struct LoadJob {
    pub path: PathBuf,
    rx: mpsc::Receiver<LoaderEvent>,
    cancel: Arc<AtomicBool>,
}

impl LoadJob {
    /// Starts decoding `path`. With `build_waveform` false (the library
    /// already has one) no waveform is built; with `build_detail` the fine
    /// waveform for a deck's scrolling view is built after it.
    pub fn start(
        path: PathBuf,
        sample_rate: u32,
        build_waveform: bool,
        build_detail: bool,
    ) -> std::io::Result<Self> {
        let cancel = Arc::new(AtomicBool::new(false));
        let inner = dj_decode::spawn_load(path.clone(), sample_rate, Arc::clone(&cancel))?;
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("kaldor-load".into())
            .spawn(move || {
                let mut audio: Option<Arc<TrackAudio>> = None;
                for ev in inner {
                    match ev {
                        LoadEvent::Started { info, audio: a } => {
                            audio = Some(Arc::clone(&a));
                            if tx.send(LoaderEvent::Started { info, audio: a }).is_err() {
                                return;
                            }
                        }
                        LoadEvent::Finished { frames, truncated } => {
                            let _ = tx.send(LoaderEvent::Finished { frames, truncated });
                            if let Some(a) = audio.take() {
                                if build_waveform {
                                    let wf = Waveform::build(&a);
                                    let _ = tx.send(LoaderEvent::Waveform(Arc::new(wf)));
                                }
                                if build_detail {
                                    let wf = Waveform::build_detail(&a);
                                    let _ = tx.send(LoaderEvent::Detail(Arc::new(wf)));
                                }
                            }
                        }
                        LoadEvent::Failed(e) => {
                            let _ = tx.send(LoaderEvent::Failed(e));
                        }
                    }
                }
            })?;
        Ok(Self { path, rx, cancel })
    }

    /// Non-blocking: the next event, if any.
    pub fn poll(&self) -> Option<LoaderEvent> {
        self.rx.try_recv().ok()
    }
}

impl Drop for LoadJob {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
