use gpui::{Context, Task};
use std::time::Duration;

fn blink_interval() -> Option<Duration> {
    let millis = unsafe { windows::Win32::UI::WindowsAndMessaging::GetCaretBlinkTime() };
    (millis != 0 && millis != u32::MAX).then(|| Duration::from_millis(millis.into()))
}

/// Manages cursor blinking according to Windows' caret blink interval.
pub(crate) struct BlinkCursor {
    visible: bool,
    paused: bool,
    epoch: usize,
    _task: Task<()>,
}

impl BlinkCursor {
    pub(crate) fn new() -> Self {
        Self {
            visible: false,
            paused: false,
            epoch: 0,
            _task: Task::ready(()),
        }
    }

    pub(crate) fn start(&mut self, cx: &mut Context<Self>) {
        self.visible = false;
        self.blink(self.epoch, cx);
    }

    pub(crate) fn stop(&mut self, cx: &mut Context<Self>) {
        self.epoch = 0;
        self.paused = false;
        self.visible = false;
        self._task = Task::ready(());
        cx.notify();
    }

    fn next_epoch(&mut self) -> usize {
        self.epoch += 1;
        self.epoch
    }

    fn blink(&mut self, epoch: usize, cx: &mut Context<Self>) {
        if epoch != self.epoch {
            return;
        }
        if self.paused {
            self.visible = true;
            return;
        }
        let Some(interval) = blink_interval() else {
            self.visible = true;
            self._task = Task::ready(());
            cx.notify();
            return;
        };

        self.visible = !self.visible;
        cx.notify();

        let epoch = self.next_epoch();
        self._task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(interval).await;
            if let Some(this) = this.upgrade() {
                this.update(cx, |this, cx| this.blink(epoch, cx));
            }
        });
    }

    pub(crate) fn visible(&self) -> bool {
        self.paused || self.visible
    }

    pub(crate) fn pause(&mut self, cx: &mut Context<Self>) {
        if self.epoch == 0 {
            return;
        }

        self.paused = true;
        self.visible = true;
        cx.notify();

        let epoch = self.next_epoch();
        let Some(delay) = blink_interval() else {
            self._task = Task::ready(());
            return;
        };
        self._task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            if let Some(this) = this.upgrade() {
                this.update(cx, |this, cx| {
                    if epoch != this.epoch {
                        return;
                    }
                    this.paused = false;
                    this.blink(epoch, cx);
                });
            }
        });
    }
}
