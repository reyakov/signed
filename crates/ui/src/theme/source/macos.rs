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
    NSNotificationCenter, NSNotificationName, NSObjectNSKeyValueObserverRegistration, NSString,
    NSUserDefaults,
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
        // SAFETY: `this` is freshly allocated with ivars set and not yet
        // initialized, which is exactly the receiver `init` requires.
        unsafe { msg_send![super(this), init] }
    }
}

/// One watcher registration, kept so [`Drop`] can undo it.
enum Watcher {
    /// A distributed notification, registered by name.
    Distributed {
        center: Retained<NSDistributedNotificationCenter>,
        name: Retained<NSNotificationName>,
    },
    /// A defaults key path observed via KVO, which does not retain observers.
    Defaults {
        defaults: Retained<NSUserDefaults>,
        key: Retained<NSString>,
    },
    /// A workspace notification, registered by name.
    Workspace {
        center: Retained<NSNotificationCenter>,
        name: &'static NSNotificationName,
    },
}

#[derive(Default)]
pub(crate) struct MacosSource {
    /// The observer, retained until `Drop` has removed every registration.
    observer: Option<Retained<ThemeObserver>>,
    /// Registrations to undo, in the order they were installed.
    watchers: Vec<Watcher>,
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
        let mut watchers = Vec::new();

        let center = NSDistributedNotificationCenter::defaultCenter();
        for name in [
            "AppleColorPreferencesChangedNotification",
            "AppleInterfaceThemeChangedNotification",
        ] {
            let name = NSNotificationName::from_str(name);
            // SAFETY: the center tracks this exact registration until `Drop`
            // removes it, and the observer is retained by `MacosSource` until
            // after that removal.
            unsafe {
                center.addObserver_selector_name_object(
                    &observer,
                    sel!(themeDidChange:),
                    Some(&name),
                    None,
                );
            }
            watchers.push(Watcher::Distributed {
                center: center.clone(),
                name,
            });
        }

        let defaults = NSUserDefaults::standardUserDefaults();
        for key in [
            "AppleAccentColor",
            "AppleAquaColorVariant",
            "AppleInterfaceStyle",
        ] {
            let key = NSString::from_str(key);
            // SAFETY: KVO does not retain the observer, so the registration is
            // only valid while `MacosSource` keeps the observer alive and
            // `Drop` unregisters the key path before releasing it.
            unsafe {
                defaults.addObserver_forKeyPath_options_context(
                    &observer,
                    &key,
                    NSKeyValueObservingOptions::empty(),
                    core::ptr::null_mut(),
                );
            }
            watchers.push(Watcher::Defaults {
                defaults: defaults.clone(),
                key,
            });
        }

        let workspace_center = NSWorkspace::sharedWorkspace().notificationCenter();
        let accessibility_name =
            unsafe { NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification };

        // SAFETY: the workspace center retains the observer for the
        // registration's lifetime, and `Drop` removes the registration.
        unsafe {
            workspace_center.addObserver_selector_name_object(
                &observer,
                sel!(themeDidChange:),
                Some(accessibility_name),
                None,
            );
        }
        watchers.push(Watcher::Workspace {
            center: workspace_center,
            name: accessibility_name,
        });

        self.observer = Some(observer);
        self.watchers = watchers;
        self._signal = Some(signal);
    }

    fn label(&self) -> &'static str {
        SourceKind::Macos.label()
    }
}

impl Drop for MacosSource {
    fn drop(&mut self) {
        let Some(observer) = self.observer.take() else {
            return;
        };
        for watcher in self.watchers.drain(..) {
            match watcher {
                Watcher::Distributed { center, name } => {
                    // SAFETY: the observer is registered with this center
                    // under this name and object.
                    unsafe {
                        center.removeObserver_name_object(&observer, Some(&name), None);
                    }
                }
                Watcher::Defaults { defaults, key } => {
                    // SAFETY: the observer is registered for this key path
                    // with no context.
                    unsafe {
                        defaults.removeObserver_forKeyPath(&observer, &key);
                    }
                }
                Watcher::Workspace { center, name } => {
                    // SAFETY: the observer is registered with this center
                    // under this name and object.
                    unsafe {
                        center.removeObserver_name_object(&observer, Some(name), None);
                    }
                }
            }
        }
    }
}
