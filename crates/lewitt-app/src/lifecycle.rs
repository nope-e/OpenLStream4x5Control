/// User- or OS-originated lifecycle actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleAction {
    WindowCloseRequested,
    TrayShow,
    TrayReconnect,
    TrayStatus,
    TrayQuit,
    SecondInstanceShow,
}

/// Side effects for the platform integration layer to execute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleEffect {
    HideWindow,
    ShowWindow,
    Reconnect,
    ShowStatus,
    Shutdown,
}

/// Pure lifecycle state machine shared by Windows and Linux frontends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lifecycle {
    window_visible: bool,
    quitting: bool,
}

impl Lifecycle {
    #[must_use]
    pub const fn new(background: bool) -> Self {
        Self {
            window_visible: !background,
            quitting: false,
        }
    }

    #[must_use]
    pub const fn window_visible(self) -> bool {
        self.window_visible
    }

    #[must_use]
    pub const fn is_quitting(self) -> bool {
        self.quitting
    }

    /// Apply one action and return the required platform side effect.
    pub fn apply(&mut self, action: LifecycleAction) -> Option<LifecycleEffect> {
        if self.quitting {
            return None;
        }
        match action {
            LifecycleAction::WindowCloseRequested => {
                self.window_visible = false;
                Some(LifecycleEffect::HideWindow)
            }
            LifecycleAction::TrayShow | LifecycleAction::SecondInstanceShow => {
                self.window_visible = true;
                Some(LifecycleEffect::ShowWindow)
            }
            LifecycleAction::TrayReconnect => Some(LifecycleEffect::Reconnect),
            LifecycleAction::TrayStatus => Some(LifecycleEffect::ShowStatus),
            LifecycleAction::TrayQuit => {
                self.quitting = true;
                Some(LifecycleEffect::Shutdown)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_hides_but_tray_quit_stops_runtime() {
        let mut lifecycle = Lifecycle::new(false);
        assert_eq!(
            lifecycle.apply(LifecycleAction::WindowCloseRequested),
            Some(LifecycleEffect::HideWindow)
        );
        assert!(!lifecycle.window_visible());
        assert!(!lifecycle.is_quitting());

        assert_eq!(
            lifecycle.apply(LifecycleAction::TrayQuit),
            Some(LifecycleEffect::Shutdown)
        );
        assert!(lifecycle.is_quitting());
        assert_eq!(lifecycle.apply(LifecycleAction::TrayShow), None);
    }

    #[test]
    fn background_start_is_hidden_and_can_be_shown() {
        let mut lifecycle = Lifecycle::new(true);
        assert!(!lifecycle.window_visible());
        assert_eq!(
            lifecycle.apply(LifecycleAction::TrayShow),
            Some(LifecycleEffect::ShowWindow)
        );
        assert!(lifecycle.window_visible());
    }

    #[test]
    fn second_instance_signal_shows_background_window() {
        let mut lifecycle = Lifecycle::new(true);
        assert_eq!(
            lifecycle.apply(LifecycleAction::SecondInstanceShow),
            Some(LifecycleEffect::ShowWindow)
        );
        assert!(lifecycle.window_visible());
    }
}
