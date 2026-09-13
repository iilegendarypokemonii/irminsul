use std::fmt::Display;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::thread;

use anyhow::{Context as _, Result, anyhow};
use chrono::Local;
use egui::{
    Button, Color32, Context, DragValue, Id, Key, KeyboardShortcut, Modal, Modifiers, OpenUrl,
    PointerButton, RichText, Sense, ViewportCommand,
};
use egui_file_dialog::FileDialog;
use egui_notify::Toasts;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot, watch};

use crate::monitor::Monitor;
use crate::update::check_for_app_update;
use crate::{
    AppState, ConfirmationType, Message, ReloadHandle, State, TracingLevel, admin, capture,
    open_log_dir, wish,
};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SavedAppState {
    #[serde(default)]
    selection: irminsul_core::DataSelection,
    #[serde(default)]
    export_settings: irminsul_core::player_data::ExportSettings,
    #[serde(default)]
    auto_start_capture: bool,
    #[serde(default)]
    tracing_level: TracingLevel,
}

impl Default for SavedAppState {
    fn default() -> Self {
        Self {
            selection: irminsul_core::DataSelection::default(),
            export_settings: Default::default(),
            auto_start_capture: false,
            tracing_level: Default::default(),
        }
    }
}

#[derive(Clone, Debug)]
enum OptimizerExportTarget {
    None,
    Clipboard,
    File,
}

pub struct IrminsulApp {
    ui_message_tx: mpsc::UnboundedSender<Message>,
    state_rx: watch::Receiver<AppState>,
    wish_url_rx: watch::Receiver<Option<String>>,
    tracing_reload_handle: ReloadHandle,

    toasts: Toasts,

    power_tools_open: bool,
    bug_report_open: bool,

    capture_settings_open: bool,
    optimizer_settings_open: bool,
    selected_uid: Option<String>,

    optimizer_export_rx: Option<oneshot::Receiver<Result<String>>>,
    optimizer_save_dialog: Option<FileDialog>,
    optimizer_save_path: Option<PathBuf>,
    optimizer_export_target: OptimizerExportTarget,

    restarting: bool,

    saved_state: SavedAppState,
}

trait ToastError<T> {
    fn toast_error(self, app: &mut IrminsulApp) -> Option<T>;
}

impl<T, E: Display> ToastError<T> for std::result::Result<T, E> {
    fn toast_error(self, app: &mut IrminsulApp) -> Option<T> {
        match self {
            Ok(val) => Some(val),
            Err(e) => {
                tracing::error!("{e}");
                app.toasts.error(e.to_string());
                None
            }
        }
    }
}

fn start_async_runtime(
    egui_ctx: Context,
    log_packets_rx: watch::Receiver<bool>,
    capture_backend: capture::BackendType,
) -> (
    mpsc::UnboundedSender<Message>,
    watch::Receiver<AppState>,
    watch::Receiver<Option<String>>,
) {
    tracing::info!("starting tokio async");
    let (ui_message_tx, mut ui_message_rx) = mpsc::unbounded_channel::<Message>();

    let (state_tx, state_rx) = watch::channel(AppState::new());
    let (wish_url_tx, wish_url_rx) = watch::channel(None);
    let mut updater_state_rx = state_rx.clone();
    let updater_ctx = egui_ctx.clone();
    thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();

        rt.block_on(async {
            // Before starting the monitor, check for updates if not in debug mode
            tracing::info!("Checking for update");
            if let Err(e) = check_for_app_update(&state_tx, &mut ui_message_rx).await {
                tracing::error!("error checking for update: {e}");
            }

            // Check for wish URL
            tokio::spawn(async move {
                let Ok(mut wish) = wish::Wish::new(wish_url_tx).await else {
                    tracing::error!("Failed to create new wish monitor");
                    return;
                };

                if let Err(e) = wish.monitor().await {
                    tracing::error!("Error monitoring for wishes: {e}");
                }
            });

            // Notify egui of state changes.
            tokio::spawn(async move {
                loop {
                    let _ = updater_state_rx.changed().await;
                    updater_ctx.request_repaint();
                }
            });
            tracing::info!("Starting monitor");
            let monitor = match Monitor::new(
                state_tx,
                ui_message_rx,
                log_packets_rx,
                capture_backend,
            )
            .await
            {
                Ok(monitor) => monitor,
                Err(e) => {
                    tracing::error!("error loading monitor task: {e}");
                    return;
                }
            };
            monitor.run().await;
        });
    });
    tracing::info!("started tokio");
    (ui_message_tx, state_rx, wish_url_rx)
}

impl IrminsulApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        mut tracing_reload_handle: ReloadHandle,
        capture_backend: capture::BackendType,
    ) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        egui_material_icons::initialize(&cc.egui_ctx);

        let saved_state: SavedAppState = if let Some(storage) = cc.storage {
            eframe::get_value(storage, eframe::APP_KEY).unwrap_or_default()
        } else {
            Default::default()
        };

        tracing_reload_handle.set_filter(saved_state.tracing_level.get_filter());
        let (_log_packets_tx, log_packets_rx) = watch::channel(false);
        let (ui_message_tx, state_rx, wish_url_rx) =
            start_async_runtime(cc.egui_ctx.clone(), log_packets_rx, capture_backend);

        if saved_state.auto_start_capture {
            if let Err(e) = ui_message_tx.send(Message::StartCapture) {
                tracing::error!("Failed to send auto start message: {e}");
            }
        }

        let toasts = Toasts::default().with_anchor(egui_notify::Anchor::BottomLeft);

        Self {
            saved_state,
            ui_message_tx,
            tracing_reload_handle,
            toasts,
            power_tools_open: false,
            bug_report_open: false,
            capture_settings_open: false,
            optimizer_settings_open: false,
            selected_uid: None,
            optimizer_export_rx: None,
            optimizer_save_dialog: None,
            optimizer_save_path: None,
            optimizer_export_target: OptimizerExportTarget::None,
            restarting: false,
            state_rx,
            wish_url_rx,
        }
    }
}

impl eframe::App for IrminsulApp {
    /// Called by the framework to save state before shutdown.
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, eframe::APP_KEY, &self.saved_state);
    }

    /// Called each time the UI needs repainting, which may be many times per second.
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.style_mut(|style| {
            style.interaction.selectable_labels = false;
            style.interaction.tooltip_delay = 0.25;
        });

        self.toasts.show(ctx);
        if let Some(optimizer_save_dialog) = &mut self.optimizer_save_dialog {
            optimizer_save_dialog.update(ctx);
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.with_layout(egui::Layout::top_down(egui::Align::LEFT), |ui| {
                egui::Image::new(egui::include_image!("../assets/background.webp"))
                    .paint_at(ui, ui.ctx().screen_rect());
            });

            ui.vertical(|ui| {
                self.title_bar(ui);
                ui.add_space(25.);

                // Handle power tools here instead of main UI to allow it to be opened
                // in other app states.
                let power_tools_shortcut = KeyboardShortcut {
                    modifiers: Modifiers {
                        command: true,
                        shift: true,
                        ..Default::default()
                    },
                    logical_key: Key::P,
                };
                ui.ctx().input_mut(|i| {
                    if i.consume_shortcut(&power_tools_shortcut) {
                        self.power_tools_open = true;
                    }
                });

                if self.power_tools_open {
                    let modal = Modal::new(Id::new("Power Tools")).show(ui.ctx(), |ui| {
                        self.power_tools_modal(ui);
                    });
                    if modal.should_close() {
                        self.power_tools_open = false;
                    }
                }

                if self.bug_report_open {
                    let modal = Modal::new(Id::new("Bug Report")).show(ui.ctx(), |ui| {
                        self.bug_report_modal(ui);
                    });
                    if modal.should_close() {
                        self.bug_report_open = false;
                    }
                }

                ui.horizontal(|ui| {
                    ui.add_space(525.);
                    let state = self.state_rx.borrow_and_update().clone();
                    ui.vertical(|ui| match state.state {
                        State::Starting => (),
                        State::CheckingForUpdate => self.checking_for_update_ui(ui),
                        State::WaitingForUpdateConfirmation(status) => {
                            self.waiting_for_update_confirmation_ui(ui, status)
                        }
                        State::Updating => self.updating_ui(ui),
                        State::Updated { needs_caps } => self.updated_ui(ui, needs_caps),
                        State::CheckingForData => self.checking_for_data_ui(ui),
                        State::WaitingForDownloadConfirmation(confirmation_type) => {
                            self.waiting_for_download_confirmation_ui(ui, confirmation_type)
                        }
                        State::Downloading => self.load_data_ui(ui),
                        State::Main => self.main_ui(ui, &state),
                    });
                });
            });

            ui.with_layout(egui::Layout::bottom_up(egui::Align::RIGHT), |ui| {
                ui.horizontal(|ui| {
                    let discord_icon = egui::include_image!("../assets/discord.svg");
                    if ui
                        .add(
                            Button::image(discord_icon)
                                .frame(false)
                                .image_tint_follows_text_color(true),
                        )
                        .clicked()
                    {
                        ui.ctx()
                            .open_url(OpenUrl::new_tab("https://discord.gg/aQqdZPHEpP"));
                    }

                    use egui::special_emojis::GITHUB;
                    if ui
                        .add(Button::new(RichText::new(GITHUB).size(16.)).frame(false))
                        .clicked()
                    {
                        ui.ctx()
                            .open_url(OpenUrl::new_tab("https://github.com/konkers/irminsul"));
                    }

                    let button = ui.add(
                        Button::new(
                            RichText::new(egui_material_icons::icons::ICON_BUG_REPORT).size(16.),
                        )
                        .frame(false),
                    );
                    if button.clicked() {
                        self.bug_report_open = true;
                    }
                    ui.label(env!("CARGO_PKG_VERSION").to_string());
                    egui::warn_if_debug_build(ui);
                });
            });
        });
    }
}

impl IrminsulApp {
    fn title_bar(&self, ui: &mut egui::Ui) {
        let (_, button_width) = egui::Sides::new().show(
            ui,
            |_ui| {},
            |ui| {
                let button = ui.add(
                    Button::new(RichText::new(egui_material_icons::icons::ICON_CLOSE).size(24.))
                        .frame(false),
                );
                if button.clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
                button.rect.width()
            },
        );

        let app_rect = ui.max_rect();

        let title_bar_height = 32.0;
        let title_bar_rect = {
            let mut rect = app_rect;
            rect.max.y = rect.min.y + title_bar_height;
            rect.max.x -= button_width;
            rect
        };

        let response = ui.interact(
            title_bar_rect,
            Id::new("title_bar"),
            Sense::click_and_drag(),
        );

        if response.drag_started_by(PointerButton::Primary) {
            ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
        }
    }

    fn checking_for_update_ui(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Checking for Irminsul updates".to_string());
        });
    }

    fn waiting_for_update_confirmation_ui(&self, ui: &mut egui::Ui, version: String) {
        ui.label(format!(
            "Update {} available.  Download and install?",
            version
        ));

        ui.horizontal(|ui| {
            if ui.add(egui::Button::new("Yes")).clicked() {
                if let Err(e) = self.ui_message_tx.send(Message::UpdateAcknowledged) {
                    tracing::error!("Unable to send UI message: {e}");
                }
            }
            if ui.add(egui::Button::new("No")).clicked() {
                if let Err(e) = self.ui_message_tx.send(Message::UpdateCanceled) {
                    tracing::error!("Unable to send UI message: {e}");
                }
            }
        });
    }

    fn updating_ui(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Downloading and updating...".to_string());
            ui.spinner();
        });
    }

    /// Relaunch Irminsul and close this instance.
    ///
    /// Uses the executable's own path rather than argv[0], which may be a bare
    /// name that only resolves through PATH, or be relative to a directory we
    /// are no longer in.
    fn restart(&mut self, ui: &mut egui::Ui) {
        match std::env::current_exe() {
            Ok(exe_path) => {
                let _ = std::process::Command::new(exe_path).spawn();
            }
            Err(e) => tracing::error!("unable to find Irminsul to restart it: {e}"),
        }
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        self.restarting = true;
    }

    fn updated_ui(&mut self, ui: &mut egui::Ui, needs_caps: bool) {
        if needs_caps {
            return self.update_reset_permissions_ui(ui);
        }

        ui.horizontal(|ui| {
            ui.label("Updated. Restarting...".to_string());
        });
        if !self.restarting {
            self.restart(ui);
        }
    }

    /// Shown when the update dropped the packet capture permissions the user
    /// had granted.  Restarting without re-granting them just lands in the
    /// missing permissions dialog, so ask here instead of restarting straight
    /// away.
    fn update_reset_permissions_ui(&mut self, ui: &mut egui::Ui) {
        let exe_path = admin::current_exe_path();
        let command = admin::setcap_command(&exe_path);

        ui.label("Updated.  The update reset Irminsul's packet capture");
        ui.label("permissions.  Re-grant them with:");
        ui.add_space(5.0);
        ui.label(&command);
        ui.add_space(5.0);

        ui.horizontal(|ui| {
            if ui.button("Copy").clicked() {
                ui.ctx().copy_text(command.clone());
            }
            if ui.button("Restart").clicked() && !self.restarting {
                self.restart(ui);
            }
        });
    }

    fn checking_for_data_ui(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Checking for game data updates".to_string());
        });
    }

    fn waiting_for_download_confirmation_ui(
        &self,
        ui: &mut egui::Ui,
        confirmation_type: ConfirmationType,
    ) {
        let label = match confirmation_type {
            ConfirmationType::Initial => "Irminsul needs to download initial data",
            ConfirmationType::Update => "New data available",
        };
        ui.label(label.to_string());
        if ui.add(egui::Button::new("Download")).clicked() {
            if let Err(e) = self.ui_message_tx.send(Message::DownloadAcknowledged) {
                tracing::error!("Unable to send UI message{e}");
            }
        }
    }

    fn load_data_ui(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Downloading Data".to_string());
            ui.spinner();
        });
    }

    fn main_ui(&mut self, ui: &mut egui::Ui, app_state: &AppState) {
        if self.capture_settings_open {
            let modal = Modal::new(Id::new("Capture Settings")).show(ui.ctx(), |ui| {
                self.capture_settings_modal(ui);
            });
            if modal.should_close() {
                self.capture_settings_open = false;
            }
        }

        if self.optimizer_settings_open {
            let modal = Modal::new(Id::new("Optimizer Settings")).show(ui.ctx(), |ui| {
                self.optimizer_settings_modal(ui);
            });
            if modal.should_close() {
                self.optimizer_settings_open = false;
            }
        }
        self.capture_ui(ui, app_state);
        ui.separator();
        self.genshin_optimizer_ui(ui, app_state);
        ui.separator();
        self.wish_ui(ui);
        ui.separator();
        self.achievement_ui(ui, app_state);
    }

    fn capture_ui(&mut self, ui: &mut egui::Ui, app_state: &AppState) {
        ui.vertical(|ui| {
            egui::Sides::new().show(
                ui,
                |ui| {
                    Self::section_header(ui, "Packet Capture");
                },
                |ui| {
                    if ui
                        .button(egui_material_icons::icons::ICON_SETTINGS)
                        .clicked()
                    {
                        self.capture_settings_open = true;
                    }

                    if app_state.capturing {
                        if ui
                            .button(egui_material_icons::icons::ICON_PAUSE)
                            .on_hover_text("Stop capture")
                            .clicked()
                        {
                            let _ = self.ui_message_tx.send(Message::StopCapture);
                        }
                    } else if ui
                        .button(egui_material_icons::icons::ICON_PLAY_ARROW)
                        .on_hover_text("Start capture")
                        .clicked()
                    {
                        let _ = self.ui_message_tx.send(Message::StartCapture);
                    }
                },
            );
        });
        ui.label(&app_state.capture.message).on_hover_text(
            "Start capture before entering the door. Keep it running while switching accounts through the title menu. Capture stops after four hours; completed login snapshots stay available."
        );
        self.account_selection_ui(ui, app_state);
    }

    fn genshin_optimizer_ui(&mut self, ui: &mut egui::Ui, _app_state: &AppState) {
        self.optimizer_handle_export(ui).toast_error(self);
        ui.vertical(|ui| {
            egui::Sides::new().show(
                ui,
                |ui| {
                    Self::section_header(ui, "Genshin Optimizer");
                },
                |ui| {
                    if ui
                        .button(egui_material_icons::icons::ICON_SETTINGS)
                        .on_hover_text("Import and export settings")
                        .clicked()
                    {
                        self.optimizer_settings_open = true;
                    }
                    ui.add_enabled_ui(
                        self.selected_uid.is_some()
                            && (self.saved_state.selection.artifacts
                                || self.saved_state.selection.weapons
                                || self.saved_state.selection.characters
                                || self.saved_state.selection.materials)
                            && self.optimizer_export_rx.is_none(),
                        |ui| {
                            if ui
                                .button(egui_material_icons::icons::ICON_DOWNLOAD)
                                .clicked()
                            {
                                let now = Local::now();
                                let mut optimizer_save_dialog = FileDialog::new()
                                    .add_file_filter_extensions("JSON files", vec!["json"])
                                    .default_file_name(&format!(
                                        "irminsul_{}_{}.json",
                                        self.selected_uid.as_deref().unwrap_or("unknown"),
                                        now.format("%Y-%m-%d_%H-%M")
                                    ));
                                optimizer_save_dialog.save_file();
                                self.optimizer_save_dialog = Some(optimizer_save_dialog);
                            }

                            if let Some(optimizer_save_dialog) = &mut self.optimizer_save_dialog
                                && let Some(path) = optimizer_save_dialog.take_picked()
                            {
                                self.optimizer_save_path = Some(path);
                                self.genshin_optimizer_request_export(OptimizerExportTarget::File);
                            }

                            if ui
                                .button(egui_material_icons::icons::ICON_CONTENT_PASTE_GO)
                                .clicked()
                            {
                                self.genshin_optimizer_request_export(
                                    OptimizerExportTarget::Clipboard,
                                );
                            }
                        },
                    );
                },
            );
        });
    }

    fn account_selection_ui(&mut self, ui: &mut egui::Ui, app_state: &AppState) {
        if self.selected_uid.is_none() {
            self.selected_uid = app_state.capture.snapshots.first().map(|s| s.uid.clone());
        }
        egui::ComboBox::from_id_salt("captured-account")
            .selected_text(
                self.selected_uid
                    .as_deref()
                    .unwrap_or("No captured accounts"),
            )
            .show_ui(ui, |ui| {
                for snapshot in &app_state.capture.snapshots {
                    ui.selectable_value(
                        &mut self.selected_uid,
                        Some(snapshot.uid.clone()),
                        &snapshot.uid,
                    );
                }
            });
        let snapshot = app_state
            .capture
            .snapshots
            .iter()
            .find(|s| Some(&s.uid) == self.selected_uid.as_ref());
        egui::Grid::new("capture_stats")
            .num_columns(2)
            .min_col_width(0.)
            .show(ui, |ui| {
                for (label, count) in [
                    ("Artifacts", snapshot.map(|s| s.counts.artifacts)),
                    ("Weapons", snapshot.map(|s| s.counts.weapons)),
                    ("Characters", snapshot.map(|s| s.counts.characters)),
                    ("Materials", snapshot.map(|s| s.counts.materials)),
                ] {
                    let icon = if count.is_some() {
                        RichText::new(egui_material_icons::icons::ICON_CHECK_CIRCLE)
                            .color(Color32::from_rgb(0, 171, 63))
                    } else {
                        RichText::new(egui_material_icons::icons::ICON_CHECK_INDETERMINATE_SMALL)
                    };
                    ui.label(icon);
                    ui.label(count.map_or_else(|| label.to_string(), |n| format!("{label}: {n}")));
                    ui.end_row();
                }
            });
        if let Some(snapshot) = snapshot {
            if let Some(time) =
                chrono::DateTime::from_timestamp_millis(snapshot.captured_at_ms as i64)
            {
                ui.small(format!("Login snapshot: {}", time.with_timezone(&Local).format("%Y-%m-%d %H:%M:%S")))
                    .on_hover_text("Inventory at login. Loot and upgrades made afterwards are not included. Log in again with capture running to refresh this account.");
            }
            for warning in &snapshot.warnings {
                ui.colored_label(Color32::YELLOW, warning);
            }
        }
    }

    fn genshin_optimizer_request_export(&mut self, target: OptimizerExportTarget) {
        let state = self.state_rx.borrow().clone();
        let Some(snapshot) = state
            .capture
            .snapshots
            .iter()
            .find(|s| Some(&s.uid) == self.selected_uid.as_ref())
        else {
            return;
        };
        let (tx, rx) = oneshot::channel();
        let _ = self.ui_message_tx.send(Message::ExportGenshinOptimizer(
            snapshot.uid.clone(),
            snapshot.capture_id.clone(),
            irminsul_core::player_data::ExportSettings {
                include_artifacts: self.saved_state.selection.artifacts,
                include_characters: self.saved_state.selection.characters,
                include_weapons: self.saved_state.selection.weapons,
                include_materials: self.saved_state.selection.materials,
                ..self.saved_state.export_settings.clone()
            },
            tx,
        ));
        self.optimizer_export_target = target;
        self.optimizer_export_rx = Some(rx);
    }

    fn wish_ui(&mut self, ui: &mut egui::Ui) {
        self.optimizer_handle_export(ui).toast_error(self);

        let wish_url = self.wish_url_rx.borrow_and_update().clone();
        ui.vertical(|ui| {
            egui::Sides::new().show(
                ui,
                |ui| {
                    Self::section_header(ui, "Wish History");
                    ui.label(egui_material_icons::icons::ICON_HELP)
                        .on_hover_text("Click the Copy icon to copy the wish URL to the clipboard.  Paste this into paimon.moe using the Manual auto-import method.");
                },
                |ui| {
                    ui.add_enabled_ui(wish_url.is_some(), |ui| {
                        if ui
                            .button(egui_material_icons::icons::ICON_CONTENT_PASTE_GO)
                            .clicked()
                        {
                            if let Some(url) = wish_url {
                                ui.ctx().copy_text(url);
                            }
                        }
                    });
                },
            );
        });
    }

    fn power_tools_modal(&mut self, ui: &mut egui::Ui) {
        ui.set_width(300.0);
        ui.heading("Power Tools");
        ui.separator();
        let prev_level = self.saved_state.tracing_level;
        egui::ComboBox::from_label("Logging Level")
            .selected_text(format!("{}", self.saved_state.tracing_level))
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut self.saved_state.tracing_level,
                    TracingLevel::Default,
                    "Default",
                );
                ui.selectable_value(
                    &mut self.saved_state.tracing_level,
                    TracingLevel::VerboseInfo,
                    "Verbose Info",
                );
                ui.selectable_value(
                    &mut self.saved_state.tracing_level,
                    TracingLevel::VerboseDebug,
                    "Verbose Debug",
                );
                ui.selectable_value(
                    &mut self.saved_state.tracing_level,
                    TracingLevel::VerboseTrace,
                    "Verbose Trace",
                );
            });
        if prev_level != self.saved_state.tracing_level {
            self.tracing_reload_handle
                .set_filter(self.saved_state.tracing_level.get_filter());
        }
        ui.end_row();
        ui.separator();
        egui::Sides::new().show(
            ui,
            |_ui| {},
            |ui| {
                if ui.button("Ok").clicked() {
                    ui.close()
                }
            },
        );
    }

    fn bug_report_modal(&mut self, ui: &mut egui::Ui) {
        ui.set_width(300.0);
        ui.heading("Bug Report");
        ui.separator();
        ui.label("When filing a bug, please include the latest log file:");
        if ui.button("Open log directory").clicked() {
            thread::spawn(|| {
                let _ = open_log_dir();
            });
        }
        ui.separator();
        egui::Sides::new().show(
            ui,
            |_ui| {},
            |ui| {
                if ui.button("New GitHub Issue").clicked() {
                    ui.ctx().open_url(OpenUrl::new_tab(
                        "https://github.com/konkers/irminsul/issues/new",
                    ));
                    ui.close()
                }
                if ui.button("Cancel").clicked() {
                    ui.close()
                }
            },
        );
    }

    fn capture_settings_modal(&mut self, ui: &mut egui::Ui) {
        ui.set_width(300.0);
        ui.heading("Capture settings");
        ui.separator();
        ui.checkbox(
            &mut self.saved_state.auto_start_capture,
            "Start capture on Irminsul launch",
        );
        ui.separator();
        egui::Sides::new().show(
            ui,
            |_ui| {},
            |ui| {
                if ui.button("Ok").clicked() {
                    ui.close()
                }
            },
        );
    }

    fn optimizer_settings_modal(&mut self, ui: &mut egui::Ui) {
        ui.set_width(300.0);
        ui.heading("Genshin Optimizer Settings");
        ui.separator();
        ui.checkbox(&mut self.saved_state.selection.characters, "Characters");
        ui.horizontal(|ui| {
            ui.add_space(20.);
            egui::Grid::new("char_options")
                .striped(true)
                .show(ui, |ui| {
                    ui.label("Min level".to_string());
                    ui.add(
                        DragValue::new(&mut self.saved_state.export_settings.min_character_level)
                            .range(1..=100),
                    );
                    ui.end_row();
                    ui.label("Min ascension".to_string());
                    ui.add(
                        DragValue::new(
                            &mut self.saved_state.export_settings.min_character_ascension,
                        )
                        .range(0..=6),
                    );
                    ui.end_row();
                    ui.label("Min constellation".to_string());
                    ui.add(
                        DragValue::new(
                            &mut self.saved_state.export_settings.min_character_constellation,
                        )
                        .range(0..=6),
                    );
                    ui.end_row();
                });
        });
        ui.checkbox(&mut self.saved_state.selection.artifacts, "Artifacts");
        ui.horizontal(|ui| {
            ui.add_space(20.);
            egui::Grid::new("artifact_options")
                .striped(true)
                .show(ui, |ui| {
                    ui.label("Min level".to_string());
                    ui.add(
                        DragValue::new(&mut self.saved_state.export_settings.min_artifact_level)
                            .range(0..=20),
                    );
                    ui.end_row();
                    ui.label("Min rarity".to_string());
                    ui.add(
                        DragValue::new(&mut self.saved_state.export_settings.min_artifact_rarity)
                            .range(1..=5),
                    );
                    ui.end_row();
                });
        });
        ui.checkbox(&mut self.saved_state.selection.weapons, "Weapons");
        ui.horizontal(|ui| {
            ui.add_space(20.);
            egui::Grid::new("weapon_options")
                .striped(true)
                .show(ui, |ui| {
                    ui.label("Min level".to_string());
                    ui.add(
                        DragValue::new(&mut self.saved_state.export_settings.min_weapon_level)
                            .range(1..=90),
                    );
                    ui.end_row();

                    ui.label("Min refinement".to_string());
                    ui.add(
                        DragValue::new(&mut self.saved_state.export_settings.min_weapon_refinement)
                            .range(1..=5),
                    );
                    ui.end_row();

                    ui.label("Min ascension".to_string());
                    ui.add(
                        DragValue::new(&mut self.saved_state.export_settings.min_weapon_ascension)
                            .range(0..=6),
                    );
                    ui.end_row();

                    ui.label("Min rarity".to_string());
                    ui.add(
                        DragValue::new(&mut self.saved_state.export_settings.min_weapon_rarity)
                            .range(1..=5),
                    );
                    ui.end_row();
                });
        });
        ui.checkbox(&mut self.saved_state.selection.materials, "Materials");
        ui.checkbox(
            &mut self.saved_state.export_settings.fake_initialize_4th_line,
            "Fake level-up 5* artifacts with unactivated stats (hover for more info)"
        ).on_hover_text(
            "Compatibility option for older tools. Eligible 5-star artifacts below level 4 are exported at level 4 with their fourth stat activated. Current Genshin Optimizer supports unactivated stats, so leave this off to preserve actual levels. Minimum-level filters use the actual level. Captured snapshots are unchanged."
        );
        ui.separator();
        egui::Sides::new().show(
            ui,
            |_ui| {},
            |ui| {
                if ui.button("Ok").clicked() {
                    ui.close()
                }
            },
        );
    }

    fn achievement_ui(&self, ui: &mut egui::Ui, _app_state: &AppState) {
        Self::section_header(ui, "Achievement Export");
        ui.label("coming soon".to_string());
    }

    fn optimizer_handle_export(&mut self, ui: &mut egui::Ui) -> Result<()> {
        let Some(mut rx) = self.optimizer_export_rx.take() else {
            return Ok(());
        };

        let json = match rx.try_recv() {
            Ok(result) => result?,
            Err(oneshot::error::TryRecvError::Empty) => {
                self.optimizer_export_rx = Some(rx);
                return Ok(());
            }
            Err(_) => return Err(anyhow!("Export worker closed.")),
        };

        match self.optimizer_export_target {
            OptimizerExportTarget::None => {
                tracing::warn!("Unexpected json export");
            }
            OptimizerExportTarget::Clipboard => {
                self.optimizer_save_to_clipboard(ui, json)?;
            }
            OptimizerExportTarget::File => {
                self.optimizer_save_to_file(json)?;
            }
        }

        self.optimizer_export_target = OptimizerExportTarget::None;
        Ok(())
    }

    fn optimizer_save_to_clipboard(&mut self, ui: &mut egui::Ui, json: String) -> Result<()> {
        ui.ctx().copy_text(json);
        self.toasts
            .info("Genshin Optimizer data copied to clipboard");
        Ok(())
    }

    fn optimizer_save_to_file(&mut self, json: String) -> Result<()> {
        let path = self
            .optimizer_save_path
            .take()
            .ok_or_else(|| anyhow!("No save file path set"))?;

        let file = File::create(&path).with_context(|| format!("Unable to open file {path:?}"))?;
        let mut writer = BufWriter::new(file);
        writer.write_all(json.as_bytes())?;

        self.toasts.info("Genshin Optimizer data saved to file");
        Ok(())
    }

    fn section_header(ui: &mut egui::Ui, name: &str) {
        ui.label(RichText::new(name).size(18.));
    }
}
