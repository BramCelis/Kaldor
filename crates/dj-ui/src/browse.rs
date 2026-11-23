//! Browse buttons: the system's own open, save and folder dialogs. The
//! dialog runs on its own thread, so the decks keep drawing while it is
//! open (on macOS on the UI thread, which AppKit requires). Dialogs are off
//! until the app turns them on ([`enable`]): tests and headless runs never
//! open a window.

use crate::settings_ui::button as settings_button;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

static ENABLED: AtomicBool = AtomicBool::new(false);

// Per UI thread: the app draws on one thread, and tests running side by
// side each have their own.
thread_local! {
    static STATE: RefCell<State> = const {
        RefCell::new(State {
            pending: None,
            done: None,
        })
    };
}

#[derive(Default)]
struct State {
    /// The dialog that is open: which button asked, and its answer.
    pending: Option<(&'static str, mpsc::Receiver<Option<PathBuf>>)>,
    /// A path picked and not taken yet by its button.
    done: Option<(&'static str, PathBuf)>,
}

/// What a Browse button picks.
#[derive(Clone, Debug)]
pub enum Pick {
    /// An audio file the app can play.
    AudioFile,
    /// A file of one kind ("Controller profile", ["toml"]).
    File(&'static str, &'static [&'static str]),
    Folder,
    /// A file to write: kind, extension, suggested name.
    Save(&'static str, &'static str, String),
    /// A file to write in one of several formats: (kind, extension) each,
    /// suggested name.
    SaveAs(&'static [(&'static str, &'static str)], String),
}

/// Lets Browse buttons open dialogs (the app calls this at start).
pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
}

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with(|s| f(&mut s.borrow_mut()))
}

/// The path picked for button `id`, once.
pub fn take(id: &'static str) -> Option<PathBuf> {
    with(|s| {
        if let Some((pid, rx)) = &s.pending {
            let pid = *pid;
            match rx.try_recv() {
                Ok(Some(p)) => {
                    s.pending = None;
                    s.done = Some((pid, p));
                }
                Ok(None) | Err(mpsc::TryRecvError::Disconnected) => s.pending = None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if s.done.as_ref().is_some_and(|(d, _)| *d == id) {
            return s.done.take().map(|(_, p)| p);
        }
        None
    })
}

/// A dialog is open.
pub fn busy() -> bool {
    with(|s| s.pending.is_some())
}

/// Acts as if the dialog of button `id` returned `path` (tests).
pub fn set_for_test(id: &'static str, path: PathBuf) {
    with(|s| s.done = Some((id, path)));
}

/// Where the dialog starts: the folder `start` names, or the folder of the
/// file it names.
fn start_dir(start: &str) -> Option<PathBuf> {
    let p = crate::relink::clean_path(start);
    if p.as_os_str().is_empty() {
        return None;
    }
    if p.is_dir() {
        return Some(p);
    }
    p.parent().filter(|d| d.is_dir()).map(Path::to_path_buf)
}

/// Opens the dialog for button `id` (nothing when dialogs are off or one
/// is open already).
pub fn open(id: &'static str, title: &str, pick: Pick, start: &str) {
    if !ENABLED.load(Ordering::Relaxed) {
        tracing::info!("file dialogs are off (headless run): {id}");
        return;
    }
    if busy() {
        return;
    }
    let dir = start_dir(start);
    let title = title.to_string();
    let job = move || -> Option<PathBuf> {
        let mut d = rfd::FileDialog::new().set_title(title);
        if let Some(dir) = dir {
            d = d.set_directory(dir);
        }
        match pick {
            Pick::AudioFile => d
                .add_filter("Audio", dj_library::AUDIO_EXTENSIONS)
                .pick_file(),
            Pick::File(what, ext) => d.add_filter(what, ext).pick_file(),
            Pick::Folder => d.pick_folder(),
            Pick::Save(what, ext, name) => {
                d.add_filter(what, &[ext]).set_file_name(name).save_file()
            }
            Pick::SaveAs(kinds, name) => {
                for (what, ext) in kinds {
                    d = d.add_filter(*what, &[*ext]);
                }
                d.set_file_name(name).save_file()
            }
        }
    };
    let (tx, rx) = mpsc::channel();
    if cfg!(target_os = "macos") {
        let _ = tx.send(job());
    } else if let Err(e) = std::thread::Builder::new()
        .name("kaldor-file-dialog".into())
        .spawn(move || {
            let _ = tx.send(job());
        })
    {
        tracing::warn!("cannot open the file dialog: {e}");
        return;
    }
    with(|s| s.pending = Some((id, rx)));
}

/// A "Browse…" button that opens the dialog. Returns the button and, once,
/// the path picked with it.
pub fn button(
    ui: &mut egui::Ui,
    id: &'static str,
    title: &str,
    pick: Pick,
    start: &str,
) -> (egui::Response, Option<PathBuf>) {
    let busy = busy();
    let r = ui
        .add_enabled_ui(!busy, |ui| settings_button(ui, "Browse…", false, None))
        .inner
        .on_hover_text(title.to_string());
    if r.clicked() {
        open(id, title, pick, start);
    }
    (r, take(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_result_goes_to_its_own_button_once() {
        set_for_test("browse_test_a", PathBuf::from("x.wav"));
        assert_eq!(take("browse_test_b"), None);
        assert_eq!(take("browse_test_a"), Some(PathBuf::from("x.wav")));
        assert_eq!(take("browse_test_a"), None);
    }

    #[test]
    fn dialogs_start_in_the_folder_of_the_path() {
        let dir = crate::test_dir("kaldor-browse");
        assert_eq!(start_dir(&dir.display().to_string()), Some(dir.clone()));
        let file = dir.join("no such file here.mp3");
        assert_eq!(start_dir(&file.display().to_string()), Some(dir));
        assert_eq!(start_dir(""), None);
    }
}
