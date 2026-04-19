use crate::git;
use crate::model::{DotfileEntry, EntryKind, GitRepoState, ManagedState, ScanReport};
use crate::operations;
use crate::scanner;
use crate::state::{AppPaths, PersistedState};
use anyhow::anyhow;
use gio::prelude::*;
use glib::translate::IntoGlib;
use glib::ControlFlow;
use gtk4::prelude::*;
use gtk4::{
    gdk, pango, Align, Application, ApplicationWindow, Box as GtkBox, Button, ComboBoxText, Dialog,
    Entry, FileChooserAction, FileChooserNative, FlowBox, FlowBoxChild, Grid, Image, Label,
    ListBox, ListBoxRow, Orientation, Paned, PolicyType, ResponseType, Revealer, ScrolledWindow,
    SearchEntry, SelectionMode, Stack, StackSwitcher, TextTag, TextView,
};
use similar::{ChangeTag, TextDiff};
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::fs;
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

#[derive(Clone)]
struct Widgets {
    window: ApplicationWindow,
    search_entry: SearchEntry,
    profile_combo: ComboBoxText,
    scope_filter_combo: ComboBoxText,
    kind_filter_combo: ComboBoxText,
    sort_combo: ComboBoxText,
    new_profile_button: Button,
    copy_profile_button: Button,
    remove_profile_button: Button,
    open_repo_root_button: Button,
    settings_button: Button,
    stage_all_button: Button,
    sync_button: Button,
    notice_revealer: Revealer,
    notice_label: Label,
    list_box: ListBox,
    grid_box: FlowBox,
    summary_label: Label,
    overview_section: GtkBox,
    entry_title_label: Label,
    path_value_label: Label,
    repo_value_label: Label,
    branch_value_label: Label,
    remote_value_label: Label,
    workspace_path_label: Label,
    workspace_stack: Stack,
    toggle_workspace_button: Button,
    show_editor_button: Button,
    show_diff_button: Button,
    repo_editor_status: Label,
    repo_files_list: ListBox,
    repo_editor_line_numbers: TextView,
    repo_editor_view: TextView,
    ignore_repo_button: Button,
    save_repo_button: Button,
    reload_repo_button: Button,
    diff_status_label: Label,
    diff_base_title: Label,
    diff_current_title: Label,
    diff_base_view: TextView,
    diff_current_view: TextView,
    enable_button: Button,
    disable_button: Button,
    stage_button: Button,
    auto_commit_button: Button,
    commit_button: Button,
    push_button: Button,
    open_live_button: Button,
    open_repo_button: Button,
    reveal_button: Button,
    refresh_button: Button,
}

struct AppRuntime {
    widgets: Widgets,
    paths: AppPaths,
    persisted: PersistedState,
    report: ScanReport,
    git_state: GitRepoState,
    filter_text: String,
    scope_filter: ScopeFilter,
    kind_filter: KindFilter,
    sort_mode: SortMode,
    selected_entry_id: Option<String>,
    browser_selection_updating: bool,
    repo_root_path: Option<PathBuf>,
    repo_files: Vec<PathBuf>,
    repo_rows: Vec<RepoExplorerRow>,
    expanded_repo_dirs: BTreeSet<PathBuf>,
    active_repo_row_path: Option<PathBuf>,
    selected_repo_file: Option<PathBuf>,
    loaded_repo_content: Option<String>,
    repo_explorer_updating: bool,
    sync_in_progress: bool,
}

#[derive(Clone)]
struct RepoExplorerRow {
    path: PathBuf,
    label: String,
    depth: usize,
    is_dir: bool,
    expanded: bool,
}

enum RepoSelectionAction {
    None,
    RefreshTree,
    LoadFile,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ScopeFilter {
    All,
    Home,
    Config,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum KindFilter {
    All,
    Files,
    Directories,
    Symlinks,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SortMode {
    NameAsc,
    NameDesc,
    Scope,
    Status,
}

const EDITOR_LINE_NUMBER_LIMIT: usize = 25_000;
const EDITOR_EDIT_BYTE_LIMIT: usize = 700_000;
const EDITOR_EDIT_LINE_LIMIT: usize = 20_000;
const EDITOR_PREVIEW_CHAR_LIMIT: usize = 180_000;
const EDITOR_PREVIEW_LINE_LIMIT: usize = 2_500;
const EDITOR_HIGHLIGHT_BYTE_LIMIT: usize = 400_000;
const EDITOR_HIGHLIGHT_LINE_LIMIT: usize = 8_000;
const DIFF_BYTE_LIMIT: usize = 350_000;
const DIFF_LINE_LIMIT: usize = 12_000;
const DIFF_PREVIEW_CHAR_LIMIT: usize = 140_000;
const DIFF_PREVIEW_LINE_LIMIT: usize = 2_000;
const DIFF_RENDER_LINE_LIMIT: usize = 3_500;
const MAIN_LEFT_PANEL_MIN_WIDTH: i32 = 360;
const MAIN_RIGHT_PANEL_MIN_WIDTH: i32 = 560;
const WORKSPACE_SIDEBAR_MIN_WIDTH: i32 = 220;
const WORKSPACE_EDITOR_MIN_WIDTH: i32 = 360;
const DIFF_PANEL_MIN_WIDTH: i32 = 280;

pub fn build(app: &Application) {
    let paths = match AppPaths::discover() {
        Ok(paths) => paths,
        Err(error) => {
            eprintln!("{}", error);
            return;
        }
    };

    let persisted = PersistedState::load(&paths).unwrap_or_default();
    let window = ApplicationWindow::builder()
        .application(app)
        .title("Doter")
        .default_width(1320)
        .default_height(840)
        .build();

    let widgets = build_widgets(&window);
    let runtime = Rc::new(RefCell::new(AppRuntime {
        widgets: widgets.clone(),
        paths,
        persisted,
        report: ScanReport::default(),
        git_state: GitRepoState::default(),
        filter_text: String::new(),
        scope_filter: ScopeFilter::All,
        kind_filter: KindFilter::All,
        sort_mode: SortMode::NameAsc,
        selected_entry_id: None,
        browser_selection_updating: false,
        repo_root_path: None,
        repo_files: Vec::new(),
        repo_rows: Vec::new(),
        expanded_repo_dirs: BTreeSet::new(),
        active_repo_row_path: None,
        selected_repo_file: None,
        loaded_repo_content: None,
        repo_explorer_updating: false,
        sync_in_progress: false,
    }));

    install_handlers(runtime.clone());
    install_background_tasks(runtime.clone());
    window.present();
    ensure_repo_then_refresh(runtime);
}

fn build_widgets(window: &ApplicationWindow) -> Widgets {
    let root = GtkBox::new(Orientation::Vertical, 12);
    root.set_margin_top(12);
    root.set_margin_bottom(12);
    root.set_margin_start(12);
    root.set_margin_end(12);

    let notice_label = Label::new(None);
    notice_label.set_xalign(0.0);
    notice_label.set_wrap(true);
    notice_label.set_wrap_mode(pango::WrapMode::WordChar);
    notice_label.add_css_class("notice-banner");
    let notice_revealer = Revealer::new();
    notice_revealer.set_reveal_child(false);
    notice_revealer.set_child(Some(&notice_label));

    let content = Paned::new(Orientation::Horizontal);
    content.set_wide_handle(true);
    content.set_position(480);
    content.set_hexpand(true);
    content.set_vexpand(true);
    content.set_shrink_start_child(false);
    content.set_shrink_end_child(false);

    let left_panel = GtkBox::new(Orientation::Vertical, 10);
    left_panel.set_hexpand(true);
    left_panel.set_vexpand(true);
    left_panel.set_width_request(MAIN_LEFT_PANEL_MIN_WIDTH);

    let dotfiles_header = GtkBox::new(Orientation::Horizontal, 8);
    let left_heading = section_heading("Dotfiles", "folder-symbolic");
    left_heading.set_hexpand(true);
    let view_switcher = StackSwitcher::new();

    let search_entry = SearchEntry::builder()
        .placeholder_text("Search dotfiles")
        .hexpand(true)
        .build();
    let profile_combo = ComboBoxText::new();
    let refresh_button = icon_button("view-refresh-symbolic", "Refresh");
    let new_profile_button = icon_button("list-add-symbolic", "New");
    let copy_profile_button = icon_button("edit-copy-symbolic", "Copy");
    let remove_profile_button = icon_button("user-trash-symbolic", "Remove");
    let open_repo_root_button = icon_button("folder-open-symbolic", "Open Repo");
    let settings_button = icon_button("emblem-system-symbolic", "Settings");
    let filter_grid = Grid::new();
    filter_grid.set_row_spacing(8);
    filter_grid.set_column_spacing(8);
    filter_grid.attach(&search_entry, 0, 0, 4, 1);
    filter_grid.attach(&profile_combo, 0, 1, 2, 1);
    filter_grid.attach(&refresh_button, 2, 1, 1, 1);
    filter_grid.attach(&new_profile_button, 3, 1, 1, 1);
    filter_grid.attach(&copy_profile_button, 0, 2, 1, 1);
    filter_grid.attach(&remove_profile_button, 1, 2, 1, 1);
    filter_grid.attach(&open_repo_root_button, 2, 2, 1, 1);
    filter_grid.attach(&settings_button, 3, 2, 1, 1);

    let filter_controls = Grid::new();
    filter_controls.set_row_spacing(8);
    filter_controls.set_column_spacing(8);
    let scope_label = Label::new(Some("Scope:"));
    scope_label.add_css_class("dim-label");
    let scope_filter_combo = ComboBoxText::new();
    scope_filter_combo.append(Some("all"), "All");
    scope_filter_combo.append(Some("home"), "Home");
    scope_filter_combo.append(Some("config"), "Config");
    scope_filter_combo.set_active_id(Some("all"));
    let kind_label = Label::new(Some("Type:"));
    kind_label.add_css_class("dim-label");
    let kind_filter_combo = ComboBoxText::new();
    kind_filter_combo.append(Some("all"), "All");
    kind_filter_combo.append(Some("directories"), "Dirs");
    kind_filter_combo.append(Some("files"), "Files");
    kind_filter_combo.append(Some("symlinks"), "Links");
    kind_filter_combo.set_active_id(Some("all"));
    let sort_label = Label::new(Some("Sort:"));
    sort_label.add_css_class("dim-label");
    let sort_combo = ComboBoxText::new();
    sort_combo.append(Some("name_asc"), "A-Z");
    sort_combo.append(Some("name_desc"), "Z-A");
    sort_combo.append(Some("scope"), "Scope");
    sort_combo.append(Some("status"), "Status");
    sort_combo.set_active_id(Some("name_asc"));
    filter_controls.attach(&scope_label, 0, 0, 1, 1);
    filter_controls.attach(&scope_filter_combo, 1, 0, 1, 1);
    filter_controls.attach(&kind_label, 2, 0, 1, 1);
    filter_controls.attach(&kind_filter_combo, 3, 0, 1, 1);
    filter_controls.attach(&sort_label, 0, 1, 1, 1);
    filter_controls.attach(&sort_combo, 1, 1, 1, 1);

    let list_box = ListBox::new();
    list_box.set_selection_mode(SelectionMode::Single);
    let grid_box = FlowBox::new();
    grid_box.set_selection_mode(SelectionMode::Single);
    grid_box.set_activate_on_single_click(true);
    grid_box.set_homogeneous(false);
    grid_box.set_max_children_per_line(4);
    grid_box.set_min_children_per_line(1);
    grid_box.set_row_spacing(12);
    grid_box.set_column_spacing(12);
    let left_scroll = ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(true)
        .child(&list_box)
        .build();
    let grid_scroll = ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(true)
        .child(&grid_box)
        .build();
    let entries_stack = Stack::new();
    entries_stack.set_hexpand(true);
    entries_stack.set_vexpand(true);
    entries_stack.add_titled(&left_scroll, Some("list"), "List");
    entries_stack.add_titled(&grid_scroll, Some("grid"), "Grid");
    view_switcher.set_stack(Some(&entries_stack));
    dotfiles_header.append(&left_heading);
    dotfiles_header.append(&view_switcher);

    let summary_label = Label::new(None);
    summary_label.set_xalign(0.0);
    summary_label.set_wrap(true);
    summary_label.set_wrap_mode(pango::WrapMode::WordChar);
    summary_label.add_css_class("dim-label");

    left_panel.append(&dotfiles_header);
    left_panel.append(&filter_grid);
    left_panel.append(&filter_controls);
    left_panel.append(&summary_label);
    left_panel.append(&entries_stack);

    let right_panel = GtkBox::new(Orientation::Vertical, 16);
    right_panel.set_margin_top(8);
    right_panel.set_margin_bottom(8);
    right_panel.set_margin_start(12);
    right_panel.set_margin_end(8);
    right_panel.set_hexpand(true);
    right_panel.set_vexpand(true);
    right_panel.set_width_request(MAIN_RIGHT_PANEL_MIN_WIDTH);

    let overview_section = GtkBox::new(Orientation::Vertical, 14);
    let overview_card = GtkBox::new(Orientation::Vertical, 10);
    overview_card.add_css_class("selection-content");
    let entry_title_label = Label::new(None);
    entry_title_label.set_xalign(0.0);
    entry_title_label.set_wrap(true);
    entry_title_label.set_wrap_mode(pango::WrapMode::WordChar);
    entry_title_label.set_selectable(true);
    entry_title_label.add_css_class("entry-title");
    let path_value_label = overview_info_row(&overview_card, "folder-symbolic", "Path");
    let repo_value_label = overview_info_row(&overview_card, "folder-git-symbolic", "Repo");
    let branch_value_label = overview_info_row(&overview_card, "view-list-symbolic", "Branch");
    let remote_value_label = overview_info_row(&overview_card, "network-server-symbolic", "Remote");
    overview_card.prepend(&entry_title_label);

    let actions_content = Grid::new();
    actions_content.add_css_class("section-content");
    let enable_button = icon_button("emblem-ok-symbolic", "Enable");
    let disable_button = icon_button("action-unavailable-symbolic", "Disable");
    let open_live_button = icon_button("folder-open-symbolic", "Open Live");
    let open_repo_button = icon_button("text-x-generic-symbolic", "Open Repo");
    let reveal_button = icon_button("folder-visiting-symbolic", "Reveal");
    let stage_button = icon_button("list-add-symbolic", "Stage");
    let auto_commit_button = icon_button("document-send-symbolic", "Auto");
    let stage_all_button = icon_button("view-list-symbolic", "Stage All");
    let sync_button = icon_button("refresh-active-symbolic", "Sync");
    let commit_button = icon_button("document-save-symbolic", "Commit");
    let push_button = icon_button("send-to-symbolic", "Push");
    for button in [
        &enable_button,
        &disable_button,
        &open_live_button,
        &open_repo_button,
        &reveal_button,
        &stage_button,
        &auto_commit_button,
        &stage_all_button,
        &sync_button,
        &commit_button,
        &push_button,
    ] {
        button.set_hexpand(true);
    }
    enable_button.add_css_class("action-primary");
    sync_button.add_css_class("action-primary");
    push_button.add_css_class("action-primary");
    stage_button.add_css_class("action-secondary");
    commit_button.add_css_class("action-secondary");
    stage_all_button.add_css_class("action-secondary");
    disable_button.add_css_class("action-danger");
    actions_content.set_row_spacing(10);
    actions_content.set_column_spacing(10);
    actions_content.attach(&enable_button, 0, 0, 1, 1);
    actions_content.attach(&disable_button, 1, 0, 1, 1);
    actions_content.attach(&open_live_button, 2, 0, 1, 1);
    actions_content.attach(&open_repo_button, 3, 0, 1, 1);
    actions_content.attach(&reveal_button, 4, 0, 1, 1);
    actions_content.attach(&stage_button, 0, 1, 1, 1);
    actions_content.attach(&auto_commit_button, 1, 1, 1, 1);
    actions_content.attach(&commit_button, 2, 1, 1, 1);
    actions_content.attach(&push_button, 3, 1, 1, 1);
    actions_content.attach(&sync_button, 4, 1, 1, 1);
    actions_content.attach(&stage_all_button, 0, 2, 2, 1);
    overview_section.append(&overview_card);
    overview_section.append(&actions_content);

    let workspace_section = GtkBox::new(Orientation::Vertical, 10);
    workspace_section.set_vexpand(true);
    let workspace_header = GtkBox::new(Orientation::Horizontal, 10);
    let workspace_title = section_heading("Workspace", "text-editor-symbolic");
    workspace_title.set_hexpand(true);
    let workspace_mode = GtkBox::new(Orientation::Horizontal, 8);
    let toggle_workspace_button = Button::with_label("Focus");
    toggle_workspace_button.add_css_class("mode-button");
    toggle_workspace_button.add_css_class("action-secondary");
    let show_editor_button = Button::with_label("Editor");
    let show_diff_button = Button::with_label("Diff");
    show_editor_button.add_css_class("mode-button");
    show_diff_button.add_css_class("mode-button");
    show_editor_button.add_css_class("mode-button-active");
    workspace_mode.append(&toggle_workspace_button);
    workspace_mode.append(&show_editor_button);
    workspace_mode.append(&show_diff_button);
    workspace_header.append(&workspace_title);
    workspace_header.append(&workspace_mode);

    let workspace_path_label = Label::new(Some(
        "Select a managed entry to browse repo files or open a diff.",
    ));
    workspace_path_label.set_xalign(0.0);
    workspace_path_label.set_wrap(true);
    workspace_path_label.set_wrap_mode(pango::WrapMode::WordChar);
    workspace_path_label.add_css_class("dim-label");

    let explorer_shell = GtkBox::new(Orientation::Vertical, 8);
    explorer_shell.add_css_class("section-content");
    explorer_shell.add_css_class("workspace-sidebar");
    explorer_shell.set_width_request(WORKSPACE_SIDEBAR_MIN_WIDTH);
    let explorer_label = Label::new(Some("Explorer"));
    explorer_label.set_xalign(0.0);
    explorer_label.add_css_class("heading");
    explorer_label.add_css_class("section-title");

    let repo_files_list = ListBox::new();
    repo_files_list.set_selection_mode(SelectionMode::Single);
    repo_files_list.set_activate_on_single_click(true);
    repo_files_list.add_css_class("navigation-sidebar");
    let repo_files_scroll = ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(true)
        .min_content_width(180)
        .child(&repo_files_list)
        .build();
    explorer_shell.append(&explorer_label);
    explorer_shell.append(&repo_files_scroll);

    let repo_editor_header = GtkBox::new(Orientation::Vertical, 8);
    repo_editor_header.add_css_class("section-content");
    let repo_editor_status = Label::new(Some("Select a managed entry to edit its repo files."));
    repo_editor_status.set_xalign(0.0);
    repo_editor_status.set_wrap(true);
    repo_editor_status.set_wrap_mode(pango::WrapMode::WordChar);
    repo_editor_status.set_selectable(true);
    repo_editor_status.set_hexpand(true);
    let ignore_repo_button = icon_button("text-x-generic-symbolic", "Ignore");
    let reload_repo_button = icon_button("view-refresh-symbolic", "Reload");
    let save_repo_button = icon_button("document-save-symbolic", "Save");
    let repo_editor_actions = GtkBox::new(Orientation::Horizontal, 8);
    repo_editor_actions.set_halign(Align::Start);
    repo_editor_actions.append(&ignore_repo_button);
    repo_editor_actions.append(&reload_repo_button);
    repo_editor_actions.append(&save_repo_button);
    repo_editor_header.append(&repo_editor_status);
    repo_editor_header.append(&repo_editor_actions);

    let repo_editor_view = TextView::new();
    repo_editor_view.set_editable(true);
    repo_editor_view.set_monospace(true);
    repo_editor_view.set_wrap_mode(gtk4::WrapMode::None);
    repo_editor_view.set_vexpand(true);
    repo_editor_view.set_size_request(-1, 0);
    let repo_editor_line_numbers = TextView::new();
    repo_editor_line_numbers.set_editable(false);
    repo_editor_line_numbers.set_cursor_visible(false);
    repo_editor_line_numbers.set_monospace(true);
    repo_editor_line_numbers.set_wrap_mode(gtk4::WrapMode::None);
    repo_editor_line_numbers.set_vexpand(true);
    repo_editor_line_numbers.set_width_request(48);
    repo_editor_line_numbers.set_size_request(48, 0);
    repo_editor_line_numbers.add_css_class("line-numbers");
    let line_numbers_scroll = ScrolledWindow::builder()
        .hscrollbar_policy(PolicyType::Never)
        .vscrollbar_policy(PolicyType::Never)
        .vexpand(true)
        .min_content_width(48)
        .child(&repo_editor_line_numbers)
        .build();
    line_numbers_scroll.set_propagate_natural_height(false);
    line_numbers_scroll.set_min_content_height(0);
    let repo_editor_scroll = ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(true)
        .hscrollbar_policy(PolicyType::Automatic)
        .vscrollbar_policy(PolicyType::Automatic)
        .child(&repo_editor_view)
        .build();
    repo_editor_scroll.set_overlay_scrolling(false);
    repo_editor_scroll.set_propagate_natural_height(false);
    repo_editor_scroll.set_min_content_height(0);
    line_numbers_scroll.set_vadjustment(Some(&repo_editor_scroll.vadjustment()));
    let editor_body = GtkBox::new(Orientation::Horizontal, 0);
    editor_body.set_hexpand(true);
    editor_body.set_vexpand(true);
    editor_body.append(&line_numbers_scroll);
    editor_body.append(&repo_editor_scroll);
    let editor_page = GtkBox::new(Orientation::Vertical, 6);
    editor_page.set_hexpand(true);
    editor_page.set_vexpand(true);
    editor_page.set_width_request(WORKSPACE_EDITOR_MIN_WIDTH);
    editor_page.append(&repo_editor_header);
    editor_page.append(&editor_body);

    let diff_status_label = Label::new(Some(
        "Compare the selected repo file against the latest committed version.",
    ));
    diff_status_label.set_xalign(0.0);
    diff_status_label.set_wrap(true);
    diff_status_label.set_wrap_mode(pango::WrapMode::WordChar);
    diff_status_label.set_selectable(true);
    diff_status_label.add_css_class("section-content");
    let diff_base_view = TextView::new();
    diff_base_view.set_editable(false);
    diff_base_view.set_cursor_visible(false);
    diff_base_view.set_monospace(true);
    diff_base_view.set_wrap_mode(gtk4::WrapMode::None);
    diff_base_view.set_vexpand(true);
    diff_base_view.set_size_request(-1, 0);
    let diff_current_view = TextView::new();
    diff_current_view.set_editable(false);
    diff_current_view.set_cursor_visible(false);
    diff_current_view.set_monospace(true);
    diff_current_view.set_wrap_mode(gtk4::WrapMode::None);
    diff_current_view.set_vexpand(true);
    diff_current_view.set_size_request(-1, 0);
    let diff_base_title = Label::new(Some("HEAD"));
    diff_base_title.set_xalign(0.0);
    diff_base_title.add_css_class("heading");
    let diff_current_title = Label::new(Some("Working Tree"));
    diff_current_title.set_xalign(0.0);
    diff_current_title.add_css_class("heading");
    let diff_base_scroll = ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(true)
        .hscrollbar_policy(PolicyType::Automatic)
        .vscrollbar_policy(PolicyType::Automatic)
        .child(&diff_base_view)
        .build();
    diff_base_scroll.set_overlay_scrolling(false);
    diff_base_scroll.set_propagate_natural_height(false);
    diff_base_scroll.set_min_content_height(0);
    let diff_current_scroll = ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(true)
        .hscrollbar_policy(PolicyType::Automatic)
        .vscrollbar_policy(PolicyType::Automatic)
        .child(&diff_current_view)
        .build();
    diff_current_scroll.set_overlay_scrolling(false);
    diff_current_scroll.set_propagate_natural_height(false);
    diff_current_scroll.set_min_content_height(0);
    let diff_base_panel = GtkBox::new(Orientation::Vertical, 6);
    diff_base_panel.add_css_class("section-content");
    diff_base_panel.set_hexpand(true);
    diff_base_panel.set_vexpand(true);
    diff_base_panel.set_width_request(DIFF_PANEL_MIN_WIDTH);
    diff_base_panel.append(&diff_base_title);
    diff_base_panel.append(&diff_base_scroll);
    let diff_current_panel = GtkBox::new(Orientation::Vertical, 6);
    diff_current_panel.add_css_class("section-content");
    diff_current_panel.set_hexpand(true);
    diff_current_panel.set_vexpand(true);
    diff_current_panel.set_width_request(DIFF_PANEL_MIN_WIDTH);
    diff_current_panel.append(&diff_current_title);
    diff_current_panel.append(&diff_current_scroll);
    let diff_split = Paned::new(Orientation::Horizontal);
    diff_split.set_wide_handle(true);
    diff_split.set_position(420);
    diff_split.set_hexpand(true);
    diff_split.set_vexpand(true);
    diff_split.set_shrink_start_child(false);
    diff_split.set_shrink_end_child(false);
    diff_split.set_start_child(Some(&diff_base_panel));
    diff_split.set_end_child(Some(&diff_current_panel));
    let diff_page = GtkBox::new(Orientation::Vertical, 6);
    diff_page.set_hexpand(true);
    diff_page.set_vexpand(true);
    diff_page.append(&diff_status_label);
    diff_page.append(&diff_split);

    let workspace_stack = Stack::new();
    workspace_stack.set_hexpand(true);
    workspace_stack.set_vexpand(true);
    workspace_stack.add_titled(&editor_page, Some("editor"), "Editor");
    workspace_stack.add_titled(&diff_page, Some("diff"), "Diff");
    let workspace_split = Paned::new(Orientation::Horizontal);
    workspace_split.set_wide_handle(true);
    workspace_split.set_position(250);
    workspace_split.set_hexpand(true);
    workspace_split.set_vexpand(true);
    workspace_split.set_shrink_start_child(false);
    workspace_split.set_shrink_end_child(false);
    workspace_split.set_start_child(Some(&explorer_shell));
    workspace_split.set_end_child(Some(&workspace_stack));

    workspace_section.append(&workspace_header);
    workspace_section.append(&workspace_path_label);
    workspace_section.append(&workspace_split);

    let right_scroll = ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(true)
        .hscrollbar_policy(PolicyType::Never)
        .vscrollbar_policy(PolicyType::Automatic)
        .child(&right_panel)
        .build();
    right_scroll.set_overlay_scrolling(false);
    right_scroll.set_propagate_natural_height(false);
    right_scroll.set_min_content_height(0);

    right_panel.append(&overview_section);
    right_panel.append(&workspace_section);

    content.set_start_child(Some(&left_panel));
    content.set_end_child(Some(&right_scroll));

    root.append(&notice_revealer);
    root.append(&content);
    window.set_child(Some(&root));

    Widgets {
        window: window.clone(),
        search_entry,
        profile_combo,
        scope_filter_combo,
        kind_filter_combo,
        sort_combo,
        new_profile_button,
        copy_profile_button,
        remove_profile_button,
        open_repo_root_button,
        stage_all_button,
        sync_button,
        notice_revealer,
        notice_label,
        list_box,
        grid_box,
        summary_label,
        overview_section,
        entry_title_label,
        path_value_label,
        repo_value_label,
        branch_value_label,
        remote_value_label,
        workspace_path_label,
        workspace_stack,
        toggle_workspace_button,
        show_editor_button,
        show_diff_button,
        repo_editor_status,
        repo_files_list,
        repo_editor_line_numbers,
        repo_editor_view,
        ignore_repo_button,
        save_repo_button,
        reload_repo_button,
        diff_status_label,
        diff_base_title,
        diff_current_title,
        diff_base_view,
        diff_current_view,
        enable_button,
        disable_button,
        stage_button,
        auto_commit_button,
        commit_button,
        push_button,
        settings_button,
        open_live_button,
        open_repo_button,
        reveal_button,
        refresh_button,
    }
}

fn section_heading(text: &str, icon_name: &str) -> GtkBox {
    let row = GtkBox::new(Orientation::Horizontal, 10);
    row.set_margin_top(4);
    row.set_margin_bottom(4);
    let icon = Image::from_icon_name(icon_name);
    icon.set_pixel_size(18);
    icon.add_css_class("section-icon");
    let label = Label::new(Some(text));
    label.set_xalign(0.0);
    label.add_css_class("heading");
    label.add_css_class("section-title");
    row.append(&icon);
    row.append(&label);
    row
}

fn overview_info_row(container: &GtkBox, icon_name: &str, heading: &str) -> Label {
    let row = GtkBox::new(Orientation::Horizontal, 10);
    row.add_css_class("overview-row");
    let icon = Image::from_icon_name(icon_name);
    icon.set_pixel_size(16);
    icon.add_css_class("section-icon");
    let content = GtkBox::new(Orientation::Vertical, 2);
    let heading_label = Label::new(Some(heading));
    heading_label.set_xalign(0.0);
    heading_label.add_css_class("overview-heading");
    let value_label = Label::new(None);
    value_label.set_xalign(0.0);
    value_label.set_wrap(true);
    value_label.set_wrap_mode(pango::WrapMode::WordChar);
    value_label.set_selectable(true);
    content.append(&heading_label);
    content.append(&value_label);
    row.append(&icon);
    row.append(&content);
    container.append(&row);
    value_label
}

fn icon_button(icon_name: &str, label: &str) -> Button {
    let button = Button::new();
    button.add_css_class("action-button");
    let content = GtkBox::new(Orientation::Horizontal, 8);
    content.set_margin_start(4);
    content.set_margin_end(4);
    content.add_css_class("button-content");
    let icon = Image::from_icon_name(icon_name);
    icon.set_pixel_size(16);
    icon.set_margin_end(2);
    let text = Label::new(Some(label));
    text.set_margin_start(2);
    content.append(&icon);
    content.append(&text);
    button.set_child(Some(&content));
    button
}

fn install_handlers(runtime: Rc<RefCell<AppRuntime>>) {
    {
        let runtime = runtime.clone();
        let search_entry = runtime.borrow().widgets.search_entry.clone();
        search_entry.connect_search_changed(move |entry| {
            let text = entry.text().to_string();
            runtime.borrow_mut().filter_text = text;
            render_list(&runtime);
        });
    }

    {
        let runtime = runtime.clone();
        let list_box = runtime.borrow().widgets.list_box.clone();
        list_box.connect_row_selected(move |_, row| {
            if runtime.borrow().browser_selection_updating {
                return;
            }
            let runtime2 = runtime.clone();
            set_selected_entry_from_row(&runtime, row);
            update_details(&runtime2);
        });
    }

    {
        let runtime = runtime.clone();
        let grid_box = runtime.borrow().widgets.grid_box.clone();
        grid_box.connect_selected_children_changed(move |grid| {
            if runtime.borrow().browser_selection_updating {
                return;
            }
            let selected: Vec<_> = grid.selected_children().into_iter().collect();
            if selected.is_empty() {
                return;
            }
            let runtime2 = runtime.clone();
            set_selected_entry_from_grid(&runtime, selected.first());
            update_details(&runtime2);
        });
    }

    {
        let runtime = runtime.clone();
        let repo_files_list = runtime.borrow().widgets.repo_files_list.clone();
        repo_files_list.connect_row_selected(move |_, row| {
            let runtime2 = runtime.clone();
            match set_selected_repo_file_from_row(&runtime, row) {
                RepoSelectionAction::None => {}
                RepoSelectionAction::RefreshTree => render_repo_explorer(&runtime2),
                RepoSelectionAction::LoadFile => refresh_active_workspace_view(&runtime2),
            }
        });
    }

    {
        let runtime = runtime.clone();
        let repo_files_list = runtime.borrow().widgets.repo_files_list.clone();
        repo_files_list.connect_row_activated(move |_, row| {
            let runtime2 = runtime.clone();
            match activate_repo_row(&runtime, row) {
                RepoSelectionAction::None => {}
                RepoSelectionAction::RefreshTree => render_repo_explorer(&runtime2),
                RepoSelectionAction::LoadFile => refresh_active_workspace_view(&runtime2),
            }
        });
    }

    {
        let runtime = runtime.clone();
        let profile_combo = runtime.borrow().widgets.profile_combo.clone();
        profile_combo.connect_changed(move |combo| {
            let Some(profile) = combo.active_text() else {
                return;
            };
            let profile = profile.to_string();
            {
                let mut guard = runtime.borrow_mut();
                if guard.persisted.config.active_profile == profile {
                    return;
                }
                guard.persisted.config.active_profile = profile;
                guard.persisted.config.ensure_active_profile();
                let _ = guard.persisted.save(&guard.paths);
            }
            refresh(runtime.clone());
        });
    }

    {
        let runtime = runtime.clone();
        let scope_filter_combo = runtime.borrow().widgets.scope_filter_combo.clone();
        scope_filter_combo.connect_changed(move |combo| {
            let mut guard = runtime.borrow_mut();
            guard.scope_filter = match combo.active_id().as_deref() {
                Some("home") => ScopeFilter::Home,
                Some("config") => ScopeFilter::Config,
                _ => ScopeFilter::All,
            };
            drop(guard);
            render_list(&runtime);
            update_details(&runtime);
        });
    }

    {
        let runtime = runtime.clone();
        let kind_filter_combo = runtime.borrow().widgets.kind_filter_combo.clone();
        kind_filter_combo.connect_changed(move |combo| {
            let mut guard = runtime.borrow_mut();
            guard.kind_filter = match combo.active_id().as_deref() {
                Some("directories") => KindFilter::Directories,
                Some("files") => KindFilter::Files,
                Some("symlinks") => KindFilter::Symlinks,
                _ => KindFilter::All,
            };
            drop(guard);
            render_list(&runtime);
            update_details(&runtime);
        });
    }

    {
        let runtime = runtime.clone();
        let sort_combo = runtime.borrow().widgets.sort_combo.clone();
        sort_combo.connect_changed(move |combo| {
            let mut guard = runtime.borrow_mut();
            guard.sort_mode = match combo.active_id().as_deref() {
                Some("name_desc") => SortMode::NameDesc,
                Some("scope") => SortMode::Scope,
                Some("status") => SortMode::Status,
                _ => SortMode::NameAsc,
            };
            drop(guard);
            render_list(&runtime);
            update_details(&runtime);
        });
    }

    bind_button(
        runtime.clone(),
        |runtime| {
            prompt_new_profile(runtime);
        },
        |widgets| widgets.new_profile_button.clone(),
    );

    bind_button(
        runtime.clone(),
        |runtime| {
            prompt_copy_profile(runtime);
        },
        |widgets| widgets.copy_profile_button.clone(),
    );

    bind_button(
        runtime.clone(),
        |runtime| {
            remove_active_profile(runtime);
        },
        |widgets| widgets.remove_profile_button.clone(),
    );

    bind_button(
        runtime.clone(),
        |runtime| {
            open_repo_root(runtime);
        },
        |widgets| widgets.open_repo_root_button.clone(),
    );

    bind_button(
        runtime.clone(),
        |runtime| {
            refresh(runtime);
        },
        |widgets| widgets.refresh_button.clone(),
    );

    bind_button(
        runtime.clone(),
        |runtime| {
            stage_all(runtime);
        },
        |widgets| widgets.stage_all_button.clone(),
    );

    bind_button(
        runtime.clone(),
        |runtime| {
            enable_selected(runtime);
        },
        |widgets| widgets.enable_button.clone(),
    );

    bind_button(
        runtime.clone(),
        |runtime| {
            disable_selected(runtime);
        },
        |widgets| widgets.disable_button.clone(),
    );

    bind_button(
        runtime.clone(),
        |runtime| {
            stage_selected(runtime);
        },
        |widgets| widgets.stage_button.clone(),
    );

    bind_button(
        runtime.clone(),
        |runtime| {
            prompt_commit(runtime);
        },
        |widgets| widgets.commit_button.clone(),
    );

    bind_button(
        runtime.clone(),
        |runtime| {
            auto_commit_selected(runtime);
        },
        |widgets| widgets.auto_commit_button.clone(),
    );

    bind_button(
        runtime.clone(),
        |runtime| {
            push_current_branch(runtime);
        },
        |widgets| widgets.push_button.clone(),
    );

    bind_button(
        runtime.clone(),
        |runtime| {
            prompt_settings(runtime, false);
        },
        |widgets| widgets.settings_button.clone(),
    );

    bind_button(
        runtime.clone(),
        |runtime| {
            open_selected_live(runtime);
        },
        |widgets| widgets.open_live_button.clone(),
    );

    bind_button(
        runtime.clone(),
        |runtime| {
            open_selected_repo(runtime);
        },
        |widgets| widgets.open_repo_button.clone(),
    );

    bind_button(
        runtime.clone(),
        |runtime| {
            ignore_selected_repo_path(runtime);
        },
        |widgets| widgets.ignore_repo_button.clone(),
    );

    bind_button(
        runtime.clone(),
        |runtime| {
            reload_repo_editor_action(runtime);
        },
        |widgets| widgets.reload_repo_button.clone(),
    );

    bind_button(
        runtime.clone(),
        |runtime| {
            save_repo_editor(runtime);
        },
        |widgets| widgets.save_repo_button.clone(),
    );

    bind_button(
        runtime.clone(),
        |runtime| {
            toggle_workspace_focus(&runtime.borrow().widgets);
        },
        |widgets| widgets.toggle_workspace_button.clone(),
    );

    bind_button(
        runtime.clone(),
        |runtime| {
            set_workspace_mode(&runtime.borrow().widgets, "editor");
            load_repo_editor(&runtime);
        },
        |widgets| widgets.show_editor_button.clone(),
    );

    bind_button(
        runtime.clone(),
        |runtime| {
            set_workspace_mode(&runtime.borrow().widgets, "diff");
            load_diff_view(&runtime);
        },
        |widgets| widgets.show_diff_button.clone(),
    );

    bind_button(
        runtime.clone(),
        |runtime| {
            reveal_selected(runtime);
        },
        |widgets| widgets.reveal_button.clone(),
    );

    bind_button(
        runtime.clone(),
        |runtime| {
            sync_with_remote(runtime);
        },
        |widgets| widgets.sync_button.clone(),
    );
}

fn install_background_tasks(runtime: Rc<RefCell<AppRuntime>>) {
    let _ = runtime;
}

fn bind_button<F, G>(runtime: Rc<RefCell<AppRuntime>>, handler: F, select: G)
where
    F: Fn(Rc<RefCell<AppRuntime>>) + 'static,
    G: Fn(&Widgets) -> Button,
{
    let button = {
        let guard = runtime.borrow();
        select(&guard.widgets)
    };
    button.connect_clicked(move |_| handler(runtime.clone()));
}

fn ensure_repo_then_refresh(runtime: Rc<RefCell<AppRuntime>>) {
    sync_profile_combo(&runtime);
    let (repo_missing, onboarding_complete) = {
        let guard = runtime.borrow();
        (
            guard.persisted.config.repo_root.is_none(),
            guard.persisted.config.onboarding_complete,
        )
    };
    if repo_missing && !onboarding_complete {
        prompt_settings(runtime, true);
    } else {
        refresh(runtime);
    }
}

fn default_repo_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".dotfiles")
}

fn prompt_settings(runtime: Rc<RefCell<AppRuntime>>, onboarding: bool) {
    let default_repo = default_repo_path();
    let widgets = runtime.borrow().widgets.clone();
    let (repo_root, current_remote_name) = {
        let guard = runtime.borrow();
        (
            guard.persisted.config.repo_root.clone(),
            guard.persisted.config.remote_name.clone(),
        )
    };
    let repo_root = repo_root.unwrap_or(default_repo);
    let current_remote_url = runtime
        .borrow()
        .persisted
        .config
        .repo_root
        .as_deref()
        .and_then(|root| git::remote_url(root, &current_remote_name).ok().flatten())
        .unwrap_or_default();
    let dialog = Dialog::builder()
        .title(if onboarding {
            "Set up dotfiles repository"
        } else {
            "Settings"
        })
        .transient_for(&widgets.window)
        .modal(true)
        .build();
    dialog.add_button(if onboarding { "Later" } else { "Cancel" }, ResponseType::Cancel);
    dialog.add_button("Choose Folder", ResponseType::Other(1));
    dialog.add_button("Use Folder", ResponseType::Accept);
    dialog.add_button("Import Repo", ResponseType::Other(2));

    let content = dialog.content_area();
    let box_ = GtkBox::new(Orientation::Vertical, 8);
    let description = Label::new(Some(
        if onboarding {
            "Choose where your dotfiles repo should live locally. You can point Doter at an existing local repository or import one from a Git or GitHub URL. You can change this later from Settings."
        } else {
            "Update the local dotfiles repository path, optionally import a repo from a Git URL, or adjust the git remote used by Doter."
        },
    ));
    description.set_wrap(true);
    description.set_xalign(0.0);
    let repo_path_entry = Entry::new();
    repo_path_entry.set_text(&repo_root.to_string_lossy());
    repo_path_entry.set_placeholder_text(Some("/home/user/.dotfiles"));
    let import_url_entry = Entry::new();
    import_url_entry.set_placeholder_text(Some("git@github.com:user/dotfiles.git"));
    let remote_name_entry = Entry::new();
    remote_name_entry.set_placeholder_text(Some("origin"));
    remote_name_entry.set_text(&current_remote_name);
    let remote_url_entry = Entry::new();
    remote_url_entry.set_placeholder_text(Some("git@github.com:user/dotfiles.git"));
    remote_url_entry.set_text(&current_remote_url);
    box_.append(&description);
    box_.append(&Label::new(Some("Local repository path")));
    box_.append(&repo_path_entry);
    box_.append(&Label::new(Some("Import from Git URL")));
    box_.append(&import_url_entry);
    box_.append(&Label::new(Some("Remote name")));
    box_.append(&remote_name_entry);
    box_.append(&Label::new(Some("Remote URL")));
    box_.append(&remote_url_entry);
    content.append(&box_);

    {
        let runtime = runtime.clone();
        let repo_path_entry = repo_path_entry.clone();
        let import_url_entry = import_url_entry.clone();
        let remote_name_entry = remote_name_entry.clone();
        let remote_url_entry = remote_url_entry.clone();
        dialog.connect_response(move |dialog, response| match response {
            ResponseType::Accept => {
                let path = PathBuf::from(repo_path_entry.text().trim().to_string());
                let remote_name = remote_name_entry.text().trim().to_string();
                let remote_url = remote_url_entry.text().trim().to_string();
                match create_or_open_repo(&runtime, &path, onboarding) {
                    Ok(()) => {
                        if let Err(error) =
                            save_remote_settings(&runtime, remote_name, remote_url, onboarding)
                        {
                            show_message(
                                &runtime.borrow().widgets.window,
                                "Settings failed",
                                &error.to_string(),
                            );
                            return;
                        }
                        dialog.close();
                        refresh(runtime.clone());
                    }
                    Err(error) => {
                        show_message(
                            &runtime.borrow().widgets.window,
                            "Settings failed",
                            &error.to_string(),
                        );
                    }
                }
            }
            ResponseType::Other(1) => {
                let chooser = FileChooserNative::builder()
                    .title("Choose dotfiles repository")
                    .transient_for(&runtime.borrow().widgets.window)
                    .action(FileChooserAction::SelectFolder)
                    .accept_label("Select")
                    .cancel_label("Cancel")
                    .build();
                let repo_path_entry = repo_path_entry.clone();
                chooser.connect_response(move |chooser, response| {
                    if response == ResponseType::Accept {
                        if let Some(file) = chooser.file() {
                            if let Some(path) = file.path() {
                                repo_path_entry.set_text(&path.to_string_lossy());
                            }
                        }
                    }
                    chooser.destroy();
                });
                chooser.show();
            }
            ResponseType::Other(2) => {
                let path = PathBuf::from(repo_path_entry.text().trim().to_string());
                let import_url = import_url_entry.text().trim().to_string();
                let remote_name = remote_name_entry.text().trim().to_string();
                let remote_url = remote_url_entry.text().trim().to_string();
                match clone_repo_to_path(&runtime, &import_url, &path, onboarding) {
                    Ok(()) => {
                        if let Err(error) =
                            save_remote_settings(&runtime, remote_name, remote_url, onboarding)
                        {
                            show_message(
                                &runtime.borrow().widgets.window,
                                "Import failed",
                                &error.to_string(),
                            );
                            return;
                        }
                        dialog.close();
                        refresh(runtime.clone());
                    }
                    Err(error) => {
                        show_message(
                            &runtime.borrow().widgets.window,
                            "Import failed",
                            &error.to_string(),
                        );
                    }
                }
            }
            _ => {
                if onboarding {
                    if let Ok(mut guard) = runtime.try_borrow_mut() {
                        guard.persisted.config.onboarding_complete = true;
                        let _ = guard.persisted.save(&guard.paths);
                    }
                }
                dialog.close();
                refresh(runtime.clone());
            }
        });
    }

    dialog.present();
}

fn create_or_open_repo(
    runtime: &Rc<RefCell<AppRuntime>>,
    path: &Path,
    onboarding: bool,
) -> anyhow::Result<()> {
    if path.as_os_str().is_empty() {
        return Err(anyhow!("Repository path is required"));
    }
    let repo_root = if let Some(existing) = git::detect_repo(path)? {
        existing
    } else {
        git::init_repo(path)?
    };
    {
        let mut guard = runtime.borrow_mut();
        guard.persisted.config.repo_root = Some(repo_root);
        if onboarding {
            guard.persisted.config.onboarding_complete = true;
        }
        guard.persisted.sync_profiles_from_repo()?;
        guard.persisted.save(&guard.paths)?;
    }
    Ok(())
}

fn clone_repo_to_path(
    runtime: &Rc<RefCell<AppRuntime>>,
    url: &str,
    path: &Path,
    onboarding: bool,
) -> anyhow::Result<()> {
    if path.as_os_str().is_empty() {
        return Err(anyhow!("Repository path is required"));
    }
    let repo_root = git::clone_repo(url, path)?;
    let mut guard = runtime.borrow_mut();
    guard.persisted.config.repo_root = Some(repo_root);
    if onboarding {
        guard.persisted.config.onboarding_complete = true;
    }
    guard.persisted.sync_profiles_from_repo()?;
    guard.persisted.save(&guard.paths)?;
    Ok(())
}

fn save_remote_settings(
    runtime: &Rc<RefCell<AppRuntime>>,
    remote_name: String,
    remote_url: String,
    onboarding: bool,
) -> anyhow::Result<()> {
    let repo_root = runtime.borrow().persisted.config.repo_root.clone();
    let Some(repo_root) = repo_root else {
        if onboarding {
            let mut guard = runtime.borrow_mut();
            guard.persisted.config.onboarding_complete = true;
            guard.persisted.save(&guard.paths)?;
        }
        return Ok(());
    };

    let remote_name = if remote_name.trim().is_empty() {
        "origin".to_string()
    } else {
        remote_name
    };
    let previous_name = runtime.borrow().persisted.config.remote_name.clone();
    if !remote_url.trim().is_empty() {
        git::update_remote(&repo_root, &previous_name, &remote_name, &remote_url)?;
    }

    let mut guard = runtime.borrow_mut();
    guard.persisted.config.remote_name = remote_name;
    if onboarding {
        guard.persisted.config.onboarding_complete = true;
    }
    let paths = guard.paths.clone();
    guard.persisted.save(&paths)?;
    Ok(())
}

fn refresh(runtime: Rc<RefCell<AppRuntime>>) {
    {
        let mut guard = runtime.borrow_mut();
        if guard.persisted.sync_profiles_from_repo().unwrap_or(false) {
            let _ = guard.persisted.save(&guard.paths);
        }
        if guard.persisted.prune_stale_managed_entries() {
            let _ = guard.persisted.save(&guard.paths);
        }
    }

    let result = {
        let guard = runtime.borrow();
        scanner::scan_dotfiles(&guard.persisted)
    };

    match result {
        Ok(report) => {
            runtime.borrow_mut().report = report;
            let repo_root = runtime.borrow().persisted.config.repo_root.clone();
            runtime.borrow_mut().git_state = repo_root
                .as_deref()
                .and_then(|root| git::repo_status(root).ok())
                .unwrap_or_default();
            sync_profile_combo(&runtime);
            render_list(&runtime);
            update_details(&runtime);
        }
        Err(error) => {
            show_message(
                &runtime.borrow().widgets.window,
                "Scan failed",
                &error.to_string(),
            );
        }
    }
}

fn render_list(runtime: &Rc<RefCell<AppRuntime>>) {
    let (list_box, grid_box, summary_label, entries, summary_text, selected_entry_id) = {
        let guard = runtime.borrow();
        let entries = browser_entries(
            &guard.report,
            &guard.filter_text,
            guard.scope_filter,
            guard.kind_filter,
            guard.sort_mode,
        );
        let home_count = guard
            .report
            .entries
            .iter()
            .filter(|entry| matches!(entry.origin, crate::model::OriginScope::Home))
            .count();
        let config_count = guard
            .report
            .entries
            .iter()
            .filter(|entry| matches!(entry.origin, crate::model::OriginScope::XdgConfig))
            .count();
        let summary_text = format!(
            "{} shown | {} total | {} home | {} config | {} conflicts | profile {} | repo {}",
            entries.len(),
            guard.report.entries.len(),
            home_count,
            config_count,
            guard.report.conflicts.len(),
            guard.persisted.config.active_profile,
            guard
                .persisted
                .config
                .repo_root
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "not configured".to_string())
        );
        (
            guard.widgets.list_box.clone(),
            guard.widgets.grid_box.clone(),
            guard.widgets.summary_label.clone(),
            entries,
            summary_text,
            guard.selected_entry_id.clone(),
        )
    };

    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }
    while let Some(child) = grid_box.first_child() {
        grid_box.remove(&child);
    }

    summary_label.set_label(&summary_text);

    runtime.borrow_mut().browser_selection_updating = true;
    let mut row_to_select: Option<ListBoxRow> = None;
    let mut card_to_select: Option<FlowBoxChild> = None;
    for entry in entries {
        let row = ListBoxRow::new();
        row.set_selectable(true);
        row.set_activatable(true);
        if selected_entry_id.as_deref() == Some(entry.id.as_str()) {
            row_to_select = Some(row.clone());
        }

        let row_box = GtkBox::new(Orientation::Vertical, 6);
        row_box.set_margin_top(8);
        row_box.set_margin_bottom(8);
        row_box.set_margin_start(10);
        row_box.set_margin_end(10);

        let row_top = GtkBox::new(Orientation::Horizontal, 8);
        let entry_icon = build_browser_row_icon(entry.kind);
        let title = Label::new(Some(&entry.display_name));
        title.set_xalign(0.0);
        title.set_hexpand(true);
        title.set_wrap(true);
        let origin_badge = scope_badge(entry.origin);
        let status_badge = status_badge(entry.managed_state);
        row_top.append(&entry_icon);
        row_top.append(&title);
        row_top.append(&origin_badge);
        row_top.append(&status_badge);

        let subtitle = Label::new(Some(&entry_browser_subtitle(&entry)));
        subtitle.set_xalign(0.0);
        subtitle.add_css_class("dim-label");
        subtitle.add_css_class("caption");

        row_box.append(&row_top);
        row_box.append(&subtitle);
        row.set_child(Some(&row_box));
        list_box.append(&row);

        let card = FlowBoxChild::new();
        if selected_entry_id.as_deref() == Some(entry.id.as_str()) {
            card_to_select = Some(card.clone());
        }

        let card_box = GtkBox::new(Orientation::Vertical, 6);
        card_box.set_margin_top(6);
        card_box.set_margin_bottom(6);
        card_box.set_margin_start(6);
        card_box.set_margin_end(6);
        card_box.set_size_request(100, 104);
        card_box.set_halign(Align::Center);
        card_box.set_valign(Align::Start);
        card_box.add_css_class("grid-card");

        let icon = build_grid_icon(entry.kind);

        let card_title = Label::new(Some(&entry.display_name));
        card_title.set_xalign(0.5);
        card_title.set_justify(gtk4::Justification::Center);
        card_title.set_wrap(true);
        card_title.add_css_class("grid-card-title");
        card_title.set_max_width_chars(12);

        let card_scope = scope_badge(entry.origin);
        card_scope.set_halign(Align::Center);

        card_box.append(&icon);
        card_box.append(&card_title);
        card_box.append(&card_scope);
        card.set_child(Some(&card_box));
        grid_box.insert(&card, -1);
    }

    list_box.unselect_all();
    grid_box.unselect_all();

    if let Some(row) = row_to_select {
        list_box.select_row(Some(&row));
    }
    if let Some(card) = card_to_select {
        grid_box.select_child(&card);
    }
    runtime.borrow_mut().browser_selection_updating = false;
}

fn browser_entries(
    report: &ScanReport,
    filter_text: &str,
    scope_filter: ScopeFilter,
    kind_filter: KindFilter,
    sort_mode: SortMode,
) -> Vec<DotfileEntry> {
    let needle = filter_text.trim().to_lowercase();
    let mut entries = report
        .entries
        .iter()
        .filter(|entry| {
            let matches_search = needle.is_empty()
                || entry.display_name.to_lowercase().contains(&needle)
                || entry
                    .path
                    .to_string_lossy()
                    .to_lowercase()
                    .contains(&needle);
            let matches_scope = match scope_filter {
                ScopeFilter::All => true,
                ScopeFilter::Home => matches!(entry.origin, crate::model::OriginScope::Home),
                ScopeFilter::Config => {
                    matches!(entry.origin, crate::model::OriginScope::XdgConfig)
                }
            };
            let matches_kind = match kind_filter {
                KindFilter::All => true,
                KindFilter::Files => matches!(entry.kind, EntryKind::File),
                KindFilter::Directories => matches!(entry.kind, EntryKind::Directory),
                KindFilter::Symlinks => matches!(entry.kind, EntryKind::Symlink),
            };
            matches_search && matches_scope && matches_kind
        })
        .cloned()
        .collect::<Vec<_>>();

    match sort_mode {
        SortMode::NameAsc => {
            entries.sort_by(|left, right| left.display_name.cmp(&right.display_name))
        }
        SortMode::NameDesc => {
            entries.sort_by(|left, right| right.display_name.cmp(&left.display_name))
        }
        SortMode::Scope => entries.sort_by(|left, right| {
            scope_sort_key(left.origin)
                .cmp(&scope_sort_key(right.origin))
                .then(left.display_name.cmp(&right.display_name))
        }),
        SortMode::Status => entries.sort_by(|left, right| {
            status_sort_key(left.managed_state)
                .cmp(&status_sort_key(right.managed_state))
                .then(left.display_name.cmp(&right.display_name))
        }),
    }

    entries
}

fn scope_sort_key(origin: crate::model::OriginScope) -> u8 {
    match origin {
        crate::model::OriginScope::XdgConfig => 0,
        crate::model::OriginScope::Home => 1,
    }
}

fn status_sort_key(state: ManagedState) -> u8 {
    match state {
        ManagedState::ManagedActive => 0,
        ManagedState::Conflicted => 1,
        ManagedState::ManagedInactive => 2,
        ManagedState::Unmanaged => 3,
    }
}

fn scope_badge(origin: crate::model::OriginScope) -> Label {
    let text = match origin {
        crate::model::OriginScope::Home => "HOME",
        crate::model::OriginScope::XdgConfig => "CONFIG",
    };
    let label = Label::new(Some(text));
    label.add_css_class("scope-badge");
    match origin {
        crate::model::OriginScope::Home => label.add_css_class("scope-home"),
        crate::model::OriginScope::XdgConfig => label.add_css_class("scope-config"),
    }
    label
}

fn status_badge(state: ManagedState) -> Label {
    let label = Label::new(Some(match state {
        ManagedState::Unmanaged => "Unmanaged",
        ManagedState::ManagedActive => "Active",
        ManagedState::ManagedInactive => "Inactive",
        ManagedState::Conflicted => "Conflicted",
    }));
    label.add_css_class("status-badge");
    match state {
        ManagedState::ManagedActive => label.add_css_class("status-active"),
        ManagedState::ManagedInactive => label.add_css_class("status-inactive"),
        ManagedState::Conflicted => label.add_css_class("status-conflicted"),
        ManagedState::Unmanaged => label.add_css_class("status-unmanaged"),
    }
    label
}

fn entry_browser_subtitle(entry: &DotfileEntry) -> String {
    match entry.origin {
        crate::model::OriginScope::Home => format!("~/{}", entry.display_name),
        crate::model::OriginScope::XdgConfig => format!(".config/{}", entry.display_name),
    }
}

fn sync_repo_editor(runtime: &Rc<RefCell<AppRuntime>>) {
    let selected = selected_entry(runtime);
    let widgets = runtime.borrow().widgets.clone();

    let Some(entry) = selected else {
        clear_repo_editor(runtime, "Select a managed entry to edit its repo files.");
        widgets
            .workspace_path_label
            .set_label("Select a managed entry to browse repo files or open a diff.");
        return;
    };
    let Some(managed_source) = entry.managed_source else {
        clear_repo_editor(
            runtime,
            "This entry is not managed in the repo yet. Enable it first to edit the repo copy.",
        );
        widgets.workspace_path_label.set_label(
            "This entry is not managed in the repo yet, so there is no repo explorer or diff.",
        );
        return;
    };

    let repo_files = match repo_files_for_source(&managed_source) {
        Ok(files) => files,
        Err(error) => {
            clear_repo_editor(runtime, &format!("Unable to load repo files: {}", error));
            widgets
                .workspace_path_label
                .set_label("Unable to load repo files for this entry.");
            return;
        }
    };

    if repo_files.is_empty() {
        clear_repo_editor(
            runtime,
            "The managed repo path exists but contains no editable files.",
        );
        widgets
            .workspace_path_label
            .set_label("The managed path exists but does not contain editable text files.");
        return;
    }

    let selected_repo_file = {
        let guard = runtime.borrow();
        guard
            .selected_repo_file
            .clone()
            .filter(|path| repo_files.contains(path))
            .or_else(|| repo_files.first().cloned())
    };

    {
        let mut guard = runtime.borrow_mut();
        guard.repo_root_path = Some(managed_source.clone());
        guard.repo_files = repo_files.clone();
        guard.selected_repo_file = selected_repo_file.clone();
        guard.active_repo_row_path = selected_repo_file
            .clone()
            .or_else(|| Some(managed_source.clone()));
        ensure_expanded_dirs(
            &mut guard.expanded_repo_dirs,
            &managed_source,
            selected_repo_file.as_deref(),
        );
    }
    render_repo_explorer(runtime);

    widgets.workspace_path_label.set_label(&format!(
        "{}  •  {} file{}",
        managed_source.display(),
        repo_files.len(),
        if repo_files.len() == 1 { "" } else { "s" }
    ));
    refresh_active_workspace_view(runtime);
}

fn refresh_active_workspace_view(runtime: &Rc<RefCell<AppRuntime>>) {
    let mode = runtime
        .borrow()
        .widgets
        .workspace_stack
        .visible_child_name()
        .map(|name| name.to_string())
        .unwrap_or_else(|| "editor".to_string());
    if mode == "diff" {
        load_diff_view(runtime);
    } else {
        load_repo_editor(runtime);
    }
}

fn clear_repo_editor(runtime: &Rc<RefCell<AppRuntime>>, status: &str) {
    let widgets = runtime.borrow().widgets.clone();
    {
        let mut guard = runtime.borrow_mut();
        guard.repo_root_path = None;
        guard.repo_files.clear();
        guard.repo_rows.clear();
        guard.expanded_repo_dirs.clear();
        guard.active_repo_row_path = None;
        guard.selected_repo_file = None;
        guard.loaded_repo_content = None;
    }

    while let Some(child) = widgets.repo_files_list.first_child() {
        widgets.repo_files_list.remove(&child);
    }
    widgets.repo_editor_status.set_label(status);
    widgets.repo_editor_line_numbers.buffer().set_text("");
    widgets.repo_editor_view.buffer().set_text("");
    widgets.repo_editor_view.set_editable(false);
    widgets.ignore_repo_button.set_sensitive(false);
    widgets.save_repo_button.set_sensitive(false);
    widgets.reload_repo_button.set_sensitive(false);
    widgets.diff_status_label.set_label(status);
    widgets.diff_base_title.set_label("HEAD");
    widgets.diff_current_title.set_label("Working Tree");
    widgets.diff_base_view.buffer().set_text("");
    widgets.diff_current_view.buffer().set_text("");
}

fn render_repo_explorer(runtime: &Rc<RefCell<AppRuntime>>) {
    let widgets = runtime.borrow().widgets.clone();
    let (root, active_repo_row_path, rows) = {
        let mut guard = runtime.borrow_mut();
        let Some(root) = guard.repo_root_path.clone() else {
            return;
        };
        guard.repo_explorer_updating = true;
        let rows = build_repo_explorer_rows(&root, &guard.expanded_repo_dirs).unwrap_or_default();
        guard.repo_rows = rows.clone();
        (root, guard.active_repo_row_path.clone(), rows)
    };

    while let Some(child) = widgets.repo_files_list.first_child() {
        widgets.repo_files_list.remove(&child);
    }

    let mut row_to_select: Option<ListBoxRow> = None;
    for row_data in rows {
        let row = ListBoxRow::new();
        row.set_selectable(true);
        row.set_activatable(true);
        if active_repo_row_path.as_ref() == Some(&row_data.path) {
            row_to_select = Some(row.clone());
        }

        let row_box = GtkBox::new(Orientation::Horizontal, 8);
        row_box.set_margin_top(6);
        row_box.set_margin_bottom(6);
        row_box.set_margin_end(8);
        row_box.set_margin_start(8 + (row_data.depth as i32 * 16));

        let icon_name = if row_data.is_dir {
            if row_data.expanded {
                "pan-down-symbolic"
            } else {
                "pan-end-symbolic"
            }
        } else {
            "text-x-generic-symbolic"
        };
        let icon = Image::from_icon_name(icon_name);
        icon.set_pixel_size(14);
        icon.add_css_class("row-icon");

        let label = Label::new(Some(&row_data.label));
        label.set_xalign(0.0);
        label.set_hexpand(true);
        label.set_ellipsize(pango::EllipsizeMode::Middle);
        if row_data.is_dir {
            label.add_css_class("heading");
        }

        row_box.append(&icon);
        row_box.append(&label);
        row.set_child(Some(&row_box));
        widgets.repo_files_list.append(&row);
    }

    widgets.repo_files_list.unselect_all();
    if let Some(row) = row_to_select {
        widgets.repo_files_list.select_row(Some(&row));
    } else if active_repo_row_path.is_none() && root.is_file() {
        if let Some(first_row) = widgets.repo_files_list.row_at_index(0) {
            widgets.repo_files_list.select_row(Some(&first_row));
        }
    }
    runtime.borrow_mut().repo_explorer_updating = false;
}

fn load_repo_editor(runtime: &Rc<RefCell<AppRuntime>>) {
    let selected = selected_entry(runtime);
    let (widgets, managed_source, selected_repo_file) = {
        let guard = runtime.borrow();
        (
            guard.widgets.clone(),
            selected.and_then(|entry| entry.managed_source),
            guard.selected_repo_file.clone(),
        )
    };

    let Some(managed_source) = managed_source else {
        clear_repo_editor(runtime, "This entry is not managed in the repo yet.");
        return;
    };
    let Some(selected_repo_file) = selected_repo_file else {
        widgets
            .repo_editor_status
            .set_label("Select a repo file to edit it.");
        widgets.repo_editor_line_numbers.buffer().set_text("");
        widgets.repo_editor_view.buffer().set_text("");
        widgets.repo_editor_view.set_editable(false);
        widgets.save_repo_button.set_sensitive(false);
        widgets.reload_repo_button.set_sensitive(false);
        return;
    };

    widgets.ignore_repo_button.set_sensitive(true);
    widgets.reload_repo_button.set_sensitive(true);

    match load_text_with_limit(&selected_repo_file, EDITOR_PREVIEW_CHAR_LIMIT) {
        Ok((content, truncated)) => {
            let label = repo_file_label(&managed_source, &selected_repo_file);
            let line_count = content.split('\n').count().max(1);
            let editable = !truncated
                && content.len() <= EDITOR_EDIT_BYTE_LIMIT
                && line_count <= EDITOR_EDIT_LINE_LIMIT;
            let (rendered_content, rendered_truncated) =
                truncate_for_preview_lines(&content, EDITOR_PREVIEW_LINE_LIMIT, "editor preview");
            let rendered_line_count = rendered_content.split('\n').count().max(1);
            let highlight_enabled =
                !rendered_truncated && should_enable_editor_highlighting(content.len(), line_count);
            let status_suffix = if !editable {
                " (read-only preview: file is too large to edit safely here)"
            } else if highlight_enabled {
                ""
            } else {
                " (highlighting disabled for large file)"
            };
            let status_suffix = if rendered_truncated {
                format!(
                    "{} (showing first {} lines)",
                    status_suffix, EDITOR_PREVIEW_LINE_LIMIT
                )
            } else {
                status_suffix.to_string()
            };
            widgets
                .repo_editor_status
                .set_label(&format!("Editing: {}{}", label, status_suffix));
            update_editor_line_numbers(&widgets.repo_editor_line_numbers, rendered_line_count);
            widgets
                .repo_editor_view
                .buffer()
                .set_text(&rendered_content);
            if editable && highlight_enabled {
                apply_syntax_highlighting(
                    &widgets.repo_editor_view,
                    &selected_repo_file,
                    &rendered_content,
                );
            } else {
                clear_text_view_highlighting(&widgets.repo_editor_view);
            }
            widgets.repo_editor_view.set_editable(editable);
            widgets.save_repo_button.set_sensitive(editable);
            {
                let mut guard = runtime.borrow_mut();
                guard.loaded_repo_content = if editable { Some(content) } else { None };
            }
        }
        Err(error) if error.kind() == ErrorKind::InvalidData => {
            widgets.repo_editor_status.set_label(&format!(
                "Cannot edit {} - not valid UTF-8.",
                repo_file_label(&managed_source, &selected_repo_file)
            ));
            widgets.repo_editor_line_numbers.buffer().set_text("");
            widgets.repo_editor_view.buffer().set_text(
                "This file is not UTF-8 text, so the embedded editor is disabled.
",
            );
            clear_text_view_highlighting(&widgets.repo_editor_view);
            widgets.repo_editor_view.set_editable(false);
            widgets.save_repo_button.set_sensitive(false);
            runtime.borrow_mut().loaded_repo_content = None;
        }
        Err(error) => {
            widgets
                .repo_editor_status
                .set_label("Unable to load the selected repo file.");
            widgets.repo_editor_line_numbers.buffer().set_text("");
            widgets.repo_editor_view.buffer().set_text(&format!(
                "Failed to read {}:
{}",
                selected_repo_file.display(),
                error
            ));
            clear_text_view_highlighting(&widgets.repo_editor_view);
            widgets.repo_editor_view.set_editable(false);
            widgets.save_repo_button.set_sensitive(false);
            runtime.borrow_mut().loaded_repo_content = None;
        }
    }
}

fn load_diff_view(runtime: &Rc<RefCell<AppRuntime>>) {
    let selected = selected_entry(runtime);
    let (widgets, repo_root, managed_source, selected_repo_file) = {
        let guard = runtime.borrow();
        (
            guard.widgets.clone(),
            guard.persisted.config.repo_root.clone(),
            selected.and_then(|entry| entry.managed_source),
            guard.selected_repo_file.clone(),
        )
    };

    let Some(managed_source) = managed_source else {
        widgets
            .diff_status_label
            .set_label("Enable this entry first to compare repo changes.");
        widgets.diff_base_title.set_label("HEAD");
        widgets.diff_current_title.set_label("Working Tree");
        widgets.diff_base_view.buffer().set_text("");
        widgets.diff_current_view.buffer().set_text("");
        return;
    };

    let Some(selected_repo_file) = selected_repo_file else {
        widgets
            .diff_status_label
            .set_label("Select a repo file in the explorer to view its diff.");
        widgets.diff_base_title.set_label("HEAD");
        widgets.diff_current_title.set_label("Working Tree");
        widgets.diff_base_view.buffer().set_text("");
        widgets.diff_current_view.buffer().set_text("");
        return;
    };

    let label = repo_file_label(&managed_source, &selected_repo_file);
    widgets
        .diff_base_title
        .set_label(&format!("HEAD  •  {}", label));
    widgets
        .diff_current_title
        .set_label(&format!("Working Tree  •  {}", label));

    let current_content = match fs::read_to_string(&selected_repo_file) {
        Ok(content) => content,
        Err(error) if error.kind() == ErrorKind::InvalidData => {
            widgets
                .diff_status_label
                .set_label("The current file is not valid UTF-8, so diff preview is unavailable.");
            widgets.diff_base_view.buffer().set_text("");
            widgets.diff_current_view.buffer().set_text("");
            return;
        }
        Err(error) => {
            widgets
                .diff_status_label
                .set_label(&format!("Unable to read working tree file: {}", error));
            widgets.diff_base_view.buffer().set_text("");
            widgets.diff_current_view.buffer().set_text("");
            return;
        }
    };

    let base_content = match repo_root
        .as_deref()
        .map(|root| git::tracked_file_text(root, &selected_repo_file))
        .transpose()
    {
        Ok(content) => content.flatten(),
        Err(error) => {
            widgets.diff_status_label.set_label(&format!(
                "Unable to read the tracked file from HEAD: {}",
                error
            ));
            widgets.diff_base_view.buffer().set_text("");
            widgets.diff_current_view.buffer().set_text("");
            return;
        }
    };

    let base_text = base_content.as_deref().unwrap_or("");
    let current_line_count = current_content.split('\n').count().max(1);
    let base_line_count = base_text.split('\n').count().max(1);
    let too_large_for_diff = current_content.len() > DIFF_BYTE_LIMIT
        || base_text.len() > DIFF_BYTE_LIMIT
        || current_line_count > DIFF_LINE_LIMIT
        || base_line_count > DIFF_LINE_LIMIT;

    if too_large_for_diff {
        let base_preview = truncate_for_preview(base_text, DIFF_PREVIEW_CHAR_LIMIT);
        let current_preview = truncate_for_preview(&current_content, DIFF_PREVIEW_CHAR_LIMIT);
        let (base_preview, base_line_truncated) =
            truncate_for_preview_lines(&base_preview, DIFF_PREVIEW_LINE_LIMIT, "diff preview");
        let (current_preview, current_line_truncated) =
            truncate_for_preview_lines(&current_preview, DIFF_PREVIEW_LINE_LIMIT, "diff preview");
        widgets.diff_base_view.buffer().set_text(&base_preview);
        widgets
            .diff_current_view
            .buffer()
            .set_text(&current_preview);
        clear_text_view_highlighting(&widgets.diff_base_view);
        clear_text_view_highlighting(&widgets.diff_current_view);
        let line_limit_hint = if base_line_truncated || current_line_truncated {
            format!(" Showing first {} lines only.", DIFF_PREVIEW_LINE_LIMIT)
        } else {
            String::new()
        };
        widgets.diff_status_label.set_label(&format!(
            "Diff preview disabled for large files to keep the app responsive.{}",
            line_limit_hint
        ));
        return;
    }

    let (base_rendered, current_rendered, diff_truncated) =
        render_side_by_side_diff(base_text, &current_content, DIFF_RENDER_LINE_LIMIT);
    widgets.diff_base_view.buffer().set_text(&base_rendered);
    widgets
        .diff_current_view
        .buffer()
        .set_text(&current_rendered);
    if diff_truncated {
        clear_text_view_highlighting(&widgets.diff_base_view);
        clear_text_view_highlighting(&widgets.diff_current_view);
    } else {
        apply_diff_syntax_highlighting(
            &widgets.diff_base_view,
            &selected_repo_file,
            &base_rendered,
        );
        apply_diff_syntax_highlighting(
            &widgets.diff_current_view,
            &selected_repo_file,
            &current_rendered,
        );
    }

    let status = if base_content.is_none() {
        "New file in working tree. Nothing tracked in HEAD yet.".to_string()
    } else if base_text == current_content {
        "No textual changes between HEAD and the working tree.".to_string()
    } else if diff_truncated {
        format!(
            "Showing a side-by-side line diff between HEAD and the working tree (limited to {} lines for responsiveness).",
            DIFF_RENDER_LINE_LIMIT
        )
    } else {
        "Showing a side-by-side line diff between HEAD and the working tree.".to_string()
    };
    widgets.diff_status_label.set_label(&status);
}

fn render_side_by_side_diff(base: &str, current: &str, max_lines: usize) -> (String, String, bool) {
    let diff = TextDiff::from_lines(base, current);
    let mut left = String::new();
    let mut right = String::new();
    let mut left_line = 1usize;
    let mut right_line = 1usize;
    let mut rendered_lines = 0usize;
    let mut truncated = false;

    'ops: for op in diff.ops() {
        for change in diff.iter_changes(op) {
            let rendered = change.to_string();
            let mut lines = rendered.lines();
            if rendered.is_empty() {
                lines.next();
            }
            for line in lines {
                if rendered_lines >= max_lines {
                    truncated = true;
                    break 'ops;
                }
                match change.tag() {
                    ChangeTag::Equal => {
                        push_diff_line(&mut left, Some(left_line), line);
                        push_diff_line(&mut right, Some(right_line), line);
                        left_line += 1;
                        right_line += 1;
                    }
                    ChangeTag::Delete => {
                        push_diff_line(&mut left, Some(left_line), line);
                        push_diff_line(&mut right, None, "");
                        left_line += 1;
                    }
                    ChangeTag::Insert => {
                        push_diff_line(&mut left, None, "");
                        push_diff_line(&mut right, Some(right_line), line);
                        right_line += 1;
                    }
                }
                rendered_lines += 1;
            }
        }
    }

    if left.is_empty() && right.is_empty() {
        push_diff_line(&mut left, None, "");
        push_diff_line(&mut right, None, "");
    }

    if truncated {
        push_diff_line(
            &mut left,
            None,
            "... diff output truncated for performance ...",
        );
        push_diff_line(
            &mut right,
            None,
            "... diff output truncated for performance ...",
        );
    }

    (left, right, truncated)
}

fn push_diff_line(output: &mut String, line_no: Option<usize>, line: &str) {
    match line_no {
        Some(number) => output.push_str(&format!("{:>4} | {}\n", number, line)),
        None => output.push_str(&format!("     | {}\n", line)),
    }
}

fn update_editor_line_numbers(view: &TextView, line_count: usize) {
    let capped_line_count = line_count.min(EDITOR_LINE_NUMBER_LIMIT);
    let mut numbers = (1..=capped_line_count)
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    if line_count > capped_line_count {
        numbers.push_str("\n...");
    }
    view.buffer().set_text(&numbers);
}

fn should_enable_editor_highlighting(byte_len: usize, line_count: usize) -> bool {
    byte_len <= EDITOR_HIGHLIGHT_BYTE_LIMIT && line_count <= EDITOR_HIGHLIGHT_LINE_LIMIT
}

fn truncate_for_preview(content: &str, max_chars: usize) -> String {
    let char_count = content.chars().count();
    if char_count <= max_chars {
        return content.to_string();
    }
    let mut preview = content.chars().take(max_chars).collect::<String>();
    preview.push_str("\n\n... preview truncated for performance ...");
    preview
}

fn truncate_for_preview_lines(content: &str, max_lines: usize, label: &str) -> (String, bool) {
    let mut output = String::new();
    let mut line_count = 0usize;
    let mut truncated = false;

    for line in content.lines() {
        if line_count >= max_lines {
            truncated = true;
            break;
        }
        output.push_str(line);
        output.push('\n');
        line_count += 1;
    }

    if !truncated {
        if content.ends_with('\n') {
            return (output, false);
        }
        return (content.to_string(), false);
    }

    output.push_str(&format!("\n... {} truncated for performance ...", label));
    (output, true)
}

fn load_text_with_limit(path: &Path, max_chars: usize) -> std::io::Result<(String, bool)> {
    let mut file = fs::File::open(path)?;
    let max_bytes = max_chars.saturating_mul(4).saturating_add(1);
    let mut bytes = Vec::with_capacity(max_bytes.min(256 * 1024));
    let mut chunk = [0u8; 16 * 1024];

    loop {
        let read = file.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        let remaining = max_bytes.saturating_sub(bytes.len());
        if remaining == 0 {
            break;
        }
        let take = read.min(remaining);
        bytes.extend_from_slice(&chunk[..take]);
        if bytes.len() >= max_bytes {
            break;
        }
    }

    let mut text = String::from_utf8(bytes)
        .map_err(|_| std::io::Error::new(ErrorKind::InvalidData, "not valid UTF-8"))?;

    let mut truncated = false;
    if text.chars().count() > max_chars {
        text = truncate_for_preview(&text, max_chars);
        truncated = true;
    }

    Ok((text, truncated))
}

fn set_workspace_mode(widgets: &Widgets, mode: &str) {
    widgets.workspace_stack.set_visible_child_name(mode);
    let editor_active = mode == "editor";
    if editor_active {
        widgets
            .show_editor_button
            .add_css_class("mode-button-active");
        widgets
            .show_diff_button
            .remove_css_class("mode-button-active");
    } else {
        widgets.show_diff_button.add_css_class("mode-button-active");
        widgets
            .show_editor_button
            .remove_css_class("mode-button-active");
    }
}

fn toggle_workspace_focus(widgets: &Widgets) {
    let focused = widgets.overview_section.is_visible();
    widgets.overview_section.set_visible(!focused);
    if focused {
        widgets.toggle_workspace_button.set_label("Overview");
        widgets
            .toggle_workspace_button
            .add_css_class("mode-button-active");
    } else {
        widgets.toggle_workspace_button.set_label("Focus");
        widgets
            .toggle_workspace_button
            .remove_css_class("mode-button-active");
    }
}

fn apply_syntax_highlighting(view: &TextView, path: &Path, content: &str) {
    let buffer = view.buffer();
    clear_text_view_highlighting(view);

    let keyword_tag = ensure_text_tag(
        &buffer,
        "syntax-keyword",
        "#89ddff",
        Some(pango::Weight::Bold),
    );
    let string_tag = ensure_text_tag(&buffer, "syntax-string", "#ecc48d", None);
    let comment_tag = ensure_text_tag(&buffer, "syntax-comment", "#7f8c98", None);
    let number_tag = ensure_text_tag(&buffer, "syntax-number", "#c792ea", None);

    let keywords = syntax_keywords(path);
    let comment_prefix = syntax_comment_prefix(path);
    let mut line_start_chars = 0usize;

    for line in content.split('\n') {
        let comment_offset = comment_prefix.and_then(|prefix| line.find(prefix));
        let code_segment = comment_offset.map(|offset| &line[..offset]).unwrap_or(line);

        if let Some(offset) = comment_offset {
            let comment_start = line[..offset].chars().count();
            let comment_end = line.chars().count();
            apply_text_tag_range(
                &buffer,
                &comment_tag,
                line_start_chars + comment_start,
                line_start_chars + comment_end,
            );
        }

        for (start, end) in scan_string_ranges(code_segment) {
            apply_text_tag_range(
                &buffer,
                &string_tag,
                line_start_chars + start,
                line_start_chars + end,
            );
        }
        for (start, end) in scan_number_ranges(code_segment) {
            apply_text_tag_range(
                &buffer,
                &number_tag,
                line_start_chars + start,
                line_start_chars + end,
            );
        }
        for (start, end) in scan_keyword_ranges(code_segment, keywords) {
            apply_text_tag_range(
                &buffer,
                &keyword_tag,
                line_start_chars + start,
                line_start_chars + end,
            );
        }

        line_start_chars += line.chars().count() + 1;
    }
}

fn apply_diff_syntax_highlighting(view: &TextView, path: &Path, content: &str) {
    let buffer = view.buffer();
    clear_text_view_highlighting(view);

    let keyword_tag = ensure_text_tag(
        &buffer,
        "syntax-keyword",
        "#89ddff",
        Some(pango::Weight::Bold),
    );
    let string_tag = ensure_text_tag(&buffer, "syntax-string", "#ecc48d", None);
    let comment_tag = ensure_text_tag(&buffer, "syntax-comment", "#7f8c98", None);
    let number_tag = ensure_text_tag(&buffer, "syntax-number", "#c792ea", None);

    let keywords = syntax_keywords(path);
    let comment_prefix = syntax_comment_prefix(path);
    let mut line_start_chars = 0usize;

    for line in content.split('\n') {
        let Some(pipe_offset) = line.find("| ") else {
            line_start_chars += line.chars().count() + 1;
            continue;
        };

        let code_prefix_chars = line[..pipe_offset + 2].chars().count();
        let code = &line[pipe_offset + 2..];
        let comment_offset = comment_prefix.and_then(|prefix| code.find(prefix));
        let code_segment = comment_offset.map(|offset| &code[..offset]).unwrap_or(code);

        if let Some(offset) = comment_offset {
            let comment_start = code[..offset].chars().count();
            let comment_end = code.chars().count();
            apply_text_tag_range(
                &buffer,
                &comment_tag,
                line_start_chars + code_prefix_chars + comment_start,
                line_start_chars + code_prefix_chars + comment_end,
            );
        }

        for (start, end) in scan_string_ranges(code_segment) {
            apply_text_tag_range(
                &buffer,
                &string_tag,
                line_start_chars + code_prefix_chars + start,
                line_start_chars + code_prefix_chars + end,
            );
        }
        for (start, end) in scan_number_ranges(code_segment) {
            apply_text_tag_range(
                &buffer,
                &number_tag,
                line_start_chars + code_prefix_chars + start,
                line_start_chars + code_prefix_chars + end,
            );
        }
        for (start, end) in scan_keyword_ranges(code_segment, keywords) {
            apply_text_tag_range(
                &buffer,
                &keyword_tag,
                line_start_chars + code_prefix_chars + start,
                line_start_chars + code_prefix_chars + end,
            );
        }

        line_start_chars += line.chars().count() + 1;
    }
}

fn clear_text_view_highlighting(view: &TextView) {
    let buffer = view.buffer();
    let start = buffer.start_iter();
    let end = buffer.end_iter();
    buffer.remove_all_tags(&start, &end);
}

fn ensure_text_tag(
    buffer: &gtk4::TextBuffer,
    name: &str,
    color: &str,
    weight: Option<pango::Weight>,
) -> TextTag {
    let table = buffer.tag_table();
    if let Some(tag) = table.lookup(name) {
        return tag;
    }

    let mut builder = TextTag::builder().name(name);
    if let Ok(rgba) = gdk::RGBA::parse(color) {
        builder = builder.foreground_rgba(&rgba);
    }
    if let Some(weight) = weight {
        builder = builder.weight(weight.into_glib());
    }
    let tag = builder.build();
    table.add(&tag);
    tag
}

fn apply_text_tag_range(buffer: &gtk4::TextBuffer, tag: &TextTag, start: usize, end: usize) {
    if start >= end {
        return;
    }
    let start_iter = buffer.iter_at_offset(start as i32);
    let end_iter = buffer.iter_at_offset(end as i32);
    buffer.apply_tag(tag, &start_iter, &end_iter);
}

fn syntax_comment_prefix(path: &Path) -> Option<&'static str> {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
    {
        "lua" => Some("--"),
        "sh" | "bash" | "zsh" | "fish" | "toml" | "yaml" | "yml" | "conf" | "ini" => Some("#"),
        "rs" | "js" | "ts" | "tsx" | "jsx" | "c" | "cpp" | "h" | "hpp" | "go" | "java" => {
            Some("//")
        }
        _ => None,
    }
}

fn syntax_keywords(path: &Path) -> &'static [&'static str] {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
    {
        "lua" => &[
            "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "if", "in",
            "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
        ],
        "json" => &["true", "false", "null"],
        "toml" => &["true", "false"],
        "sh" | "bash" | "zsh" | "fish" => &[
            "if", "then", "else", "fi", "for", "do", "done", "case", "esac", "function", "export",
            "local", "return", "in",
        ],
        _ => &[],
    }
}

fn scan_string_ranges(line: &str) -> Vec<(usize, usize)> {
    let chars: Vec<char> = line.chars().collect();
    let mut ranges = Vec::new();
    let mut index = 0usize;

    while index < chars.len() {
        let quote = chars[index];
        if quote == '"' || quote == '\'' {
            let start = index;
            index += 1;
            while index < chars.len() {
                if chars[index] == quote && chars.get(index.wrapping_sub(1)) != Some(&'\\') {
                    index += 1;
                    break;
                }
                index += 1;
            }
            ranges.push((start, index.min(chars.len())));
            continue;
        }
        index += 1;
    }

    ranges
}

fn scan_number_ranges(line: &str) -> Vec<(usize, usize)> {
    let chars: Vec<char> = line.chars().collect();
    let mut ranges = Vec::new();
    let mut index = 0usize;

    while index < chars.len() {
        if chars[index].is_ascii_digit() {
            let start = index;
            index += 1;
            while index < chars.len() && (chars[index].is_ascii_digit() || chars[index] == '.') {
                index += 1;
            }
            ranges.push((start, index));
            continue;
        }
        index += 1;
    }

    ranges
}

fn scan_keyword_ranges(line: &str, keywords: &[&str]) -> Vec<(usize, usize)> {
    let chars: Vec<char> = line.chars().collect();
    let mut ranges = Vec::new();
    let mut index = 0usize;

    while index < chars.len() {
        if chars[index].is_ascii_alphabetic() || chars[index] == '_' {
            let start = index;
            index += 1;
            while index < chars.len()
                && (chars[index].is_ascii_alphanumeric() || chars[index] == '_')
            {
                index += 1;
            }
            let word = chars[start..index].iter().collect::<String>();
            if keywords.iter().any(|keyword| *keyword == word) {
                ranges.push((start, index));
            }
            continue;
        }
        index += 1;
    }

    ranges
}

fn present_sync_notice(widgets: &Widgets, title: &str, body: &str, is_error: bool) {
    widgets.notice_label.remove_css_class("notice-success");
    widgets.notice_label.remove_css_class("notice-error");
    widgets.notice_label.set_label(body);
    widgets.notice_label.add_css_class(if is_error {
        "notice-error"
    } else {
        "notice-success"
    });
    widgets.notice_revealer.set_reveal_child(true);

    if let Some(app) = widgets
        .window
        .application()
        .and_then(|app| app.downcast::<Application>().ok())
    {
        let notification = gio::Notification::new(title);
        notification.set_body(Some(body));
        app.send_notification(None, &notification);
    }

    let revealer = widgets.notice_revealer.clone();
    glib::timeout_add_seconds_local(5, move || {
        revealer.set_reveal_child(false);
        ControlFlow::Break
    });
}

fn repo_files_for_source(source: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_repo_files(source, &mut files)?;
    if let Some(gitignore) = gitignore_path_for_source(source) {
        if gitignore.exists() && !files.iter().any(|file| file == &gitignore) {
            files.push(gitignore);
        }
    }
    files.sort();
    Ok(files)
}

fn build_repo_explorer_rows(
    root: &Path,
    expanded_dirs: &BTreeSet<PathBuf>,
) -> anyhow::Result<Vec<RepoExplorerRow>> {
    let mut rows = Vec::new();
    if root.is_dir() {
        append_repo_explorer_rows(root, root, expanded_dirs, 0, &mut rows)?;
    } else {
        rows.push(RepoExplorerRow {
            path: root.to_path_buf(),
            label: root
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| root.display().to_string()),
            depth: 0,
            is_dir: false,
            expanded: false,
        });
    }
    if let Some(gitignore) = gitignore_path_for_source(root) {
        if gitignore.exists() && !gitignore.starts_with(root) {
            rows.push(RepoExplorerRow {
                path: gitignore.clone(),
                label: repo_file_label(root, &gitignore),
                depth: 0,
                is_dir: false,
                expanded: false,
            });
        }
    }
    Ok(rows)
}

fn append_repo_explorer_rows(
    root: &Path,
    current: &Path,
    expanded_dirs: &BTreeSet<PathBuf>,
    depth: usize,
    rows: &mut Vec<RepoExplorerRow>,
) -> anyhow::Result<()> {
    let mut directories = Vec::new();
    let mut files = Vec::new();

    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
            directories.push(path);
        } else {
            files.push(path);
        }
    }

    directories.sort();
    files.sort();

    for directory in directories {
        let expanded = expanded_dirs.contains(&directory);
        rows.push(RepoExplorerRow {
            label: directory
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| repo_file_label(root, &directory)),
            path: directory.clone(),
            depth,
            is_dir: true,
            expanded,
        });

        if expanded {
            append_repo_explorer_rows(root, &directory, expanded_dirs, depth + 1, rows)?;
        }
    }

    for file in files {
        rows.push(RepoExplorerRow {
            label: file
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| repo_file_label(root, &file)),
            path: file,
            depth,
            is_dir: false,
            expanded: false,
        });
    }

    Ok(())
}

fn ensure_expanded_dirs(
    expanded_dirs: &mut BTreeSet<PathBuf>,
    root: &Path,
    selected_file: Option<&Path>,
) {
    if root.is_dir() {
        expanded_dirs.insert(root.to_path_buf());
    }

    let Some(selected_file) = selected_file else {
        return;
    };

    let mut current = selected_file.parent();
    while let Some(path) = current {
        if !path.starts_with(root) {
            break;
        }
        expanded_dirs.insert(path.to_path_buf());
        if path == root {
            break;
        }
        current = path.parent();
    }
}

fn collect_repo_files(path: &Path, files: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_dir() {
        let mut children = fs::read_dir(path)?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        children.sort();
        for child in children {
            collect_repo_files(&child, files)?;
        }
        return Ok(());
    }

    files.push(path.to_path_buf());
    Ok(())
}

fn gitignore_path_for_source(source: &Path) -> Option<PathBuf> {
    if source.is_dir() {
        return Some(source.join(".gitignore"));
    }

    source.parent().map(|parent| parent.join(".gitignore"))
}

fn repo_paths_to_stage(source: &Path) -> Vec<PathBuf> {
    let mut paths = vec![source.to_path_buf()];
    if let Some(gitignore) =
        gitignore_path_for_source(source).filter(|path| path.exists() && !path.starts_with(source))
    {
        paths.push(gitignore);
    }
    paths
}

fn ignore_pattern_for_path(root: &Path, path: &Path) -> Option<String> {
    let relative = if root.is_dir() {
        path.strip_prefix(root).ok()?.to_path_buf()
    } else if path == root {
        PathBuf::from(path.file_name()?)
    } else {
        return None;
    };

    let is_directory = fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_dir() && !metadata.file_type().is_symlink())
        .unwrap_or(false);
    let mut pattern = format!("/{}", relative.to_string_lossy().replace('\\', "/"));
    if is_directory && !pattern.ends_with('/') {
        pattern.push('/');
    }
    Some(pattern)
}

fn append_gitignore_rule(existing: &str, rule: &str) -> Option<String> {
    if existing.lines().map(str::trim).any(|line| line == rule) {
        return None;
    }

    let mut updated = existing.to_string();
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(rule);
    updated.push('\n');
    Some(updated)
}

fn repo_file_label(root: &Path, file: &Path) -> String {
    if gitignore_path_for_source(root).as_deref() == Some(file) && !file.starts_with(root) {
        return ".gitignore".to_string();
    }

    if root == file {
        return file
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| file.display().to_string());
    }

    file.strip_prefix(root)
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| file.display().to_string())
}

fn build_grid_icon(kind: EntryKind) -> Image {
    let icon_name = match kind {
        EntryKind::Directory => "folder",
        EntryKind::Symlink => "emblem-symbolic-link",
        EntryKind::File | EntryKind::Unknown => "text-x-generic",
    };
    let icon = Image::from_icon_name(icon_name);
    icon.set_pixel_size(48);
    icon.set_halign(Align::Center);
    icon.set_valign(Align::Center);
    icon
}

fn build_browser_row_icon(kind: EntryKind) -> Image {
    let icon_name = match kind {
        EntryKind::Directory => "folder-symbolic",
        EntryKind::Symlink => "emblem-symbolic-link",
        EntryKind::File | EntryKind::Unknown => "text-x-generic-symbolic",
    };
    let icon = Image::from_icon_name(icon_name);
    icon.set_pixel_size(14);
    icon.add_css_class("row-icon");
    icon
}

fn update_details(runtime: &Rc<RefCell<AppRuntime>>) {
    let selected = selected_entry(runtime);
    let (widgets, repo_root, current_branch, remote_details, staged, unstaged, untracked, dirty) = {
        let guard = runtime.borrow();
        (
            guard.widgets.clone(),
            guard.git_state.repo_root.clone(),
            guard.git_state.current_branch.clone(),
            guard.git_state.remote_details.clone(),
            guard.git_state.staged_files.len(),
            guard.git_state.unstaged_files.len(),
            guard.git_state.untracked_files.len(),
            guard.git_state.is_dirty(),
        )
    };

    match selected {
        Some(entry) => {
            widgets.entry_title_label.set_label(&format!(
                "{}  •  {}  •  {}",
                entry.display_name,
                entry.status_label(),
                entry_browser_subtitle(&entry),
            ));
            widgets
                .path_value_label
                .set_label(&entry.path.display().to_string());
            widgets.repo_value_label.set_label(
                &repo_root
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "Repository not configured".to_string()),
            );
            widgets.branch_value_label.set_label(&format!(
                "{}  •  {} staged  •  {} unstaged  •  {} untracked",
                current_branch
                    .clone()
                    .unwrap_or_else(|| "(unborn)".to_string()),
                staged,
                unstaged,
                untracked,
            ));
            widgets.remote_value_label.set_label(&format!(
                "{}{}",
                if remote_details.is_empty() {
                    "No remote configured".to_string()
                } else {
                    remote_details.join(", ")
                },
                entry
                    .warning
                    .as_ref()
                    .map(|warning| format!("  •  {}", warning))
                    .unwrap_or_default(),
            ));

            widgets
                .enable_button
                .set_sensitive(entry.managed_state != ManagedState::ManagedActive);
            widgets
                .disable_button
                .set_sensitive(entry.managed_state == ManagedState::ManagedActive);
            widgets
                .stage_button
                .set_sensitive(entry.managed_source.is_some());
            widgets
                .ignore_repo_button
                .set_sensitive(entry.managed_source.is_some());
            widgets
                .auto_commit_button
                .set_sensitive(entry.managed_source.is_some() && repo_root.is_some());
            widgets.open_live_button.set_sensitive(true);
            widgets
                .open_repo_button
                .set_sensitive(entry.managed_source.is_some());
            widgets.reveal_button.set_sensitive(true);
            widgets.commit_button.set_sensitive(staged > 0);
            widgets.stage_all_button.set_sensitive(dirty);
            widgets.push_button.set_sensitive(repo_root.is_some());
            widgets
                .open_repo_root_button
                .set_sensitive(repo_root.is_some());
            sync_repo_editor(runtime);
        }
        None => {
            widgets
                .entry_title_label
                .set_label("Select a dotfile to inspect it and open its repo workspace.");
            widgets.path_value_label.set_label("Nothing selected");
            widgets.repo_value_label.set_label(
                &repo_root
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "Repository not configured".to_string()),
            );
            widgets.branch_value_label.set_label(&format!(
                "{}  •  {} staged  •  {} unstaged  •  {} untracked",
                current_branch.unwrap_or_else(|| "-".to_string()),
                staged,
                unstaged,
                untracked,
            ));
            widgets
                .remote_value_label
                .set_label(&if remote_details.is_empty() {
                    "Select an entry to see its remote and warning details.".to_string()
                } else {
                    remote_details.join(", ")
                });
            widgets.enable_button.set_sensitive(false);
            widgets.disable_button.set_sensitive(false);
            widgets.stage_button.set_sensitive(false);
            widgets.ignore_repo_button.set_sensitive(false);
            widgets.auto_commit_button.set_sensitive(false);
            widgets.open_live_button.set_sensitive(false);
            widgets.open_repo_button.set_sensitive(false);
            widgets.reveal_button.set_sensitive(false);
            widgets.commit_button.set_sensitive(staged > 0);
            widgets.stage_all_button.set_sensitive(dirty);
            widgets.push_button.set_sensitive(repo_root.is_some());
            widgets
                .open_repo_root_button
                .set_sensitive(repo_root.is_some());
            sync_repo_editor(runtime);
        }
    }
}

fn selected_entry(runtime: &Rc<RefCell<AppRuntime>>) -> Option<DotfileEntry> {
    let guard = runtime.borrow();
    let selected_id = guard.selected_entry_id.as_ref()?;
    guard
        .report
        .entries
        .iter()
        .find(|entry| &entry.id == selected_id)
        .cloned()
}

fn enable_selected(runtime: Rc<RefCell<AppRuntime>>) {
    let Some(entry) = selected_entry(&runtime) else {
        show_message(
            &runtime.borrow().widgets.window,
            "No selection",
            "Select a dotfile first, then enable it.",
        );
        return;
    };
    if entry.managed_state == ManagedState::ManagedActive {
        show_message(
            &runtime.borrow().widgets.window,
            "Already enabled",
            "This dotfile is already active for the current profile.",
        );
        return;
    }
    let (title, body) = if entry.managed_state == ManagedState::Conflicted {
        (
            "Repair conflict",
            format!(
                "Replace the current path at {} with the managed symlink for the active profile? The existing path will be backed up first.",
                entry.path.display()
            ),
        )
    } else {
        (
            "Enable dotfile",
            format!(
                "Back up and replace {} with a managed symlink?",
                entry.path.display()
            ),
        )
    };
    let confirm = confirm_action(&runtime.borrow().widgets.window, title, &body);
    if !confirm {
        return;
    }

    let result = {
        let mut guard = runtime.borrow_mut();
        let paths = guard.paths.clone();
        let result = if entry.managed_state == ManagedState::Conflicted {
            operations::resolve_conflict_entry(&mut guard.persisted, &paths, &entry)
        } else {
            operations::enable_entry(&mut guard.persisted, &paths, &entry)
        };
        if result.is_ok() {
            guard.persisted.save(&paths).ok();
        }
        result
    };

    match result {
        Ok(_) => refresh(runtime),
        Err(error) => show_message(
            &runtime.borrow().widgets.window,
            "Failed to enable",
            &error.to_string(),
        ),
    }
}

fn disable_selected(runtime: Rc<RefCell<AppRuntime>>) {
    let Some(entry) = selected_entry(&runtime) else {
        show_message(
            &runtime.borrow().widgets.window,
            "No selection",
            "Select a dotfile first, then disable it.",
        );
        return;
    };
    if entry.managed_state != ManagedState::ManagedActive {
        show_message(
            &runtime.borrow().widgets.window,
            "Not enabled",
            "This dotfile is not currently active for the selected profile.",
        );
        return;
    }
    let confirm = confirm_action(
        &runtime.borrow().widgets.window,
        "Disable dotfile",
        &format!(
            "Restore the original file at {} from backup? The managed symlink will be removed.",
            entry.path.display()
        ),
    );
    if !confirm {
        return;
    }

    let managed_source = entry.managed_source.clone();
    let repo_root = runtime.borrow().persisted.config.repo_root.clone();

    let result = {
        let mut guard = runtime.borrow_mut();
        let paths = guard.paths.clone();
        let result = operations::disable_entry(&mut guard.persisted, &entry);
        if result.is_ok() {
            guard.persisted.save(&paths).ok();
        }
        result
    };

    match result {
        Ok(_) => {
            if let (Some(repo_root), Some(managed_source)) = (repo_root, managed_source) {
                if let Err(error) = git::remove_from_index_and_delete(&repo_root, &managed_source) {
                    eprintln!(
                        "Warning: failed to remove {} from git: {}",
                        managed_source.display(),
                        error
                    );
                }
            }
            refresh(runtime);
        }
        Err(error) => show_message(
            &runtime.borrow().widgets.window,
            "Failed to disable",
            &error.to_string(),
        ),
    }
}

fn stage_selected(runtime: Rc<RefCell<AppRuntime>>) {
    let Some(entry) = selected_entry(&runtime) else {
        show_message(
            &runtime.borrow().widgets.window,
            "No selection",
            "Select a dotfile first.",
        );
        return;
    };
    let Some(repo_root) = runtime.borrow().persisted.config.repo_root.clone() else {
        show_message(
            &runtime.borrow().widgets.window,
            "No repo",
            "Repository is not configured.",
        );
        return;
    };
    let Some(managed_source) = entry.managed_source.clone() else {
        show_message(
            &runtime.borrow().widgets.window,
            "Not managed",
            "This entry is not managed in the repo yet.",
        );
        return;
    };

    let paths = repo_paths_to_stage(&managed_source);
    match git::stage_paths(&repo_root, &paths) {
        Ok(()) => refresh(runtime),
        Err(error) => show_message(
            &runtime.borrow().widgets.window,
            "Failed to stage",
            &error.to_string(),
        ),
    }
}

fn stage_all(runtime: Rc<RefCell<AppRuntime>>) {
    let Some(repo_root) = runtime.borrow().persisted.config.repo_root.clone() else {
        show_message(
            &runtime.borrow().widgets.window,
            "No repo",
            "Repository is not configured.",
        );
        return;
    };

    match git::stage_all(&repo_root) {
        Ok(()) => refresh(runtime),
        Err(error) => show_message(
            &runtime.borrow().widgets.window,
            "Failed to stage all",
            &error.to_string(),
        ),
    }
}

fn auto_commit_selected(runtime: Rc<RefCell<AppRuntime>>) {
    let Some(entry) = selected_entry(&runtime) else {
        show_message(
            &runtime.borrow().widgets.window,
            "No selection",
            "Select a dotfile first.",
        );
        return;
    };
    let Some(repo_root) = runtime.borrow().persisted.config.repo_root.clone() else {
        show_message(
            &runtime.borrow().widgets.window,
            "No repo",
            "Repository is not configured.",
        );
        return;
    };
    let Some(managed_source) = entry.managed_source.clone() else {
        show_message(
            &runtime.borrow().widgets.window,
            "Not managed",
            "This entry is not managed in the repo yet.",
        );
        return;
    };

    let paths = repo_paths_to_stage(&managed_source);
    let result = git::stage_paths(&repo_root, &paths)
        .and_then(|()| git::commit_staged(&repo_root, &entry.display_name));

    match result {
        Ok(()) => refresh(runtime),
        Err(error) => show_message(
            &runtime.borrow().widgets.window,
            "Auto commit failed",
            &error.to_string(),
        ),
    }
}

fn prompt_commit(runtime: Rc<RefCell<AppRuntime>>) {
    let staged = runtime.borrow().git_state.staged_files.len();
    if staged == 0 {
        show_message(
            &runtime.borrow().widgets.window,
            "Nothing to commit",
            "Stage some files first.",
        );
        return;
    }

    let widgets = runtime.borrow().widgets.clone();
    let dialog = Dialog::builder()
        .title("Commit changes")
        .transient_for(&widgets.window)
        .modal(true)
        .build();
    dialog.add_button("Cancel", ResponseType::Cancel);
    dialog.add_button("Commit", ResponseType::Accept);

    let content = dialog.content_area();
    let box_ = GtkBox::new(Orientation::Vertical, 8);
    let description = Label::new(Some(&format!(
        "{} file(s) staged. Enter a commit message:",
        staged
    )));
    description.set_wrap(true);
    description.set_xalign(0.0);
    let entry = Entry::new();
    entry.set_placeholder_text(Some("Auto-committed via Doter"));
    box_.append(&description);
    box_.append(&entry);
    content.append(&box_);

    {
        let runtime = runtime.clone();
        let entry = entry.clone();
        dialog.connect_response(move |dialog, response| {
            if response == ResponseType::Accept {
                let message = entry.text().to_string();
                let message = if message.is_empty() {
                    "Auto-committed via Doter".to_string()
                } else {
                    message
                };
                let repo_root = runtime.borrow().persisted.config.repo_root.clone();
                if let Some(repo_root) = repo_root {
                    match git::commit_staged(&repo_root, &message) {
                        Ok(()) => {
                            dialog.close();
                            refresh(runtime.clone());
                        }
                        Err(error) => {
                            show_message(
                                &runtime.borrow().widgets.window,
                                "Commit failed",
                                &error.to_string(),
                            );
                        }
                    }
                }
            } else {
                dialog.close();
            }
        });
    }

    dialog.present();
}

fn push_current_branch(runtime: Rc<RefCell<AppRuntime>>) {
    let Some(repo_root) = runtime.borrow().persisted.config.repo_root.clone() else {
        show_message(
            &runtime.borrow().widgets.window,
            "No repo",
            "Repository is not configured.",
        );
        return;
    };

    {
        let mut guard = runtime.borrow_mut();
        if guard.sync_in_progress {
            return;
        }
        guard.sync_in_progress = true;
        guard.widgets.push_button.set_sensitive(false);
        guard.widgets.sync_button.set_sensitive(false);
        guard
            .widgets
            .repo_editor_status
            .set_label("Pushing current branch to remote...");
    }

    let branch = runtime.borrow().git_state.current_branch.clone();
    let remote = active_remote_name(&runtime);
    let branch = branch.unwrap_or_else(|| "main".to_string());
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result = git::push_current_branch(&repo_root, &remote, &branch)
            .map_err(|error| error.to_string());
        let _ = sender.send(result);
    });

    glib::timeout_add_local(Duration::from_millis(100), move || {
        match receiver.try_recv() {
            Ok(Ok(())) => {
                {
                    let mut guard = runtime.borrow_mut();
                    guard.sync_in_progress = false;
                    guard.widgets.push_button.set_sensitive(true);
                    guard.widgets.sync_button.set_sensitive(true);
                }
                let widgets = runtime.borrow().widgets.clone();
                present_sync_notice(
                    &widgets,
                    "Push complete",
                    "Branch pushed successfully.",
                    false,
                );
                refresh(runtime.clone());
                ControlFlow::Break
            }
            Ok(Err(error)) => {
                {
                    let mut guard = runtime.borrow_mut();
                    guard.sync_in_progress = false;
                    guard.widgets.push_button.set_sensitive(true);
                    guard.widgets.sync_button.set_sensitive(true);
                }
                let widgets = runtime.borrow().widgets.clone();
                present_sync_notice(&widgets, "Push failed", &error, true);
                refresh(runtime.clone());
                ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => {
                {
                    let mut guard = runtime.borrow_mut();
                    guard.sync_in_progress = false;
                    guard.widgets.push_button.set_sensitive(true);
                    guard.widgets.sync_button.set_sensitive(true);
                }
                let widgets = runtime.borrow().widgets.clone();
                present_sync_notice(
                    &widgets,
                    "Push failed",
                    "The background push task exited unexpectedly.",
                    true,
                );
                refresh(runtime.clone());
                ControlFlow::Break
            }
        }
    });
}

fn sync_with_remote(runtime: Rc<RefCell<AppRuntime>>) {
    let Some(repo_root) = runtime.borrow().persisted.config.repo_root.clone() else {
        show_message(
            &runtime.borrow().widgets.window,
            "No repo",
            "Repository is not configured.",
        );
        return;
    };

    {
        let mut guard = runtime.borrow_mut();
        if guard.sync_in_progress {
            return;
        }
        guard.sync_in_progress = true;
        guard.widgets.sync_button.set_sensitive(false);
        guard.widgets.push_button.set_sensitive(false);
        guard
            .widgets
            .repo_editor_status
            .set_label("Syncing repository with remote...");
    }

    let remote = active_remote_name(&runtime);
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result = git::sync_with_remote(&repo_root, &remote).map_err(|error| error.to_string());
        let _ = sender.send(result);
    });

    glib::timeout_add_local(Duration::from_millis(100), move || {
        match receiver.try_recv() {
            Ok(Ok(outcome)) => {
                {
                    let mut guard = runtime.borrow_mut();
                    guard.sync_in_progress = false;
                    guard.widgets.sync_button.set_sensitive(true);
                    guard.widgets.push_button.set_sensitive(true);
                }
                let widgets = runtime.borrow().widgets.clone();
                let body = format!(
                    "Fetched: {} | Pulled: {} | Pushed: {}",
                    yes_no(outcome.fetched),
                    yes_no(outcome.pulled),
                    yes_no(outcome.pushed),
                );
                present_sync_notice(&widgets, "Sync complete", &body, false);
                refresh(runtime.clone());
                ControlFlow::Break
            }
            Ok(Err(error)) => {
                {
                    let mut guard = runtime.borrow_mut();
                    guard.sync_in_progress = false;
                    guard.widgets.sync_button.set_sensitive(true);
                    guard.widgets.push_button.set_sensitive(true);
                }
                let widgets = runtime.borrow().widgets.clone();
                present_sync_notice(&widgets, "Sync failed", &error, true);
                refresh(runtime.clone());
                ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => {
                {
                    let mut guard = runtime.borrow_mut();
                    guard.sync_in_progress = false;
                    guard.widgets.sync_button.set_sensitive(true);
                    guard.widgets.push_button.set_sensitive(true);
                }
                let widgets = runtime.borrow().widgets.clone();
                present_sync_notice(
                    &widgets,
                    "Sync failed",
                    "The background sync task exited unexpectedly.",
                    true,
                );
                refresh(runtime.clone());
                ControlFlow::Break
            }
        }
    });
}

fn active_remote_name(runtime: &Rc<RefCell<AppRuntime>>) -> String {
    let guard = runtime.borrow();
    if !guard.persisted.config.remote_name.trim().is_empty() {
        return guard.persisted.config.remote_name.clone();
    }
    guard
        .git_state
        .remotes
        .first()
        .cloned()
        .unwrap_or_else(|| "origin".to_string())
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

fn prompt_new_profile(runtime: Rc<RefCell<AppRuntime>>) {
    let widgets = runtime.borrow().widgets.clone();
    let dialog = Dialog::builder()
        .title("New profile")
        .transient_for(&widgets.window)
        .modal(true)
        .build();
    dialog.add_button("Cancel", ResponseType::Cancel);
    dialog.add_button("Create", ResponseType::Accept);

    let content = dialog.content_area();
    let box_ = GtkBox::new(Orientation::Vertical, 8);
    let description = Label::new(Some("Enter a name for the new profile:"));
    description.set_wrap(true);
    description.set_xalign(0.0);
    let entry = Entry::new();
    entry.set_placeholder_text(Some("my-profile"));
    box_.append(&description);
    box_.append(&entry);
    content.append(&box_);

    {
        let runtime = runtime.clone();
        let entry = entry.clone();
        dialog.connect_response(move |dialog, response| {
            if response == ResponseType::Accept {
                let name = entry.text().to_string();
                if name.is_empty() {
                    show_message(
                        &runtime.borrow().widgets.window,
                        "Invalid name",
                        "Profile name cannot be empty.",
                    );
                    return;
                }
                {
                    let mut guard = runtime.borrow_mut();
                    if guard.persisted.config.profiles.iter().any(|p| *p == name) {
                        show_message(
                            &runtime.borrow().widgets.window,
                            "Profile exists",
                            "A profile with this name already exists.",
                        );
                        return;
                    }
                    if let Some(repo_root) = guard.persisted.config.repo_root.clone() {
                        let profile_root = repo_root.join("profiles").join(&name);
                        if let Err(error) = fs::create_dir_all(&profile_root) {
                            show_message(
                                &runtime.borrow().widgets.window,
                                "Create profile failed",
                                &format!(
                                    "Failed to create {}: {}",
                                    profile_root.display(),
                                    error
                                ),
                            );
                            return;
                        }
                    }
                    guard.persisted.config.profiles.push(name.clone());
                    guard.persisted.config.active_profile = name.clone();
                    guard.persisted.config.ensure_active_profile();
                    let _ = guard.persisted.save(&guard.paths);
                }
                dialog.close();
                sync_profile_combo(&runtime);
                refresh(runtime.clone());
            } else {
                dialog.close();
            }
        });
    }

    dialog.present();
}

fn prompt_copy_profile(runtime: Rc<RefCell<AppRuntime>>) {
    let widgets = runtime.borrow().widgets.clone();
    let profiles = runtime.borrow().persisted.config.profiles.clone();
    if profiles.len() < 2 {
        show_message(
            &widgets.window,
            "Not enough profiles",
            "Create another profile first, then copy dotfiles into it.",
        );
        return;
    }

    let active_profile = runtime.borrow().persisted.config.active_profile.clone();
    let dialog = Dialog::builder()
        .title("Copy profile dotfiles")
        .transient_for(&widgets.window)
        .modal(true)
        .build();
    dialog.add_button("Cancel", ResponseType::Cancel);
    dialog.add_button("Continue", ResponseType::Accept);

    let content = dialog.content_area();
    let box_ = GtkBox::new(Orientation::Vertical, 8);
    let description = Label::new(Some(
        "Copy managed dotfiles from one profile into another. If the destination already has files at the same repo paths, you will choose whether to keep them or overwrite them before anything is changed.",
    ));
    description.set_wrap(true);
    description.set_xalign(0.0);
    let source_combo = ComboBoxText::new();
    let destination_combo = ComboBoxText::new();
    for profile in &profiles {
        source_combo.append_text(profile);
        destination_combo.append_text(profile);
    }
    let active_index = profiles
        .iter()
        .position(|profile| profile == &active_profile)
        .map(|index| index as u32);
    source_combo.set_active(active_index);
    let destination_index = profiles
        .iter()
        .position(|profile| profile != &active_profile)
        .map(|index| index as u32)
        .or(active_index);
    destination_combo.set_active(destination_index);

    let source_label = Label::new(Some("Source profile"));
    source_label.set_xalign(0.0);
    let destination_label = Label::new(Some("Destination profile"));
    destination_label.set_xalign(0.0);
    box_.append(&description);
    box_.append(&source_label);
    box_.append(&source_combo);
    box_.append(&destination_label);
    box_.append(&destination_combo);
    content.append(&box_);

    {
        let runtime = runtime.clone();
        let source_combo = source_combo.clone();
        let destination_combo = destination_combo.clone();
        dialog.connect_response(move |dialog, response| {
            if response != ResponseType::Accept {
                dialog.close();
                return;
            }

            let source = source_combo
                .active_text()
                .map(|text| text.to_string())
                .unwrap_or_default();
            let destination = destination_combo
                .active_text()
                .map(|text| text.to_string())
                .unwrap_or_default();
            if source.is_empty() || destination.is_empty() {
                show_message(
                    &runtime.borrow().widgets.window,
                    "Choose profiles",
                    "Select both a source profile and a destination profile.",
                );
                return;
            }
            if source == destination {
                show_message(
                    &runtime.borrow().widgets.window,
                    "Choose different profiles",
                    "The source and destination profiles must be different.",
                );
                return;
            }

            let preview = {
                let guard = runtime.borrow();
                operations::preview_profile_copy(&guard.persisted, &source, &destination)
            };
            let preview = match preview {
                Ok(preview) => preview,
                Err(error) => {
                    show_message(
                        &runtime.borrow().widgets.window,
                        "Copy preview failed",
                        &error.to_string(),
                    );
                    return;
                }
            };

            let mode = if preview.conflict_paths.is_empty() {
                let confirm = confirm_action(
                    &runtime.borrow().widgets.window,
                    "Copy profile",
                    &format!(
                        "Copy {} managed entr{} from '{}' into '{}'? No destination repo paths will be overwritten.",
                        preview.managed_entries,
                        if preview.managed_entries == 1 { "y" } else { "ies" },
                        source,
                        destination
                    ),
                );
                if !confirm {
                    return;
                }
                operations::ProfileCopyMode::KeepExisting
            } else {
                let choices = present_profile_copy_conflict_dialog(
                    &runtime.borrow().widgets.window,
                    &source,
                    &destination,
                    &preview.conflict_paths,
                );
                let Some(mode) = choices else {
                    return;
                };
                mode
            };

            dialog.close();
            run_profile_copy(runtime.clone(), source, destination, mode);
        });
    }

    dialog.present();
}

fn present_profile_copy_conflict_dialog(
    window: &ApplicationWindow,
    source: &str,
    destination: &str,
    conflict_paths: &[PathBuf],
) -> Option<operations::ProfileCopyMode> {
    let dialog = Dialog::builder()
        .title("Destination already has dotfiles")
        .transient_for(window)
        .modal(true)
        .build();
    dialog.add_button("Cancel", ResponseType::Cancel);
    dialog.add_button("Keep Existing", ResponseType::Reject);
    dialog.add_button("Overwrite Existing", ResponseType::Accept);

    let content = dialog.content_area();
    let box_ = GtkBox::new(Orientation::Vertical, 8);
    let preview_lines = conflict_paths
        .iter()
        .take(8)
        .map(|path| format!("- {}", path.display()))
        .collect::<Vec<_>>();
    let mut body = format!(
        "Copying '{}' into '{}' would touch {} existing destination path(s).\n\nKeep Existing: skip those paths and leave the destination copy as-is.\nOverwrite Existing: replace those destination paths with the source profile copy.",
        source,
        destination,
        conflict_paths.len()
    );
    if !preview_lines.is_empty() {
        body.push_str("\n\nExamples:\n");
        body.push_str(&preview_lines.join("\n"));
        if conflict_paths.len() > preview_lines.len() {
            body.push_str(&format!(
                "\n... and {} more",
                conflict_paths.len() - preview_lines.len()
            ));
        }
    }

    let label = Label::new(Some(&body));
    label.set_wrap(true);
    label.set_xalign(0.0);
    content.append(&box_);
    box_.append(&label);

    let response = glib::MainContext::default().block_on(dialog.run_future());
    dialog.close();
    match response {
        ResponseType::Accept => Some(operations::ProfileCopyMode::OverwriteExisting),
        ResponseType::Reject => Some(operations::ProfileCopyMode::KeepExisting),
        _ => None,
    }
}

fn run_profile_copy(
    runtime: Rc<RefCell<AppRuntime>>,
    source: String,
    destination: String,
    mode: operations::ProfileCopyMode,
) {
    let result = {
        let mut guard = runtime.borrow_mut();
        let paths = guard.paths.clone();
        let result = operations::copy_profile(&mut guard.persisted, &source, &destination, mode);
        if result.is_ok() {
            let _ = guard.persisted.save(&paths);
        }
        result
    };

    match result {
        Ok(result) => {
            let widgets = runtime.borrow().widgets.clone();
            present_sync_notice(&widgets, "Profile copied", &result.message, false);
            sync_profile_combo(&runtime);
            refresh(runtime);
        }
        Err(error) => show_message(
            &runtime.borrow().widgets.window,
            "Copy profile failed",
            &error.to_string(),
        ),
    }
}

fn remove_active_profile(runtime: Rc<RefCell<AppRuntime>>) {
    let active = runtime.borrow().persisted.config.active_profile.clone();
    let confirm = confirm_action(
        &runtime.borrow().widgets.window,
        "Remove profile",
        &format!(
            "Remove the profile '{}'? This will delete all its managed entries from the repo.",
            active
        ),
    );
    if !confirm {
        return;
    }

    let result = {
        let mut guard = runtime.borrow_mut();
        let paths = guard.paths.clone();
        let result = operations::remove_profile(&mut guard.persisted, &paths, &active);
        if result.is_ok() {
            let _ = guard.persisted.save(&paths);
        }
        result
    };

    if let Err(error) = result {
        show_message(
            &runtime.borrow().widgets.window,
            "Remove profile failed",
            &error.to_string(),
        );
        return;
    }

    sync_profile_combo(&runtime);
    refresh(runtime);
}

fn sync_profile_combo(runtime: &Rc<RefCell<AppRuntime>>) {
    let widgets = runtime.borrow().widgets.clone();
    let profiles = runtime.borrow().persisted.config.profiles.clone();
    let active = runtime.borrow().persisted.config.active_profile.clone();

    widgets.profile_combo.remove_all();

    for profile in &profiles {
        widgets.profile_combo.append_text(profile);
    }

    let active_index = profiles
        .iter()
        .position(|profile| profile == &active)
        .map(|index| index as u32);
    widgets.profile_combo.set_active(active_index);
}

fn open_repo_root(runtime: Rc<RefCell<AppRuntime>>) {
    let Some(repo_root) = runtime.borrow().persisted.config.repo_root.clone() else {
        show_message(
            &runtime.borrow().widgets.window,
            "No repo",
            "Repository is not configured.",
        );
        return;
    };

    let _ = Command::new("xdg-open").arg(&repo_root).spawn();
}

fn open_selected_live(runtime: Rc<RefCell<AppRuntime>>) {
    let Some(entry) = selected_entry(&runtime) else {
        return;
    };

    let _ = Command::new("xdg-open").arg(&entry.path).spawn();
}

fn open_selected_repo(runtime: Rc<RefCell<AppRuntime>>) {
    let selected = selected_entry(&runtime);
    let managed_source = selected.and_then(|entry| entry.managed_source.clone());
    let selected_repo_file = runtime.borrow().selected_repo_file.clone();

    let Some(repo_path) = selected_repo_file.or(managed_source) else {
        show_message(
            &runtime.borrow().widgets.window,
            "Not managed",
            "This entry is not managed in the repo yet.",
        );
        return;
    };

    let _ = Command::new("xdg-open").arg(&repo_path).spawn();
}

fn ignore_selected_repo_path(runtime: Rc<RefCell<AppRuntime>>) {
    let widgets = runtime.borrow().widgets.clone();
    let (managed_source, active_repo_row_path) = {
        let guard = runtime.borrow();
        (
            selected_entry(&runtime).and_then(|entry| entry.managed_source.clone()),
            guard.active_repo_row_path.clone(),
        )
    };

    let Some(managed_source) = managed_source else {
        show_message(
            &widgets.window,
            "Not managed",
            "This entry is not managed in the repo yet.",
        );
        return;
    };

    let Some(gitignore_path) = gitignore_path_for_source(&managed_source) else {
        show_message(
            &widgets.window,
            "No gitignore path",
            "Unable to determine where the profile gitignore should live.",
        );
        return;
    };

    let target_path = active_repo_row_path.unwrap_or_else(|| managed_source.clone());
    if target_path == gitignore_path {
        show_message(
            &widgets.window,
            "Select a target",
            "Select a file or folder to ignore, not the .gitignore file itself.",
        );
        return;
    }

    let Some(pattern) = ignore_pattern_for_path(&managed_source, &target_path) else {
        show_message(
            &widgets.window,
            "Ignore failed",
            "Only files and folders inside the selected dotfile profile can be ignored.",
        );
        return;
    };

    if !gitignore_path.exists() {
        if let Some(parent) = gitignore_path.parent() {
            if let Err(error) = fs::create_dir_all(parent) {
                show_message(
                    &widgets.window,
                    "Gitignore failed",
                    &format!("Failed to create {}: {}", parent.display(), error),
                );
                return;
            }
        }
        if let Err(error) = fs::write(&gitignore_path, "") {
            show_message(
                &widgets.window,
                "Gitignore failed",
                &format!("Failed to create {}: {}", gitignore_path.display(), error),
            );
            return;
        }
    }

    let existing = match fs::read_to_string(&gitignore_path) {
        Ok(content) => content,
        Err(error) if error.kind() == ErrorKind::InvalidData => {
            show_message(
                &widgets.window,
                "Gitignore failed",
                &format!("{} is not valid UTF-8.", gitignore_path.display()),
            );
            return;
        }
        Err(error) => {
            show_message(
                &widgets.window,
                "Gitignore failed",
                &format!("Failed to read {}: {}", gitignore_path.display(), error),
            );
            return;
        }
    };

    let Some(updated) = append_gitignore_rule(&existing, &pattern) else {
        present_sync_notice(
            &widgets,
            "Already ignored",
            &format!(
                "{} is already ignored by {}.",
                repo_file_label(&managed_source, &target_path),
                repo_file_label(&managed_source, &gitignore_path)
            ),
            false,
        );
        return;
    };

    if let Err(error) = fs::write(&gitignore_path, updated) {
        show_message(
            &widgets.window,
            "Gitignore failed",
            &format!("Failed to update {}: {}", gitignore_path.display(), error),
        );
        return;
    }

    {
        let mut guard = runtime.borrow_mut();
        guard.selected_repo_file = Some(gitignore_path.clone());
        guard.active_repo_row_path = Some(gitignore_path.clone());
    }
    sync_repo_editor(&runtime);
    present_sync_notice(
        &widgets,
        "Updated gitignore",
        &format!(
            "Added {} to {}.",
            pattern,
            repo_file_label(&managed_source, &gitignore_path)
        ),
        false,
    );
}

fn reveal_selected(runtime: Rc<RefCell<AppRuntime>>) {
    let Some(entry) = selected_entry(&runtime) else {
        return;
    };

    let path = entry.path.parent().unwrap_or(&entry.path);
    let _ = Command::new("xdg-open").arg(path).spawn();
}

fn reload_repo_editor_action(runtime: Rc<RefCell<AppRuntime>>) {
    refresh_active_workspace_view(&runtime);
}

fn save_repo_editor(runtime: Rc<RefCell<AppRuntime>>) {
    let (widgets, selected_repo_file, loaded_repo_content) = {
        let guard = runtime.borrow();
        (
            guard.widgets.clone(),
            guard.selected_repo_file.clone(),
            guard.loaded_repo_content.clone(),
        )
    };

    let Some(file_path) = selected_repo_file else {
        show_message(&widgets.window, "No file", "No file selected.");
        return;
    };

    let Some(original_content) = loaded_repo_content else {
        show_message(&widgets.window, "No content", "No content to save.");
        return;
    };

    let buffer = widgets.repo_editor_view.buffer();
    let start = buffer.start_iter();
    let end = buffer.end_iter();
    let current_content = buffer.text(&start, &end, false).to_string();

    if current_content == original_content {
        widgets.repo_editor_status.set_label("No changes to save.");
        return;
    }

    let confirm = confirm_action(
        &widgets.window,
        "Save changes",
        &format!("Save changes to {}?", file_path.display()),
    );
    if !confirm {
        return;
    }

    match fs::write(&file_path, &current_content) {
        Ok(()) => {
            {
                let mut guard = runtime.borrow_mut();
                guard.loaded_repo_content = Some(current_content);
            }
            refresh(runtime);
        }
        Err(error) => show_message(
            &widgets.window,
            "Save failed",
            &format!("Failed to write file: {}", error),
        ),
    }
}

fn set_selected_entry_from_row(runtime: &Rc<RefCell<AppRuntime>>, row: Option<&ListBoxRow>) {
    let Some(row) = row else {
        runtime.borrow_mut().selected_entry_id = None;
        return;
    };

    let index = row.index();
    if index < 0 {
        runtime.borrow_mut().selected_entry_id = None;
        return;
    }

    let entry_id = {
        let guard = runtime.borrow();
        browser_entries(
            &guard.report,
            &guard.filter_text,
            guard.scope_filter,
            guard.kind_filter,
            guard.sort_mode,
        )
        .get(index as usize)
        .map(|entry| entry.id.clone())
    };
    runtime.borrow_mut().selected_entry_id = entry_id;
}

fn set_selected_entry_from_grid(runtime: &Rc<RefCell<AppRuntime>>, child: Option<&FlowBoxChild>) {
    let Some(child) = child else {
        runtime.borrow_mut().selected_entry_id = None;
        return;
    };

    let index = child.index();
    if index < 0 {
        runtime.borrow_mut().selected_entry_id = None;
        return;
    }

    let entry_id = {
        let guard = runtime.borrow();
        browser_entries(
            &guard.report,
            &guard.filter_text,
            guard.scope_filter,
            guard.kind_filter,
            guard.sort_mode,
        )
        .get(index as usize)
        .map(|entry| entry.id.clone())
    };
    runtime.borrow_mut().selected_entry_id = entry_id;
}

fn set_selected_repo_file_from_row(
    runtime: &Rc<RefCell<AppRuntime>>,
    row: Option<&ListBoxRow>,
) -> RepoSelectionAction {
    if runtime.borrow().repo_explorer_updating {
        return RepoSelectionAction::None;
    }

    let Some(row) = row else {
        return RepoSelectionAction::None;
    };

    let index = row.index();
    if index < 0 {
        return RepoSelectionAction::None;
    }

    let row_data = {
        let guard = runtime.borrow();
        guard.repo_rows.get(index as usize).cloned()
    };

    let Some(row_data) = row_data else {
        return RepoSelectionAction::None;
    };

    if row_data.is_dir {
        {
            let mut guard = runtime.borrow_mut();
            guard.active_repo_row_path = Some(row_data.path.clone());
        }
        return RepoSelectionAction::None;
    }

    {
        let mut guard = runtime.borrow_mut();
        guard.active_repo_row_path = Some(row_data.path.clone());
        guard.selected_repo_file = Some(row_data.path.clone());
        if let Some(root) = guard.repo_root_path.clone() {
            ensure_expanded_dirs(&mut guard.expanded_repo_dirs, &root, Some(&row_data.path));
        }
    }
    RepoSelectionAction::LoadFile
}

fn activate_repo_row(runtime: &Rc<RefCell<AppRuntime>>, row: &ListBoxRow) -> RepoSelectionAction {
    if runtime.borrow().repo_explorer_updating {
        return RepoSelectionAction::None;
    }

    let index = row.index();
    if index < 0 {
        return RepoSelectionAction::None;
    }

    let row_data = {
        let guard = runtime.borrow();
        guard.repo_rows.get(index as usize).cloned()
    };

    let Some(row_data) = row_data else {
        return RepoSelectionAction::None;
    };

    if row_data.is_dir {
        let mut guard = runtime.borrow_mut();
        guard.active_repo_row_path = Some(row_data.path.clone());
        if row_data.expanded {
            guard.expanded_repo_dirs.remove(&row_data.path);
        } else {
            guard.expanded_repo_dirs.insert(row_data.path.clone());
        }
        return RepoSelectionAction::RefreshTree;
    }

    RepoSelectionAction::None
}

fn show_message(window: &ApplicationWindow, title: &str, body: &str) {
    let dialog = Dialog::builder()
        .title(title)
        .transient_for(window)
        .modal(true)
        .build();
    dialog.add_button("OK", ResponseType::Ok);

    let content = dialog.content_area();
    let label = Label::new(Some(body));
    label.set_wrap(true);
    label.set_xalign(0.0);
    content.append(&label);

    dialog.connect_response(|dialog, _| dialog.close());
    dialog.present();
}

fn confirm_action(window: &ApplicationWindow, title: &str, body: &str) -> bool {
    let dialog = Dialog::builder()
        .title(title)
        .transient_for(window)
        .modal(true)
        .build();
    dialog.add_button("Cancel", ResponseType::Cancel);
    dialog.add_button("Continue", ResponseType::Accept);
    let content = dialog.content_area();
    let label = Label::new(Some(body));
    label.set_wrap(true);
    label.set_xalign(0.0);
    content.append(&label);
    let response = glib::MainContext::default().block_on(dialog.run_future());
    dialog.close();
    response == ResponseType::Accept
}

#[cfg(test)]
mod tests {
    use super::{
        append_gitignore_rule, build_repo_explorer_rows, gitignore_path_for_source,
        ignore_pattern_for_path, repo_file_label, repo_files_for_source, repo_paths_to_stage,
    };
    use std::collections::BTreeSet;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn collects_repo_files_recursively_for_directories() {
        let tempdir = tempdir().unwrap();
        let root = tempdir.path().join("profiles/default/config/nvim");
        fs::create_dir_all(root.join("lua/plugins")).unwrap();
        fs::write(root.join("init.lua"), "return {}").unwrap();
        fs::write(root.join("lua/plugins/example.lua"), "-- plugin").unwrap();

        let files = repo_files_for_source(&root).unwrap();

        assert_eq!(files.len(), 2);
        assert_eq!(repo_file_label(&root, &files[0]), "init.lua");
        assert_eq!(repo_file_label(&root, &files[1]), "lua/plugins/example.lua");
    }

    #[test]
    fn builds_folder_rows_for_explorer() {
        let tempdir = tempdir().unwrap();
        let root = tempdir.path().join("profiles/default/config/nvim");
        fs::create_dir_all(root.join("lua/plugins")).unwrap();
        fs::write(root.join("init.lua"), "return {}").unwrap();
        fs::write(root.join("lua/plugins/example.lua"), "-- plugin").unwrap();

        let mut expanded = BTreeSet::new();
        expanded.insert(root.clone());
        expanded.insert(root.join("lua"));

        let rows = build_repo_explorer_rows(&root, &expanded).unwrap();
        let labels = rows
            .iter()
            .map(|row| row.label.as_str())
            .collect::<Vec<_>>();

        assert_eq!(labels, vec!["lua", "plugins", "init.lua"]);
    }

    #[test]
    fn includes_profile_gitignore_for_file_entries() {
        let tempdir = tempdir().unwrap();
        let root = tempdir.path().join("profiles/default/home/.zshrc");
        let gitignore = root.parent().unwrap().join(".gitignore");
        fs::create_dir_all(root.parent().unwrap()).unwrap();
        fs::write(&root, "export EDITOR=nvim\n").unwrap();
        fs::write(&gitignore, "*.zwc\n").unwrap();

        let files = repo_files_for_source(&root).unwrap();
        let labels = files
            .iter()
            .map(|path| repo_file_label(&root, path))
            .collect::<Vec<_>>();

        assert_eq!(labels, vec![".gitignore", ".zshrc"]);
        assert_eq!(gitignore_path_for_source(&root), Some(gitignore));
    }

    #[test]
    fn adds_profile_gitignore_row_for_file_entries() {
        let tempdir = tempdir().unwrap();
        let root = tempdir.path().join("profiles/default/home/.zshrc");
        let gitignore = root.parent().unwrap().join(".gitignore");
        fs::create_dir_all(root.parent().unwrap()).unwrap();
        fs::write(&root, "export EDITOR=nvim\n").unwrap();
        fs::write(&gitignore, "*.zwc\n").unwrap();

        let rows = build_repo_explorer_rows(&root, &BTreeSet::new()).unwrap();
        let labels = rows
            .iter()
            .map(|row| row.label.as_str())
            .collect::<Vec<_>>();

        assert_eq!(labels, vec![".zshrc", ".gitignore"]);
    }

    #[test]
    fn builds_ignore_pattern_for_nested_paths() {
        let tempdir = tempdir().unwrap();
        let root = tempdir.path().join("profiles/default/config/nvim");
        let plugins = root.join("lua/plugins");
        let plugin = plugins.join("example.lua");
        fs::create_dir_all(&plugins).unwrap();
        fs::write(&plugin, "-- plugin").unwrap();

        assert_eq!(
            ignore_pattern_for_path(&root, &plugins),
            Some("/lua/plugins/".to_string())
        );
        assert_eq!(
            ignore_pattern_for_path(&root, &plugin),
            Some("/lua/plugins/example.lua".to_string())
        );
    }

    #[test]
    fn builds_ignore_pattern_for_single_file_entry() {
        let tempdir = tempdir().unwrap();
        let root = tempdir.path().join("profiles/default/home/.zshrc");
        fs::create_dir_all(root.parent().unwrap()).unwrap();
        fs::write(&root, "export EDITOR=nvim\n").unwrap();

        assert_eq!(
            ignore_pattern_for_path(&root, &root),
            Some("/.zshrc".to_string())
        );
    }

    #[test]
    fn appends_gitignore_rule_once() {
        assert_eq!(
            append_gitignore_rule("target/\n", "/cache/"),
            Some("target/\n/cache/\n".to_string())
        );
        assert_eq!(append_gitignore_rule("target/\n", "target/"), None);
    }

    #[test]
    fn stages_profile_gitignore_for_single_file_entries() {
        let tempdir = tempdir().unwrap();
        let root = tempdir.path().join("profiles/default/home/.zshrc");
        let gitignore = root.parent().unwrap().join(".gitignore");
        fs::create_dir_all(root.parent().unwrap()).unwrap();
        fs::write(&root, "export EDITOR=nvim\n").unwrap();
        fs::write(&gitignore, "*.zwc\n").unwrap();

        let paths = repo_paths_to_stage(&root);

        assert_eq!(paths, vec![root, gitignore]);
    }
}
