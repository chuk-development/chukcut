//! The start screen: what the window shows when no project is open.
//!
//! Decision 0004: the app no longer makes a project for the user at launch.
//! This screen offers a new one (canvas shape, frame rate, name), Open…, and
//! the recent list — CapCut's home, in our colours. It does not create or
//! open anything itself; it emits a [`HomeEvent`] and the shell does it, so
//! the start screen and the editor's menu share one path into a document.

use std::collections::HashMap;

use chukcut_engine::modules::media::thumbnail_strip;
use chukcut_engine::modules::project::recovery::RecoveryInfo;
use chukcut_engine::modules::workspace::commands::{self as workspace_commands, RecentEntry};
use gpui::assets::IconName as Lucide;
use gpui::component::button::{Button, ButtonVariants as _};
use gpui::component::input::{Input, InputState};
use gpui::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui::component::{Icon, Sizable as _};
use gpui::{AnyElement, Entity, EventEmitter, ObjectFit};

use super::settings::{fps_label, CANVAS_PRESETS, FRAME_RATES};
use super::*;

pub(crate) enum HomeEvent {
    Create {
        name: String,
        width: u32,
        height: u32,
        fps: f64,
    },
    Open(PathBuf),
    /// Open the file dialog for a project.
    Browse,
    Restore,
    DiscardRecovery,
}

enum Poster {
    Loading,
    Ready(PathBuf),
    Failed,
}

pub(crate) struct Home {
    focus: FocusHandle,
    /// `None` while the list loads.
    recent: Option<Vec<RecentEntry>>,
    posters: HashMap<String, Poster>,
    preset: usize,
    fps: f64,
    name: Entity<InputState>,
    pub(crate) recovery: Option<RecoveryInfo>,
    /// The last thing that went wrong, e.g. a project that would not open.
    pub(crate) notice: Option<SharedString>,
}

impl EventEmitter<HomeEvent> for Home {}

impl Home {
    pub(crate) fn new(
        recovery: Option<RecoveryInfo>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let settings = workspace_commands::workspace_settings_get();
        let preset = CANVAS_PRESETS
            .iter()
            .position(|(_, w, h)| (*w, *h) == settings.default_canvas)
            .unwrap_or(0);
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("Untitled"));
        let mut home = Self {
            focus: cx.focus_handle(),
            recent: None,
            posters: HashMap::new(),
            preset,
            fps: settings.default_fps,
            name,
            recovery,
            notice: None,
        };
        home.reload(cx);
        home
    }

    pub(crate) fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    /// Read the recent list again, off the UI thread: it opens every file.
    pub(crate) fn reload(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let entries = cx
                .background_executor()
                .spawn(async { workspace_commands::workspace_recent_entries() })
                .await;
            let _ = this.update(cx, |home, cx| {
                home.recent = Some(entries);
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn create(&mut self, cx: &mut Context<Self>) {
        let (_, width, height) = CANVAS_PRESETS[self.preset];
        let typed = self.name.read(cx).value().trim().to_string();
        let name = if typed.is_empty() {
            "Untitled".to_string()
        } else {
            typed
        };
        cx.emit(HomeEvent::Create {
            name,
            width,
            height,
            fps: self.fps,
        });
    }

    fn forget(&mut self, path: String, cx: &mut Context<Self>) {
        if let Err(error) = workspace_commands::workspace_recent_forget(path) {
            self.notice = Some(error.into());
        }
        self.reload(cx);
    }

    fn poster(&mut self, entry: &RecentEntry, cx: &mut Context<Self>) -> AnyElement {
        let placeholder = |icon: Lucide| {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(TEXT_DIM))
                .child(Icon::new(icon).size(px(26.0)))
                .into_any_element()
        };
        let Some(poster) = &entry.poster else {
            return placeholder(Lucide::Clapperboard);
        };
        if poster.still {
            return img(PathBuf::from(&poster.path))
                .size_full()
                .object_fit(ObjectFit::Cover)
                .into_any_element();
        }
        match self.posters.get(&poster.path) {
            Some(Poster::Ready(frame)) => img(frame.clone())
                .size_full()
                .object_fit(ObjectFit::Cover)
                .into_any_element(),
            Some(Poster::Failed) => placeholder(Lucide::Film),
            Some(Poster::Loading) => placeholder(Lucide::Film),
            None => {
                let path = poster.path.clone();
                self.posters.insert(path.clone(), Poster::Loading);
                cx.spawn(async move |this, cx| {
                    let source = path.clone();
                    // Cached on disk by the engine: the second launch is free.
                    let strip = cx
                        .background_executor()
                        .spawn(async move { thumbnail_strip(&source, 1, 180) })
                        .await;
                    let poster = match strip {
                        Ok(frames) if !frames.is_empty() => {
                            Poster::Ready(PathBuf::from(&frames[0]))
                        }
                        _ => Poster::Failed,
                    };
                    let _ = this.update(cx, |home, cx| {
                        home.posters.insert(path, poster);
                        cx.notify();
                    });
                })
                .detach();
                placeholder(Lucide::Film)
            }
        }
    }

    fn render_sidebar(&self) -> impl IntoElement {
        let nav = |id: &'static str, icon: Lucide, label: &'static str, active: bool| {
            div()
                .id(id)
                .h(px(36.0))
                .px_3()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .rounded_md()
                .text_sm()
                .cursor_pointer()
                .when(active, |item| {
                    item.bg(rgb(PANEL_RAISED)).text_color(rgb(TEXT))
                })
                .when(!active, |item| {
                    item.text_color(rgb(TEXT_DIM))
                        .hover(|style| style.bg(rgb(PANEL_RAISED)))
                })
                .child(Icon::new(icon).size(px(16.0)))
                .child(label)
        };
        div()
            .w(px(220.0))
            .flex_none()
            .flex()
            .flex_col()
            .gap_1()
            .p_3()
            .bg(rgb(PANEL))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .pt_1()
                    .pb_4()
                    .child(
                        div()
                            .size(px(26.0))
                            .rounded(px(6.0))
                            .bg(rgb(ACCENT))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                Icon::new(Lucide::Scissors)
                                    .size(px(16.0))
                                    .text_color(rgb(0x0b1214)),
                            ),
                    )
                    .child(
                        div()
                            .text_base()
                            .font_weight(gpui::FontWeight::BOLD)
                            .child("chukcut"),
                    ),
            )
            .child(nav("home-nav-home", Lucide::House, "Home", true))
            .child(div().flex_1())
            .child(
                nav("home-nav-shortcuts", Lucide::Keyboard, "Shortcuts", false)
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(ShowShortcuts), cx)),
            )
            .child(
                nav("home-nav-settings", Lucide::Settings, "Settings", false)
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenSettings), cx)),
            )
    }

    fn render_recovery(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let info = self.recovery.as_ref()?;
        Some(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_3()
                .p_4()
                .rounded_lg()
                .border_1()
                .border_color(rgb(ACCENT))
                .bg(rgb(PANEL))
                .child(
                    Icon::new(Lucide::LifeBuoy)
                        .size(px(22.0))
                        .text_color(rgb(ACCENT)),
                )
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .child(div().text_sm().child(format!(
                            "Unsaved work from \u{201c}{}\u{201d} can be restored",
                            info.name
                        )))
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(TEXT_DIM))
                                .child(recovery_detail(info)),
                        ),
                )
                .child(
                    Button::new("home-recovery-discard")
                        .label("Discard")
                        .small()
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(HomeEvent::DiscardRecovery))),
                )
                .child(
                    Button::new("home-recovery-restore")
                        .primary()
                        .label("Restore")
                        .small()
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(HomeEvent::Restore))),
                ),
        )
    }

    fn render_create(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let presets = CANVAS_PRESETS
            .iter()
            .enumerate()
            .map(|(index, (label, w, h))| {
                let active = index == self.preset;
                // A little picture of the shape, its long side 30 px.
                let long = (*w).max(*h) as f32;
                let (sw, sh) = (30.0 * *w as f32 / long, 30.0 * *h as f32 / long);
                div()
                    .id(("home-preset", index))
                    .w(px(92.0))
                    .h(px(92.0))
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_1()
                    .rounded_md()
                    .border_1()
                    .cursor_pointer()
                    .border_color(rgb(if active { ACCENT } else { BORDER }))
                    .when(active, |chip| chip.bg(rgb(PANEL_RAISED)))
                    .hover(|style| style.bg(rgb(PANEL_RAISED)))
                    .on_click(cx.listener(move |home, _, _, cx| {
                        home.preset = index;
                        cx.notify();
                    }))
                    .child(
                        div().h(px(32.0)).flex().items_center().child(
                            div()
                                .w(px(sw))
                                .h(px(sh))
                                .rounded(px(3.0))
                                .border_2()
                                .border_color(rgb(if active { ACCENT } else { TEXT_DIM })),
                        ),
                    )
                    .child(div().text_sm().child(*label))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(TEXT_DIM))
                            .child(format!("{w}\u{d7}{h}")),
                    )
            });

        let current = self.fps;
        let this = cx.entity().downgrade();
        let fps = Button::new("home-fps")
            .label(fps_label(self.fps))
            .small()
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                FRAME_RATES.into_iter().fold(menu, |menu, fps| {
                    let this = this.clone();
                    menu.item(
                        PopupMenuItem::new(fps_label(fps))
                            .checked((fps - current).abs() < 0.001)
                            .on_click(move |_, _, cx| {
                                let _ = this.update(cx, |home, cx| {
                                    home.fps = fps;
                                    cx.notify();
                                });
                            }),
                    )
                })
            });

        let new_tile = div()
            .id("home-new")
            .w(px(200.0))
            .h(px(140.0))
            .flex_none()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .rounded_lg()
            .bg(rgb(ACCENT))
            .text_color(rgb(0x0b1214))
            .cursor_pointer()
            .hover(|style| style.bg(rgb(ACCENT_HOVER)))
            .on_click(cx.listener(|home, _, _, cx| home.create(cx)))
            .child(Icon::new(Lucide::Plus).size(px(30.0)))
            .child(
                div()
                    .text_base()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("New project"),
            );

        div()
            .flex()
            .flex_row()
            .gap_5()
            .p_5()
            .rounded_lg()
            .bg(rgb(PANEL))
            .child(new_tile)
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(div().flex().flex_row().gap_2().children(presets))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_3()
                            .child(div().w(px(240.0)).child(Input::new(&self.name).small()))
                            .child(fps)
                            .child(div().flex_1())
                            .child(
                                Button::new("home-open")
                                    .icon(Lucide::FolderOpen)
                                    .label("Open project\u{2026}")
                                    .small()
                                    .on_click(
                                        cx.listener(|_, _, _, cx| cx.emit(HomeEvent::Browse)),
                                    ),
                            ),
                    ),
            )
    }

    fn render_card(
        &mut self,
        index: usize,
        entry: RecentEntry,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let poster = self.poster(&entry, cx);
        let missing = entry.missing;
        let path = entry.path.clone();
        let forget_path = entry.path.clone();
        let meta = match entry.duration {
            Some(duration) if duration > 0 => {
                format!(
                    "{} \u{b7} {}",
                    opened_label(entry.opened_at),
                    clock_label(duration)
                )
            }
            _ => opened_label(entry.opened_at),
        };
        let group = SharedString::from(format!("home-card-{index}"));
        div()
            .id(("home-card", index))
            .group(group.clone())
            .relative()
            .flex()
            .flex_col()
            .gap_1p5()
            .cursor_pointer()
            .on_click(cx.listener(move |home, _, _, cx| {
                if missing {
                    home.notice = Some(
                        format!(
                            "{path} is missing. It may be on a drive that is not mounted, \
                             or it was moved or deleted."
                        )
                        .into(),
                    );
                    cx.notify();
                } else {
                    cx.emit(HomeEvent::Open(PathBuf::from(&path)));
                }
            }))
            .child(
                div()
                    .w_full()
                    .h(px(132.0))
                    .rounded_md()
                    .overflow_hidden()
                    .bg(rgb(0x000000))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .group_hover(group.clone(), |style| style.border_color(rgb(ACCENT)))
                    .when(missing, |frame| frame.opacity(0.4))
                    .child(poster),
            )
            .child(
                div()
                    .text_sm()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_color(rgb(if missing { TEXT_DIM } else { TEXT }))
                    .child(entry.name.clone()),
            )
            .child(div().text_xs().text_color(rgb(TEXT_DIM)).child(if missing {
                format!("Missing \u{b7} {meta}")
            } else {
                meta
            }))
            .child(
                div()
                    .text_xs()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_color(rgb(TEXT_DIM))
                    .child(entry.path.clone()),
            )
            .when(missing, |card| {
                card.child(
                    div()
                        .absolute()
                        .top(px(8.0))
                        .left(px(8.0))
                        .px_1p5()
                        .rounded_sm()
                        .bg(rgb(0xe5484d))
                        .text_xs()
                        .text_color(rgb(0xffffff))
                        .child("Missing"),
                )
            })
            .child(
                div()
                    .absolute()
                    .top(px(6.0))
                    .right(px(6.0))
                    .invisible()
                    .group_hover(group, |style| style.visible())
                    .child(
                        Button::new(("home-forget", index))
                            .icon(Lucide::X)
                            .xsmall()
                            .tooltip("Remove from this list")
                            .on_click(cx.listener(move |home, _, _, cx| {
                                cx.stop_propagation();
                                home.forget(forget_path.clone(), cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    fn render_projects(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(recent) = self.recent.clone() else {
            return div()
                .text_sm()
                .text_color(rgb(TEXT_DIM))
                .child("Loading projects\u{2026}")
                .into_any_element();
        };
        if recent.is_empty() {
            return div()
                .py_8()
                .flex()
                .flex_col()
                .items_center()
                .gap_2()
                .text_color(rgb(TEXT_DIM))
                .child(Icon::new(Lucide::FolderOpen).size(px(28.0)))
                .child(
                    div()
                        .text_sm()
                        .child("No projects yet. Make a new one, or open a .chukcut file."),
                )
                .into_any_element();
        }
        let cards: Vec<AnyElement> = recent
            .into_iter()
            .enumerate()
            .map(|(index, entry)| self.render_card(index, entry, cx))
            .collect();
        div()
            .grid()
            .grid_cols(5)
            .gap_x_4()
            .gap_y_5()
            .children(cards)
            .into_any_element()
    }
}

impl Render for Home {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.recent.as_ref().map(Vec::len).unwrap_or(0);
        let recovery = self.render_recovery(cx);
        let create = self.render_create(cx);
        let projects = self.render_projects(cx);
        div()
            .track_focus(&self.focus)
            .key_context("Home")
            .size_full()
            .flex()
            .flex_row()
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
            .font_family("Noto Sans")
            .child(self.render_sidebar())
            .child(
                div()
                    .id("home-main")
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_y_scroll()
                    .px(px(40.0))
                    .py(px(32.0))
                    .flex()
                    .flex_col()
                    .gap_6()
                    .children(recovery)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(
                                div()
                                    .text_xl()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child("Start creating"),
                            )
                            .child(create),
                    )
                    .children(self.notice.clone().map(|notice| {
                        div()
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .bg(rgb(PANEL_RAISED))
                            .text_sm()
                            .child(notice)
                    }))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_baseline()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_lg()
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .child("Projects"),
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(rgb(TEXT_DIM))
                                            .child(format!("({count})")),
                                    ),
                            )
                            .child(projects),
                    ),
            )
    }
}

/// "2 clips · 0:14 · autosaved 5 min ago".
pub(crate) fn recovery_detail(info: &RecoveryInfo) -> String {
    let clips = match info.clips {
        1 => "1 clip".to_string(),
        n => format!("{n} clips"),
    };
    let mut parts = vec![clips];
    if info.duration > 0 {
        parts.push(clock_label(info.duration));
    }
    parts.push(format!("autosaved {}", opened_label(info.written_at)).to_lowercase());
    if let Some(origin) = &info.origin {
        parts.push(format!("from {origin}"));
    }
    parts.join(" \u{b7} ")
}

/// "1:05" or "1:02:05".
fn clock_label(duration: Micros) -> String {
    let seconds = duration.max(0) / 1_000_000;
    let (h, m, s) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// "Just now", "12 min ago", "3 h ago", "Yesterday", "4 days ago", "3 Oct 2026".
fn opened_label(millis: i64) -> String {
    relative_label(millis, now_millis())
}

fn relative_label(millis: i64, now: i64) -> String {
    let seconds = ((now - millis) / 1000).max(0);
    match seconds {
        0..=59 => "Just now".into(),
        60..=3599 => format!("{} min ago", seconds / 60),
        3600..=86_399 => format!("{} h ago", seconds / 3600),
        86_400..=172_799 => "Yesterday".into(),
        172_800..=604_799 => format!("{} days ago", seconds / 86_400),
        _ => {
            let (year, month, day) = civil_date(millis.div_euclid(86_400_000));
            const MONTHS: [&str; 12] = [
                "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
            ];
            format!("{day} {} {year}", MONTHS[(month - 1) as usize])
        }
    }
}

/// Days since 1970-01-01 to (year, month, day), proleptic Gregorian. Howard
/// Hinnant's `civil_from_days`.
fn civil_date(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_civil() {
        assert_eq!(civil_date(0), (1970, 1, 1));
        // 2026-10-03
        assert_eq!(civil_date(20_729), (2026, 10, 3));
        assert_eq!(civil_date(-1), (1969, 12, 31));
    }

    #[test]
    fn recent_times_read_relatively_and_old_ones_as_dates() {
        let now = 20_729 * 86_400_000 + 12 * 3_600_000;
        assert_eq!(relative_label(now - 5_000, now), "Just now");
        assert_eq!(relative_label(now - 12 * 60_000, now), "12 min ago");
        assert_eq!(relative_label(now - 26 * 3_600_000, now), "Yesterday");
        assert_eq!(relative_label(now - 30 * 86_400_000, now), "3 Sep 2026");
    }

    #[test]
    fn durations_read_as_a_clock() {
        assert_eq!(clock_label(65_000_000), "1:05");
        assert_eq!(clock_label(3_725_000_000), "1:02:05");
    }
}
