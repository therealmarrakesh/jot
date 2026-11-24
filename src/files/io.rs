use std::io;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::document::{
    self, DocPath, Encoding, Format, LoadError, SaveError, default_directory, file_name,
};
use crate::platform::{
    Jobs, OpenDialogOptions, SaveChoice, SaveDialogOptions, WindowHandle, run_open_dialog,
    run_save_dialog,
};
use crate::types::{Error, SharedStr};

use super::{FileEvent, FileIo, WriteOutcome};

const UNTITLED_FILE_NAME: &str = "Untitled.txt";
const SAVE_AS_TITLE: &str = "Save as";
const TEXT_FILE_TYPE: (&str, &str) = ("Text documents (*.txt)", "*.txt");
const ALL_FILE_TYPE: (&str, &str) = ("All files", "*.*");
const TEXT_EXTENSION: &str = "txt";
const ENCODING_LABEL: &str = "Encoding:";

#[derive(Clone, Default)]
pub(crate) struct WriteTracker(Arc<(Mutex<usize>, Condvar)>);

impl WriteTracker {
    pub(crate) fn enter(&self) -> WriteFlightGuard {
        let mut count = self.0.0.lock().unwrap_or_else(|e| e.into_inner());
        *count += 1;
        WriteFlightGuard(self.0.clone())
    }

    pub(crate) fn in_flight(&self) -> usize {
        *self.0.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn wait(&self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        let mut count = self.0.0.lock().unwrap_or_else(|e| e.into_inner());
        while *count > 0 {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            let (guard, _) = self
                .0
                .1
                .wait_timeout(count, remaining)
                .unwrap_or_else(|e| e.into_inner());
            count = guard;
        }
    }
}

pub(crate) struct WriteFlightGuard(Arc<(Mutex<usize>, Condvar)>);

impl Drop for WriteFlightGuard {
    fn drop(&mut self) {
        let mut count = self.0.0.lock().unwrap_or_else(|e| e.into_inner());
        *count = count.saturating_sub(1);
        if *count == 0 {
            self.0.1.notify_all();
        }
    }
}

pub(crate) fn spawn_file_io<E: Send + 'static>(
    io: FileIo,
    owner: Option<WindowHandle>,
    tracker: &WriteTracker,
    jobs: &Jobs<E>,
    wrap: fn(FileEvent) -> E,
) {
    match io {
        FileIo::OpenDialog { window, current } => {
            let Some(owner) = owner else {
                jobs.post(wrap(FileEvent::OpenDialogDone {
                    window,
                    result: Ok(None),
                }));
                return;
            };
            let options = OpenDialogOptions {
                multiple: true,
                directory: Some(default_directory(current.as_deref())),
                file_types: [TEXT_FILE_TYPE, ALL_FILE_TYPE]
                    .map(|(name, spec)| (SharedStr::new_static(name), SharedStr::new_static(spec)))
                    .into(),
            };
            jobs.spawn(
                "jot-open-dialog",
                move || run_open_dialog(owner, options),
                move |res| {
                    wrap(FileEvent::OpenDialogDone {
                        window,
                        result: res.unwrap_or_else(|msg| Err(Error::new(msg))),
                    })
                },
            );
        }
        FileIo::LoadFiles { window, paths } => {
            let fallback = paths.clone();
            jobs.spawn(
                "jot-load-files",
                move || {
                    paths
                        .into_iter()
                        .map(|doc_path| {
                            let loaded = document::load(doc_path.as_path());
                            (doc_path, loaded)
                        })
                        .collect::<Vec<_>>()
                },
                move |res| {
                    let results = res.unwrap_or_else(|msg| {
                        fallback
                            .into_iter()
                            .map(|doc_path| {
                                (doc_path, Err(LoadError::Io(io::Error::other(msg.clone()))))
                            })
                            .collect()
                    });
                    wrap(FileEvent::FilesLoaded { window, results })
                },
            );
        }
        FileIo::SaveDialog {
            wf,
            current,
            format,
            ..
        } => {
            let Some(owner) = owner else {
                jobs.post(wrap(FileEvent::SaveDialogDone {
                    wf,
                    result: Ok(None),
                }));
                return;
            };
            let suggested =
                file_name(current.as_deref()).unwrap_or_else(|| UNTITLED_FILE_NAME.into());
            let options = SaveDialogOptions {
                title: Some(SAVE_AS_TITLE.into()),
                directory: default_directory(current.as_deref()),
                file_name: Some(suggested),
                file_types: [TEXT_FILE_TYPE, ALL_FILE_TYPE]
                    .map(|(name, spec)| (SharedStr::new_static(name), SharedStr::new_static(spec)))
                    .into(),
                default_extension: Some(TEXT_EXTENSION.into()),
                choice: Some(SaveChoice {
                    label: ENCODING_LABEL.into(),
                    items: Encoding::SAVE_ORDER
                        .map(|encoding| SharedStr::new_static(encoding.label()))
                        .into(),
                    selected: Encoding::SAVE_ORDER
                        .iter()
                        .position(|&encoding| encoding == format.encoding)
                        .unwrap_or_default(),
                }),
            };
            jobs.spawn(
                "jot-save-dialog",
                move || run_save_dialog(owner, options),
                move |res| {
                    wrap(FileEvent::SaveDialogDone {
                        wf,
                        result: res.unwrap_or_else(|msg| Err(Error::new(msg))),
                    })
                },
            );
        }
        FileIo::WriteFile {
            wf,
            job,
            force_lossy,
        } => {
            let guard = tracker.enter();
            jobs.spawn(
                "jot-write-file",
                move || {
                    let _guard = guard;
                    if !force_lossy {
                        match document::is_lossy(&job.text, job.format) {
                            Ok(true) => return WriteOutcome::NeedsLossyConfirm,
                            Ok(false) => {}
                            Err(err) => return WriteOutcome::Error(err.into()),
                        }
                    }
                    match document::write_rope(&job.path, &job.text, job.format) {
                        Ok(()) => WriteOutcome::Saved {
                            doc_path: DocPath::new(job.path),
                            format: Format {
                                mixed_line_endings: false,
                                lossy_decode: false,
                                ..job.format
                            },
                        },
                        Err(err) => WriteOutcome::Error(err),
                    }
                },
                move |res| {
                    let outcome = res.unwrap_or_else(|msg| {
                        WriteOutcome::Error(SaveError::Io(io::Error::other(msg)))
                    });
                    wrap(FileEvent::WriteDone { wf, outcome })
                },
            );
        }
    }
}
