//! Native presentation of the application actions shared with the WebView.
//! Each window publishes its own availability; menu events target that window.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use squallz_core::lock_unpoisoned;
use squallz_i18n::Localizer;
use tauri::menu::{AboutMetadata, Menu, MenuItem, MenuItemKind, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, EventTarget, Manager, State, WebviewWindow};

use crate::{dto::ErrorDto, settings::SettingsStore};

const ACTION_EVENT: &str = "app://menu-action";
const ERROR_EVENT: &str = "app://menu-error";
const UPDATED_EVENT: &str = "app://menu-updated";
const ACTION_PREFIX: &str = "squallz.action.";
const DEFINITIONS: &str = include_str!("../../../frontend/src/lib/app-actions.json");

#[derive(Debug, Deserialize)]
struct ActionDefinition {
    id: String,
    group: String,
    label_key: String,
    accelerator: Option<String>,
    macos_accelerator: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub(crate) struct MenuSnapshot {
    language: String,
    enabled: HashSet<String>,
}

#[derive(Default)]
struct WindowMenus {
    windows: HashMap<String, MenuSnapshot>,
    focused: Option<String>,
    applied: Option<MenuSnapshot>,
}

impl WindowMenus {
    fn current(&self, default_language: &str) -> MenuSnapshot {
        self.focused
            .as_ref()
            .and_then(|label| self.windows.get(label))
            .cloned()
            .unwrap_or_else(|| MenuSnapshot {
                language: self
                    .applied
                    .as_ref()
                    .map_or(default_language, |snapshot| snapshot.language.as_str())
                    .to_owned(),
                enabled: HashSet::new(),
            })
    }

    fn action_target(&self, action: &str) -> Option<String> {
        let label = self.focused.as_ref()?;
        self.windows
            .get(label)?
            .enabled
            .contains(action)
            .then(|| label.clone())
    }

    fn release(&mut self, label: &str) {
        self.windows.remove(label);
        if self.focused.as_deref() == Some(label) {
            self.focused = None;
        }
    }
}

pub(crate) struct NativeMenus {
    definitions: Vec<ActionDefinition>,
    default_language: String,
    windows: Mutex<WindowMenus>,
}

pub(crate) fn install(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let definitions = serde_json::from_str(DEFINITIONS)?;
    let settings = app.state::<Arc<SettingsStore>>();
    let language = Localizer::load(settings.get().language.as_deref())
        .language()
        .to_owned();
    let menus = NativeMenus {
        definitions,
        default_language: language,
        windows: Mutex::new(WindowMenus::default()),
    };
    menus.refresh(app)?;
    app.manage(menus);
    Ok(())
}

impl NativeMenus {
    fn refresh(&self, app: &AppHandle) -> tauri::Result<()> {
        let (snapshot, previous) = {
            let windows = lock_unpoisoned(&self.windows);
            (
                windows.current(&self.default_language),
                windows.applied.clone(),
            )
        };
        if previous.as_ref() == Some(&snapshot) {
            return Ok(());
        }
        if previous.as_ref().map(|state| &state.language) != Some(&snapshot.language) {
            app.set_menu(build_menu(app, &self.definitions, &snapshot)?)?;
        } else if let Some(menu) = app.menu() {
            for top_level in menu.items()? {
                if let MenuItemKind::Submenu(submenu) = top_level {
                    for item in submenu.items()? {
                        if let MenuItemKind::MenuItem(item) = item {
                            if let Some(action) = item.id().as_ref().strip_prefix(ACTION_PREFIX) {
                                item.set_enabled(snapshot.enabled.contains(action))?;
                            }
                        }
                    }
                }
            }
        }
        lock_unpoisoned(&self.windows).applied = Some(snapshot);
        Ok(())
    }
}

fn schedule_refresh(app: &AppHandle) -> tauri::Result<()> {
    let handle = app.clone();
    app.run_on_main_thread(move || {
        let Some(menus) = handle.try_state::<NativeMenus>() else {
            return;
        };
        if let Err(error) = menus.refresh(&handle) {
            log::warn!("native menu update failed: {error}");
            let _ = handle.emit(ERROR_EVENT, ());
        } else {
            let _ = handle.emit(UPDATED_EVENT, ());
        }
    })
}

fn menu_error() -> ErrorDto {
    ErrorDto {
        key: "gui.native_menu.unavailable".to_owned(),
        params: HashMap::new(),
        detail: String::new(),
    }
}

#[tauri::command]
pub(crate) fn update_native_menu(
    app: AppHandle,
    window: WebviewWindow,
    menus: State<'_, NativeMenus>,
    snapshot: MenuSnapshot,
) -> Result<(), ErrorDto> {
    if snapshot
        .enabled
        .iter()
        .any(|id| !menus.definitions.iter().any(|action| &action.id == id))
    {
        return Err(menu_error());
    }
    let focused = window.is_focused().map_err(|_| menu_error())?;
    {
        let mut windows = lock_unpoisoned(&menus.windows);
        windows.windows.insert(window.label().to_owned(), snapshot);
        if focused {
            windows.focused = Some(window.label().to_owned());
        }
    }
    schedule_refresh(&app).map_err(|_| menu_error())
}

pub(crate) fn focus_changed(app: &AppHandle, label: &str, focused: bool) {
    let Some(menus) = app.try_state::<NativeMenus>() else {
        return;
    };
    {
        let mut windows = lock_unpoisoned(&menus.windows);
        if focused {
            windows.focused = Some(label.to_owned());
        } else if windows.focused.as_deref() == Some(label) {
            windows.focused = None;
        }
    }
    if schedule_refresh(app).is_err() {
        let _ = app.emit(ERROR_EVENT, ());
    }
}

pub(crate) fn release_window(app: &AppHandle, label: &str) {
    if let Some(menus) = app.try_state::<NativeMenus>() {
        lock_unpoisoned(&menus.windows).release(label);
        if schedule_refresh(app).is_err() {
            let _ = app.emit(ERROR_EVENT, ());
        }
    }
}

pub(crate) fn handle_event(app: &AppHandle, event: tauri::menu::MenuEvent) {
    let Some(action) = event.id().as_ref().strip_prefix(ACTION_PREFIX) else {
        return;
    };
    let Some(menus) = app.try_state::<NativeMenus>() else {
        return;
    };
    let target = lock_unpoisoned(&menus.windows).action_target(action);
    if let Some(label) = target {
        if app
            .emit_to(EventTarget::webview_window(label), ACTION_EVENT, action)
            .is_err()
        {
            let _ = app.emit(ERROR_EVENT, ());
        }
    }
}

fn append_actions(
    app: &AppHandle,
    submenu: &Submenu<tauri::Wry>,
    group: &str,
    definitions: &[ActionDefinition],
    snapshot: &MenuSnapshot,
    localizer: &Localizer,
) -> tauri::Result<()> {
    for action in definitions.iter().filter(|action| action.group == group) {
        let accelerator = if cfg!(target_os = "macos") {
            action
                .macos_accelerator
                .as_deref()
                .or(action.accelerator.as_deref())
        } else {
            action.accelerator.as_deref()
        };
        submenu.append(&MenuItem::with_id(
            app,
            format!("{ACTION_PREFIX}{}", action.id),
            localizer.t(&action.label_key),
            snapshot.enabled.contains(&action.id),
            accelerator,
        )?)?;
    }
    Ok(())
}

fn build_menu(
    app: &AppHandle,
    definitions: &[ActionDefinition],
    snapshot: &MenuSnapshot,
) -> tauri::Result<Menu<tauri::Wry>> {
    let localizer = Localizer::load(Some(&snapshot.language));
    let text = |key: &str| localizer.t(&format!("gui.native_menu.{key}"));
    let menu = Menu::new(app)?;
    let about = AboutMetadata {
        name: Some("Squallz".to_owned()),
        version: Some(app.package_info().version.to_string()),
        ..Default::default()
    };
    #[cfg(target_os = "macos")]
    {
        let application = Submenu::new(app, "Squallz", true)?;
        application.append(&PredefinedMenuItem::about(
            app,
            Some(&text("about")),
            Some(about.clone()),
        )?)?;
        application.append(&PredefinedMenuItem::separator(app)?)?;
        append_actions(
            app,
            &application,
            "application",
            definitions,
            snapshot,
            &localizer,
        )?;
        application.append(&PredefinedMenuItem::separator(app)?)?;
        application.append(&PredefinedMenuItem::services(app, Some(&text("services")))?)?;
        application.append(&PredefinedMenuItem::separator(app)?)?;
        application.append(&PredefinedMenuItem::hide(app, Some(&text("hide")))?)?;
        application.append(&PredefinedMenuItem::hide_others(
            app,
            Some(&text("hide_others")),
        )?)?;
        application.append(&PredefinedMenuItem::show_all(app, Some(&text("show_all")))?)?;
        application.append(&PredefinedMenuItem::separator(app)?)?;
        application.append(&PredefinedMenuItem::quit(app, Some(&text("quit")))?)?;
        menu.append(&application)?;
    }
    for group in ["file", "edit", "archive", "view", "window", "help"] {
        let submenu = Submenu::with_id(app, format!("squallz.menu.{group}"), text(group), true)?;
        if group == "edit" {
            submenu.append(&PredefinedMenuItem::undo(app, Some(&text("undo")))?)?;
            submenu.append(&PredefinedMenuItem::redo(app, Some(&text("redo")))?)?;
            submenu.append(&PredefinedMenuItem::separator(app)?)?;
            submenu.append(&PredefinedMenuItem::cut(app, Some(&text("cut")))?)?;
            submenu.append(&PredefinedMenuItem::copy(app, Some(&text("copy")))?)?;
            submenu.append(&PredefinedMenuItem::paste(app, Some(&text("paste")))?)?;
        }
        append_actions(app, &submenu, group, definitions, snapshot, &localizer)?;
        if group == "file" {
            submenu.append(&PredefinedMenuItem::separator(app)?)?;
            submenu.append(&PredefinedMenuItem::close_window(
                app,
                Some(&text("close_window")),
            )?)?;
            #[cfg(not(target_os = "macos"))]
            {
                append_actions(
                    app,
                    &submenu,
                    "application",
                    definitions,
                    snapshot,
                    &localizer,
                )?;
                submenu.append(&PredefinedMenuItem::quit(app, Some(&text("quit")))?)?;
            }
        }
        if group == "view" {
            #[cfg(target_os = "macos")]
            submenu.append(&PredefinedMenuItem::fullscreen(
                app,
                Some(&text("fullscreen")),
            )?)?;
        }
        if group == "window" {
            submenu.append(&PredefinedMenuItem::minimize(app, Some(&text("minimize")))?)?;
            let maximize_label = if cfg!(target_os = "macos") {
                "zoom"
            } else {
                "maximize"
            };
            submenu.append(&PredefinedMenuItem::maximize(
                app,
                Some(&text(maximize_label)),
            )?)?;
            #[cfg(target_os = "macos")]
            {
                submenu.append(&PredefinedMenuItem::bring_all_to_front(
                    app,
                    Some(&text("bring_all_to_front")),
                )?)?;
                submenu.set_as_windows_menu_for_nsapp()?;
            }
        }
        if group == "help" {
            #[cfg(not(target_os = "macos"))]
            submenu.append(&PredefinedMenuItem::about(
                app,
                Some(&text("about")),
                Some(about.clone()),
            )?)?;
            #[cfg(target_os = "macos")]
            submenu.set_as_help_menu_for_nsapp()?;
        }
        menu.append(&submenu)?;
    }
    Ok(menu)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(language: &str, actions: &[&str]) -> MenuSnapshot {
        MenuSnapshot {
            language: language.to_owned(),
            enabled: actions.iter().map(|id| (*id).to_owned()).collect(),
        }
    }

    #[test]
    fn actions_follow_the_focused_window_and_are_revoked_on_close() {
        let mut menus = WindowMenus::default();
        menus.windows.insert(
            "main".to_owned(),
            snapshot("zh-CN", &["rename_entry", "select_all"]),
        );
        menus
            .windows
            .insert("task-1".to_owned(), snapshot("en-US", &["select_all"]));
        menus.focused = Some("main".to_owned());
        assert_eq!(menus.action_target("rename_entry").as_deref(), Some("main"));
        menus.focused = Some("task-1".to_owned());
        assert_eq!(menus.action_target("rename_entry"), None);
        assert_eq!(menus.action_target("select_all").as_deref(), Some("task-1"));
        assert_eq!(menus.current("en-US").language, "en-US");
        menus.release("task-1");
        assert_eq!(menus.action_target("select_all"), None);
        assert!(menus.current("en-US").enabled.is_empty());
    }

    #[test]
    fn background_updates_do_not_enable_actions_in_the_active_window() {
        let mut menus = WindowMenus {
            focused: Some("task-1".to_owned()),
            ..Default::default()
        };
        menus
            .windows
            .insert("main".to_owned(), snapshot("zh-CN", &["delete_entries"]));
        assert!(menus.current("en-US").enabled.is_empty());
        assert_eq!(menus.action_target("delete_entries"), None);
        menus
            .windows
            .insert("task-1".to_owned(), snapshot("en-US", &["select_all"]));
        assert_eq!(menus.current("en-US"), snapshot("en-US", &["select_all"]));
    }

    #[test]
    fn shared_action_definitions_have_unique_ids_and_bilingual_labels() {
        let definitions: Vec<ActionDefinition> = serde_json::from_str(DEFINITIONS).unwrap();
        let mut ids = HashSet::new();
        for action in definitions {
            assert!(ids.insert(action.id));
            assert!(
                ["file", "edit", "archive", "view", "application"].contains(&action.group.as_str())
            );
            for language in ["en-US", "zh-CN"] {
                let localizer = Localizer::with_user_dir(Some(language), None);
                assert_ne!(localizer.t(&action.label_key), action.label_key);
            }
        }
    }
}
