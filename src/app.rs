use crate::ui;
use gtk4::prelude::*;
use gtk4::{gdk, CssProvider, STYLE_PROVIDER_PRIORITY_APPLICATION};

const APP_ID: &str = "com.tsp.doot";
const APP_CSS: &str = r#"
window {
    background: #1f1f31;
    color: #e7e5ef;
}

box {
    color: #e7e5ef;
}

button,
combobox,
entry,
searchentry {
    min-height: 38px;
}

/* Improved button styling */
button {
    border-radius: 8px;
    background: alpha(#ffffff, 0.06);
    border: 1px solid alpha(#ffffff, 0.08);
    padding: 6px 12px;
}

button:hover {
    background: alpha(#ffffff, 0.10);
    border-color: alpha(#ffffff, 0.12);
}

button:active {
    background: alpha(#ffffff, 0.14);
}

button image {
    color: #e7e5ef;
    opacity: 0.9;
}

/* Section styling with more padding */
.section-header {
    margin: 8px 0;
    padding: 4px 0;
}

.section-icon {
    color: alpha(#e7e5ef, 0.85);
    margin-right: 8px;
}

.section-title {
    font-weight: 700;
    font-size: 0.95rem;
    letter-spacing: 0.03em;
    color: #f0ecff;
}

/* Content areas with increased padding */
.section-content {
    margin: 8px 0 16px 0;
    padding: 12px 16px;
    background: alpha(#000000, 0.15);
    border-radius: 10px;
}

/* Selection section specific */
.selection-content {
    padding: 16px;
    background: alpha(#000000, 0.12);
    border-radius: 10px;
    margin: 8px 0;
}

.entry-title {
    font-weight: 700;
    font-size: 1.02rem;
    color: #f4f0ff;
}

.overview-row {
    padding: 2px 0;
}

.overview-heading {
    color: alpha(#e7e5ef, 0.62);
    font-size: 0.78rem;
    letter-spacing: 0.06em;
    text-transform: uppercase;
}

.action-button {
    min-height: 40px;
    border-radius: 12px;
    background: alpha(#ffffff, 0.05);
    border: 1px solid alpha(#ffffff, 0.08);
}

.action-button:hover {
    background: alpha(#ffffff, 0.10);
}

.action-primary {
    background: alpha(#6ba8ff, 0.16);
    border-color: alpha(#6ba8ff, 0.24);
    color: #d7e9ff;
}

.action-secondary {
    background: alpha(#ffffff, 0.04);
}

.action-danger {
    background: alpha(#ff9c84, 0.10);
    border-color: alpha(#ff9c84, 0.22);
    color: #ffd8d0;
}

/* Repository info styling */
.repo-info {
    padding: 12px 16px;
    background: alpha(#000000, 0.12);
    border-radius: 10px;
    margin: 8px 0;
    font-family: monospace;
    font-size: 0.9rem;
}

/* Workspace section */
.workspace-content {
    margin: 8px 0;
}

.mode-button {
    min-height: 34px;
    padding: 4px 14px;
}

.mode-button-active {
    background: alpha(#6ba8ff, 0.20);
    border-color: alpha(#6ba8ff, 0.32);
    color: #cfe5ff;
}

.workspace-sidebar {
    min-width: 180px;
}

.notice-banner {
    padding: 10px 14px;
    border-radius: 12px;
    background: alpha(#000000, 0.22);
    border: 1px solid alpha(#ffffff, 0.08);
}

.notice-success {
    background: alpha(#66d6a3, 0.14);
    border-color: alpha(#66d6a3, 0.24);
    color: #d5f5e5;
}

.notice-error {
    background: alpha(#ff9c84, 0.14);
    border-color: alpha(#ff9c84, 0.24);
    color: #ffd8d0;
}

listbox.navigation-sidebar {
    background: transparent;
}

listbox.navigation-sidebar row {
    border-radius: 10px;
    margin: 2px 0;
}

listbox.navigation-sidebar row:selected {
    background: alpha(#6ba8ff, 0.14);
}

textview {
    background: alpha(#000000, 0.16);
    border-radius: 10px;
}

textview.line-numbers {
    background: alpha(#000000, 0.08);
    color: alpha(#e7e5ef, 0.48);
    padding-right: 10px;
    padding-left: 10px;
}

scrolledwindow {
    background: transparent;
}

scrollbar {
    background: transparent;
    border: none;
    min-width: 12px;
    min-height: 12px;
}

scrollbar slider {
    min-width: 9px;
    min-height: 44px;
    margin: 3px;
    border-radius: 999px;
    background: alpha(#d8d4e7, 0.48);
    border: 1px solid alpha(#ffffff, 0.08);
}

scrollbar slider:hover {
    background: alpha(#f0ecff, 0.65);
}

scrollbar slider:active {
    background: alpha(#ffffff, 0.82);
}

scrollbar.vertical slider {
    min-height: 56px;
}

scrollbar.horizontal slider {
    min-width: 56px;
}

scrolledwindow:focus-within scrollbar slider {
    background: alpha(#9ec2ff, 0.72);
    border-color: alpha(#6ba8ff, 0.42);
}

paned > separator {
    min-width: 1px;
    min-height: 1px;
    background: alpha(#ffffff, 0.08);
}

.heading {
    font-weight: 700;
    letter-spacing: 0.04em;
}

.row-icon {
    color: alpha(#e7e5ef, 0.72);
}

.button-content {
    padding: 0 4px;
}

flowboxchild {
    background: transparent;
    border-radius: 14px;
}

flowboxchild > box.grid-card {
    background: transparent;
    border-radius: 14px;
}

flowboxchild:selected > box.grid-card {
    background: alpha(#6ba8ff, 0.10);
}

.grid-card-title {
    font-weight: 500;
    letter-spacing: 0.01em;
}

.scope-badge,
.status-badge {
    padding: 4px 10px;
    border-radius: 999px;
    font-size: 0.78rem;
    font-weight: 700;
    letter-spacing: 0.04em;
}

.scope-home {
    background: alpha(#71b7ff, 0.18);
    color: #b9dbff;
}

.scope-config {
    background: alpha(#76d7a5, 0.18);
    color: #bcedd2;
}

.status-active {
    background: alpha(#66d6a3, 0.16);
    color: #baf0d5;
}

.status-conflicted {
    background: alpha(#ff9c84, 0.16);
    color: #ffd1c6;
}

.status-inactive {
    background: alpha(#cbb8ff, 0.14);
    color: #e1d8ff;
}

.status-unmanaged {
    background: alpha(#ffffff, 0.08);
    color: #d8d4e7;
}
"#;

pub fn run() {
    configure_graphics_backend();
    let app = gtk4::Application::builder().application_id(APP_ID).build();
    app.connect_activate(|app| {
        install_css();
        ui::build(app);
    });
    app.run();
}

fn configure_graphics_backend() {
    let on_wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let renderer_forced = std::env::var_os("GSK_RENDERER").is_some();

    if on_wayland && !renderer_forced {
        // On some Wayland sessions GTK 4.22 picks the Vulkan renderer by default,
        // which can drop the compositor connection during swapchain recreation.
        // Prefer the OpenGL renderer unless the user explicitly overrides it.
        unsafe {
            std::env::set_var("GSK_RENDERER", "gl");
        }
    }
}

fn install_css() {
    let provider = CssProvider::new();
    provider.load_from_data(APP_CSS);

    if let Some(display) = gdk::Display::default() {
        gtk4::style_context_add_provider_for_display(
            &display,
            &provider,
            STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}
