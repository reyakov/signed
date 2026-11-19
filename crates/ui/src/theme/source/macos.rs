//! macOS Adapter
//!
//! Reads the system accent and watches the system's color preferences.

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{AnyThread, DefinedClass, define_class, msg_send, sel};
use objc2_app_kit::{
    NSColor, NSColorSpace, NSWorkspace, NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification,
};
use objc2_foundation::{
    NSDictionary, NSDistributedNotificationCenter, NSKeyValueObservingOptions, NSNotification,
    NSNotificationName, NSObjectNSKeyValueObserverRegistration, NSString, NSUserDefaults,
};

use super::signal::ReapplySignal;
use super::{ThemeSource, narrow_component};
use crate::theme::ThemeVariant;
use crate::theme::system::{AccessibilityPreferences, SourceKind, SystemTheme};

// An observer that forwards distributed notifications and preference.
define_class!(
    #[unsafe(super(NSObject))]
    #[ivars = async_channel::Sender<()>]
    struct ThemeObserver;

    impl ThemeObserver {
        #[unsafe(method(themeDidChange:))]
        fn theme_did_change(&self, _notification: &NSNotification) {
            ReapplySignal::poke(self.ivars());
        }

        #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
        fn observe_value_for_key_path(
            &self,
            _key_path: &NSString,
            _object: Option<&AnyObject>,
            _change: Option<&NSDictionary<NSString, AnyObject>>,
            _context: *mut core::ffi::c_void,
        ) {
            ReapplySignal::poke(self.ivars());
        }
    }
);

impl ThemeObserver {
    fn new(reapply: async_channel::Sender<()>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(reapply);
        unsafe { msg_send![super(this), init] }
    }
}

#[derive(Default)]
pub(crate) struct MacosSource {
    /// The notification centers track, but do not retain, the observer.
    _observer: Option<Retained<ThemeObserver>>,
    /// Keeps the reapply loop alive.
    _signal: Option<ReapplySignal>,
}

impl MacosSource {
    fn color_to_hsla(color: &NSColor) -> Option<gpui::Hsla> {
        let srgb = NSColorSpace::sRGBColorSpace();
        let converted = color.colorUsingColorSpace(&srgb)?;

        Some(gpui::Hsla::from(gpui::Rgba {
            r: narrow_component(converted.redComponent()),
            g: narrow_component(converted.greenComponent()),
            b: narrow_component(converted.blueComponent()),
            a: narrow_component(converted.alphaComponent()),
        }))
    }
}

impl ThemeSource for MacosSource {
    fn snapshot(&self, variant: ThemeVariant) -> SystemTheme {
        let workspace = NSWorkspace::sharedWorkspace();

        let accessibility = AccessibilityPreferences {
            high_contrast: workspace.accessibilityDisplayShouldIncreaseContrast(),
            reduce_motion: workspace.accessibilityDisplayShouldReduceMotion(),
            reduce_transparency: workspace.accessibilityDisplayShouldReduceTransparency(),
        };

        SystemTheme {
            variant,
            accent: Self::color_to_hsla(&NSColor::controlAccentColor()),
            accessibility,
            // macOS blurs what is behind the window natively.
            window_background: Some(gpui::WindowBackgroundAppearance::Blurred),
            source: SourceKind::Macos,
        }
    }

    /// Re-runs the resolve pipeline when the system's theme changes.
    fn install_watchers(&mut self, async_cx: gpui::AsyncApp) {
        let signal = ReapplySignal::install(&async_cx);
        let observer = ThemeObserver::new(signal.sender());

        let center = NSDistributedNotificationCenter::defaultCenter();
        for name in [
            "AppleColorPreferencesChangedNotification",
            "AppleInterfaceThemeChangedNotification",
        ] {
            let name = NSNotificationName::from_str(name);
            // SAFETY: the observer is retained past every delivery.
            unsafe {
                center.addObserver_selector_name_object(
                    &observer,
                    sel!(themeDidChange:),
                    Some(&name),
                    None,
                );
            }
        }

        let defaults = NSUserDefaults::standardUserDefaults();
        for key in [
            "AppleAccentColor",
            "AppleAquaColorVariant",
            "AppleInterfaceStyle",
        ] {
            let key = NSString::from_str(key);
            // SAFETY: as above
            unsafe {
                defaults.addObserver_forKeyPath_options_context(
                    &observer,
                    &key,
                    NSKeyValueObservingOptions::empty(),
                    core::ptr::null_mut(),
                );
            }
        }

        let workspace_center = NSWorkspace::sharedWorkspace().notificationCenter();
        let accessibility_name =
            unsafe { NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification };

        // SAFETY: as above.
        unsafe {
            workspace_center.addObserver_selector_name_object(
                &observer,
                sel!(themeDidChange:),
                Some(accessibility_name),
                None,
            );
        }

        self._observer = Some(observer);
        self._signal = Some(signal);
    }

    fn label(&self) -> &'static str {
        SourceKind::Macos.label()
    }
}
