//! Checking for, installing and announcing new releases.

use mog_tui::{Overlay, release_notes::ReleaseNotes};

use super::App;
use crate::update::{self, Release, UpdateEvent};

impl App {
    /// Shows what is new after an update and looks for the next one.
    ///
    /// Not part of [`App::new`] so snapshots stay offline.
    pub fn start_updates(&mut self) {
        if let Some(previous) = update::remember_version()
            && update::is_newer(update::VERSION, &previous)
        {
            self.updater.notes(Some(format!("v{}", update::VERSION)));
        }
        if self.ui.config.updates.check {
            self.updater.check(false, self.ui.config.updates.install);
        }
    }

    /// Acts on finished update work.
    pub(super) fn handle_update(&mut self, event: UpdateEvent) {
        match event {
            UpdateEvent::Checked {
                result: Ok(Some(release)),
                install,
                ..
            } => {
                let version = release.version().to_owned();
                self.update = Some(release.clone());
                let message = match update::cannot_install() {
                    None if install => {
                        self.updater.install(release);
                        format!("downloading mog v{version} in the background")
                    }
                    None => format!("mog v{version} is out, run Help: Update mog"),
                    Some(reason) => format!("mog v{version} is out, but {reason}"),
                };
                self.editor.set_status(message);
            }
            UpdateEvent::Checked {
                result: Ok(None),
                manual: true,
                ..
            } => self.editor.set_status(format!(
                "mog v{} is the newest, keep mogging",
                update::VERSION
            )),
            UpdateEvent::Checked {
                result: Err(err),
                manual: true,
                ..
            } => self
                .editor
                .set_status(format!("could not check for updates: {err}")),
            UpdateEvent::Checked { .. } => {}
            UpdateEvent::Installed(Ok(release)) => {
                self.update = None;
                self.editor.set_status(format!(
                    "updated to mog v{}, restart to use it. Help: What's new has the notes",
                    release.version()
                ));
                self.installed = Some(release);
            }
            UpdateEvent::Installed(Err(err)) => {
                self.editor.set_status(format!("update failed: {err}"));
            }
            UpdateEvent::Notes(Ok(release)) => self.show_release_notes(&release),
            UpdateEvent::Notes(Err(err)) => self
                .editor
                .set_status(format!("could not get the release notes: {err}")),
        }
    }

    /// Opens the notes of `release` in a popup.
    pub(super) fn show_release_notes(&mut self, release: &Release) {
        let newer = update::is_newer(release.version(), update::VERSION);
        let installed = self
            .installed
            .as_ref()
            .is_some_and(|installed| installed.tag_name == release.tag_name);
        let title = if newer && !installed {
            format!("mog v{} is out", release.version())
        } else {
            format!("what's new in mog v{}", release.version())
        };
        self.ui.release_notes = Some(ReleaseNotes {
            title,
            body: release.body.clone().unwrap_or_default(),
            url: release.html_url.clone(),
            can_update: newer && !installed && update::cannot_install().is_none(),
        });
        self.ui.open(Overlay::ReleaseNotes);
    }
}
