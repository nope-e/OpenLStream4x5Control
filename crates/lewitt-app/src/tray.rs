use std::thread;
use std::time::Duration;

use iced::futures::channel::mpsc;
use iced::futures::{SinkExt, Stream, executor};
use iced::{Subscription, stream};
use thiserror::Error;
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{
    BadIcon, Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
};

use crate::lifecycle::LifecycleAction;
use crate::localization::Catalog;

const SHOW_ID: &str = "lewitt.tray.show";
const RECONNECT_ID: &str = "lewitt.tray.reconnect";
const STATUS_ID: &str = "lewitt.tray.status";
const QUIT_ID: &str = "lewitt.tray.quit";
const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// A platform tray icon and the menu items whose labels can be localized.
pub struct Tray {
    icon: TrayIcon,
    show: MenuItem,
    reconnect: MenuItem,
    status: MenuItem,
    quit: MenuItem,
}

impl Tray {
    /// Creates the platform tray icon and its required menu.
    pub fn new(catalog: &Catalog) -> Result<Self, TrayError> {
        let show = menu_item(SHOW_ID, catalog, "tray.show");
        let reconnect = menu_item(RECONNECT_ID, catalog, "tray.reconnect");
        let status = menu_item(STATUS_ID, catalog, "tray.status");
        let quit = menu_item(QUIT_ID, catalog, "tray.quit");
        let separator = PredefinedMenuItem::separator();
        let menu = Menu::with_items(&[&show, &reconnect, &status, &separator, &quit])?;

        // tray-icon 0.25.1 omits NIF_GUID when updating a Windows tooltip.
        // Identifying the icon by HWND/uID keeps runtime language updates working.
        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .with_tooltip(text(catalog, "app.title"))
            .with_icon(make_icon()?)
            .build()?;

        Ok(Self {
            icon,
            show,
            reconnect,
            status,
            quit,
        })
    }

    /// Updates all user-visible tray strings after a language change.
    pub fn set_language(&self, catalog: &Catalog) -> Result<(), TrayError> {
        self.show.set_text(text(catalog, "tray.show"));
        self.reconnect.set_text(text(catalog, "tray.reconnect"));
        self.status.set_text(text(catalog, "tray.status"));
        self.quit.set_text(text(catalog, "tray.quit"));
        self.icon.set_tooltip(Some(text(catalog, "app.title")))?;
        Ok(())
    }
}

/// Subscribes to tray menu actions without blocking the GUI event thread.
pub fn subscription() -> Subscription<LifecycleAction> {
    Subscription::run(event_stream)
}

fn event_stream() -> impl Stream<Item = LifecycleAction> {
    stream::channel(8, async |output| {
        let click_output = output.clone();
        let _menu_worker = thread::Builder::new()
            .name("lewitt-tray-events".into())
            .spawn(move || forward_menu_events(output));
        let _click_worker = thread::Builder::new()
            .name("lewitt-tray-clicks".into())
            .spawn(move || forward_icon_events(click_output));
    })
}

fn forward_menu_events(mut output: mpsc::Sender<LifecycleAction>) {
    loop {
        match MenuEvent::receiver().recv_timeout(EVENT_POLL_INTERVAL) {
            Ok(event) => {
                if let Some(action) = action_for_menu_id(event.id())
                    && executor::block_on(output.send(action)).is_err()
                {
                    break;
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                if output.is_closed() {
                    break;
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn forward_icon_events(mut output: mpsc::Sender<LifecycleAction>) {
    loop {
        match TrayIconEvent::receiver().recv_timeout(EVENT_POLL_INTERVAL) {
            Ok(event) => {
                if let Some(action) = action_for_icon_event(&event)
                    && executor::block_on(output.send(action)).is_err()
                {
                    break;
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                if output.is_closed() {
                    break;
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn action_for_menu_id(id: &MenuId) -> Option<LifecycleAction> {
    match id.as_ref() {
        SHOW_ID => Some(LifecycleAction::TrayShow),
        RECONNECT_ID => Some(LifecycleAction::TrayReconnect),
        STATUS_ID => Some(LifecycleAction::TrayStatus),
        QUIT_ID => Some(LifecycleAction::TrayQuit),
        _ => None,
    }
}

fn action_for_icon_event(event: &TrayIconEvent) -> Option<LifecycleAction> {
    match event {
        TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } => Some(LifecycleAction::TrayShow),
        _ => None,
    }
}

fn menu_item(id: &'static str, catalog: &Catalog, key: &'static str) -> MenuItem {
    MenuItem::with_id(id, text(catalog, key), true, None)
}

fn text(catalog: &Catalog, key: &'static str) -> &'static str {
    catalog.get(key).unwrap_or(key)
}

fn make_icon() -> Result<Icon, BadIcon> {
    const SIDE: u32 = 32;
    let mut rgba = vec![0_u8; (SIDE * SIDE * 4) as usize];

    for y in 0..SIDE {
        for x in 0..SIDE {
            let offset = ((y * SIDE + x) * 4) as usize;
            let inside = (3..29).contains(&x) && (3..29).contains(&y);
            let dot = matches!(x, 8 | 13 | 18 | 23) && matches!(y, 7 | 12 | 17 | 22 | 27);
            let color = if dot {
                [242, 246, 255, 255]
            } else if inside {
                [37, 70, 104, 255]
            } else {
                [0, 0, 0, 0]
            };
            rgba[offset..offset + 4].copy_from_slice(&color);
        }
    }

    Icon::from_rgba(rgba, SIDE, SIDE)
}

/// Errors that prevent creation or update of the system tray.
#[derive(Debug, Error)]
pub enum TrayError {
    #[error("could not create the tray menu: {0}")]
    Menu(#[from] tray_icon::menu::Error),
    #[error("could not create the tray icon image: {0}")]
    Icon(#[from] BadIcon),
    #[error("could not access the platform tray: {0}")]
    Platform(#[from] tray_icon::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_required_menu_ids_map_to_lifecycle_actions() {
        assert_eq!(
            action_for_menu_id(&MenuId::new(SHOW_ID)),
            Some(LifecycleAction::TrayShow)
        );
        assert_eq!(
            action_for_menu_id(&MenuId::new(RECONNECT_ID)),
            Some(LifecycleAction::TrayReconnect)
        );
        assert_eq!(
            action_for_menu_id(&MenuId::new(STATUS_ID)),
            Some(LifecycleAction::TrayStatus)
        );
        assert_eq!(
            action_for_menu_id(&MenuId::new(QUIT_ID)),
            Some(LifecycleAction::TrayQuit)
        );
        assert_eq!(action_for_menu_id(&MenuId::new("other")), None);
    }

    #[test]
    fn left_button_release_shows_the_window() {
        let left_release = TrayIconEvent::Click {
            id: tray_icon::TrayIconId::new("test"),
            position: tray_icon::dpi::PhysicalPosition::default(),
            rect: tray_icon::Rect::default(),
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
        };
        let left_press = TrayIconEvent::Click {
            id: tray_icon::TrayIconId::new("test"),
            position: tray_icon::dpi::PhysicalPosition::default(),
            rect: tray_icon::Rect::default(),
            button: MouseButton::Left,
            button_state: MouseButtonState::Down,
        };

        assert_eq!(
            action_for_icon_event(&left_release),
            Some(LifecycleAction::TrayShow)
        );
        assert_eq!(action_for_icon_event(&left_press), None);
    }
}
