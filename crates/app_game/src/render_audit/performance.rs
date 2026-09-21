//! Optional performance diagnostics: UI, recording, telemetry and reports.
mod capture;
mod panel;
mod report;
mod telemetry;

use super::timing;
use crate::runtime_settings::RuntimeSettingsInit;
use bevy::prelude::*;
pub(super) use capture::CaptureSession;
#[cfg(test)]
pub(super) use panel::PanelState;

pub(super) fn install(app: &mut App) {
    app.init_resource::<CaptureSession>()
        .add_systems(PostStartup, capture::initialize.after(RuntimeSettingsInit));
    telemetry::install(app);
    panel::install(app);
}
