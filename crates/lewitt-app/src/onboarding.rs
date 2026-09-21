/// Deferred decision produced only after the user confirms onboarding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutostartDecision {
    EnableWithBackgroundFlag,
    KeepDisabled,
}

/// First-run autostart choice. A preselected checkbox is not consent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutostartConsent {
    selected: bool,
    confirmed: bool,
}

impl Default for AutostartConsent {
    fn default() -> Self {
        Self {
            selected: true,
            confirmed: false,
        }
    }
}

impl AutostartConsent {
    #[must_use]
    pub const fn selected(self) -> bool {
        self.selected
    }

    #[must_use]
    pub const fn is_confirmed(self) -> bool {
        self.confirmed
    }

    pub const fn set_selected(&mut self, selected: bool) {
        if !self.confirmed {
            self.selected = selected;
        }
    }

    /// Confirm once and produce the only decision that may mutate OS startup
    /// configuration. Repeated confirmation is ignored.
    pub const fn confirm(&mut self) -> Option<AutostartDecision> {
        if self.confirmed {
            return None;
        }
        self.confirmed = true;
        if self.selected {
            Some(AutostartDecision::EnableWithBackgroundFlag)
        } else {
            Some(AutostartDecision::KeepDisabled)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preselection_does_not_imply_consent() {
        let consent = AutostartConsent::default();
        assert!(consent.selected());
        assert!(!consent.is_confirmed());
    }

    #[test]
    fn explicit_confirmation_is_required_exactly_once() {
        let mut consent = AutostartConsent::default();
        assert_eq!(
            consent.confirm(),
            Some(AutostartDecision::EnableWithBackgroundFlag)
        );
        assert_eq!(consent.confirm(), None);
    }
}
