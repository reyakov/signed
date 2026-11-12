use gpui::{App, AppContext as _};

use super::exit::ExitDialog;
use super::import::ImportDialog;
use super::onboarding::OnboardingDialog;
use super::passphrase::PassphraseDialog;
use crate::shared::dialogs::open_global;

pub fn open_passphrase(cx: &mut App) {
    open_global(PassphraseDialog::new(cx), cx);
}

pub fn open_onboarding(cx: &mut App) {
    open_global(OnboardingDialog::new(cx), cx);
}

pub fn open_import(cx: &mut App) {
    open_global(ImportDialog::new(cx), cx);
}

pub fn open_exit(cx: &mut App) {
    open_global(cx.new(|_| ExitDialog), cx);
}
