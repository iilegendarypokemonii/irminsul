use crate::{AppState, Message, State, capture::BackendType};
use anyhow::Result;
use irminsul_core::{CaptureController, DataSelection};
use tokio::sync::{mpsc, watch};

/// GUI adapter for the same capture core used by native integrations.
pub struct Monitor {
    controller: CaptureController,
    state_tx: watch::Sender<AppState>,
    messages: mpsc::UnboundedReceiver<Message>,
    error: Option<String>,
}

impl Monitor {
    pub async fn new(
        state_tx: watch::Sender<AppState>,
        messages: mpsc::UnboundedReceiver<Message>,
        _log_packets: watch::Receiver<bool>,
        backend: BackendType,
    ) -> Result<Self> {
        anyhow::ensure!(
            backend == BackendType::Pktmon,
            "Multi-account capture currently requires the Windows Packet Monitor backend."
        );
        Ok(Self {
            controller: CaptureController::new()?,
            state_tx,
            messages,
            error: None,
        })
    }

    pub async fn run(mut self) {
        let mut poll = tokio::time::interval(std::time::Duration::from_millis(250));
        loop {
            tokio::select! {
                message = self.messages.recv() => {
                    let Some(message) = message else { break; };
                    self.message(message);
                }
                _ = poll.tick() => self.publish(),
            }
        }
        self.controller.stop();
    }

    fn message(&mut self, message: Message) {
        match message {
            Message::StartCapture => {
                self.error = self.controller.start().err().map(|error| error.to_string());
            }
            Message::StopCapture => {
                self.error = None;
                self.controller.stop();
            }
            Message::ExportGenshinOptimizer(uid, capture_id, settings, reply) => {
                let result = self
                    .controller
                    .snapshot(&uid, &capture_id)
                    .and_then(|snapshot| {
                        let selection = DataSelection {
                            artifacts: settings.include_artifacts,
                            characters: settings.include_characters,
                            weapons: settings.include_weapons,
                            materials: settings.include_materials,
                        };
                        Ok(serde_json::to_string_pretty(&snapshot.export(&selection)?)?)
                    });
                let _ = reply.send(result);
            }
            _ => (),
        }
        self.publish();
    }

    fn publish(&mut self) {
        if let Ok(capture) = self.controller.state() {
            if capture.capturing && capture.phase != "error" {
                self.error = None;
            }
            self.state_tx.send_modify(|state| {
                state.state = State::Main;
                state.capturing = capture.capturing;
                state.capture = capture;
                if let Some(error) = &self.error {
                    state.capture.phase = "error".into();
                    state.capture.message = error.clone();
                }
            });
        }
    }
}
