//! The control socket's `ui.*` requests, answered on the UI thread.
//!
//! `crate::control` queues them; the tick drains the queue here. What needs
//! only the editor (the playhead, the selection, playback) is answered from
//! it; what needs the window (keys, clicks, a screenshot) runs deferred,
//! after the tick's own update, because a key or a click dispatched while
//! the editor is borrowed would re-enter it.
//!
//! The method set follows FilmCraft's control protocol (`ui.inspect`,
//! `ui.key`, `ui.type`, `ui.click`, `ui.drag`, `ui.screenshot`,
//! `ui.playback`), minus `ui.elements`: GPUI has no automation tree to list.

use std::sync::mpsc;

use base64::Engine as _;
use chukcut_engine::modules::project::Micros;
use gpui::{
    AnyWindowHandle, App, Keystroke, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, PlatformInput,
};
use serde_json::{json, Value};

use super::*;
use crate::control::UiRequest;

type Reply = mpsc::Sender<Result<Value, String>>;

impl Editor {
    /// Drain the control socket: redraw after a command changed the
    /// document, answer the `ui.*` requests. Answers whether anything
    /// changed on screen.
    pub(super) fn poll_control(&mut self, cx: &mut Context<Self>) -> bool {
        let mut changed = false;
        if crate::control::take_changed() {
            self.refresh(cx);
            changed = true;
        }
        for request in crate::control::take_requests() {
            changed |= self.answer(request, cx);
        }
        changed
    }

    fn answer(&mut self, request: UiRequest, cx: &mut Context<Self>) -> bool {
        let UiRequest {
            method,
            params,
            reply,
        } = request;
        let result = match method.as_str() {
            "ui.inspect" => {
                // The window's size comes from the window, after this update.
                let state = self.ui_state();
                with_window(cx, reply, move |window, _| {
                    let mut state = state;
                    let size = window.viewport_size();
                    state["window"] = json!({
                        "width": f32::from(size.width),
                        "height": f32::from(size.height),
                        "scale": window.scale_factor(),
                    });
                    Ok(state)
                });
                return false;
            }
            "ui.seek" => self.ui_seek(&params),
            "ui.select" => self.ui_select(&params),
            "ui.playback" => self.ui_playback(&params),
            "ui.key" => {
                let key = params["key"].as_str().unwrap_or_default().to_string();
                match parse_key(&key) {
                    Ok(keystroke) => with_window(cx, reply, move |window, cx| {
                        let handled = window.dispatch_keystroke(keystroke, cx);
                        Ok(json!({"key": key, "handled": handled}))
                    }),
                    Err(error) => {
                        let _ = reply.send(Err(error));
                    }
                }
                return false;
            }
            "ui.type" => {
                let text = params["text"].as_str().unwrap_or_default().to_string();
                with_window(cx, reply, move |window, cx| {
                    for c in text.chars() {
                        window.dispatch_keystroke(char_key(c), cx);
                    }
                    Ok(json!({"typed": text.chars().count()}))
                });
                return false;
            }
            "ui.click" | "ui.move" | "ui.drag" => {
                match pointer_events(&method, &params) {
                    Ok(events) => with_window(cx, reply, move |window, cx| {
                        let n = events.len();
                        for event in events {
                            window.dispatch_event(event, cx);
                        }
                        Ok(json!({"events": n}))
                    }),
                    Err(error) => {
                        let _ = reply.send(Err(error));
                    }
                }
                return false;
            }
            "ui.screenshot" => {
                let max_side = params["max_side"].as_u64().unwrap_or(1600) as u32;
                with_window(cx, reply, move |window, _| {
                    // Found here, captured off the UI thread: `import` takes
                    // a few hundred milliseconds.
                    let xid = x11_window(window)?;
                    Err(Deferred::Capture { xid, max_side }.into())
                });
                return false;
            }
            other => Err(format!(
                "unknown method {other:?}; the ui methods are ui.inspect, ui.seek, ui.select, \
                 ui.playback, ui.key, ui.type, ui.click, ui.move, ui.drag and ui.screenshot"
            )),
        };
        let ok = result.is_ok();
        let _ = reply.send(result);
        if ok {
            cx.notify();
        }
        ok
    }

    fn ui_state(&self) -> Value {
        let playhead = self.clock.position();
        json!({
            "playhead": playhead as f64 / 1_000_000.0,
            "playhead_us": playhead,
            "playing": self.clock.is_playing(),
            "selection": self.selected,
            "selected": self.selection(),
            "project": self.state.project_path.read().clone(),
            "name": self.project.name,
            "unsaved": self.is_dirty(),
            "status": self.status.as_ref().map(|s| s.to_string()),
            "duration": self.project.duration() as f64 / 1_000_000.0,
        })
    }

    fn ui_seek(&mut self, params: &Value) -> Result<Value, String> {
        let to: Micros = match (params.get("time_us"), params.get("seconds")) {
            (Some(us), _) => us.as_i64().ok_or("time_us is a whole number")?,
            (None, Some(s)) => {
                let s = s
                    .as_f64()
                    .filter(|s| s.is_finite())
                    .ok_or("seconds is a number")?;
                (s * 1_000_000.0).round() as Micros
            }
            (None, None) => return Err("ui.seek needs \"seconds\" or \"time_us\"".into()),
        };
        self.seek(to);
        Ok(
            json!({"playhead": self.clock.position() as f64 / 1_000_000.0, "playhead_us": self.clock.position()}),
        )
    }

    fn ui_select(&mut self, params: &Value) -> Result<Value, String> {
        match params.get("clip").and_then(Value::as_str) {
            None | Some("") => {
                self.clear_selection();
                Ok(json!({"selection": null}))
            }
            Some(wanted) => {
                // A full id or a unique prefix, as the CLI takes them.
                let matches: Vec<String> = self
                    .project
                    .tracks
                    .iter()
                    .flat_map(|t| t.segments.iter())
                    .filter(|s| s.id.starts_with(wanted))
                    .map(|s| s.id.clone())
                    .collect();
                match matches.as_slice() {
                    [one] => {
                        self.select_only(one);
                        Ok(json!({"selection": one}))
                    }
                    [] => Err(format!(
                        "no clip on the open timeline has the id {wanted:?}"
                    )),
                    _ => Err(format!(
                        "{wanted:?} names {} clips; give more of the id",
                        matches.len()
                    )),
                }
            }
        }
    }

    fn ui_playback(&mut self, params: &Value) -> Result<Value, String> {
        let playing = self.clock.is_playing();
        match params["action"].as_str().unwrap_or("toggle") {
            "play" if !playing => self.play(),
            "pause" | "stop" if playing => self.pause(),
            "toggle" if playing => self.pause(),
            "toggle" => self.play(),
            "play" | "pause" | "stop" => {}
            other => {
                return Err(format!(
                    "unknown playback action {other:?}; play, pause or toggle"
                ))
            }
        }
        Ok(json!({"playing": self.clock.is_playing()}))
    }
}

/// What a window job hands back to be finished off the UI thread.
enum Deferred {
    Capture { xid: u64, max_side: u32 },
}

/// A window job's error, or the work it leaves for a background thread.
enum WindowError {
    Message(String),
    Deferred(Deferred),
}

impl From<String> for WindowError {
    fn from(message: String) -> Self {
        Self::Message(message)
    }
}

impl From<&str> for WindowError {
    fn from(message: &str) -> Self {
        Self::Message(message.into())
    }
}

impl From<Deferred> for WindowError {
    fn from(work: Deferred) -> Self {
        Self::Deferred(work)
    }
}

/// Run `job` with the window once the current update is over, and send its
/// answer to `reply`.
fn with_window(
    cx: &mut App,
    reply: Reply,
    job: impl FnOnce(&mut gpui::Window, &mut App) -> Result<Value, WindowError> + 'static,
) {
    let Some(handle): Option<AnyWindowHandle> = super::shell::window_handle(cx) else {
        let _ = reply.send(Err("the app has no window".into()));
        return;
    };
    cx.defer(move |cx| {
        let result = handle
            .update(cx, |_, window, cx| job(window, cx))
            .map_err(|e| WindowError::Message(format!("the window is gone: {e}")))
            .and_then(|r| r);
        match result {
            Ok(value) => {
                let _ = reply.send(Ok(value));
            }
            Err(WindowError::Message(message)) => {
                let _ = reply.send(Err(message));
            }
            Err(WindowError::Deferred(Deferred::Capture { xid, max_side })) => {
                std::thread::spawn(move || {
                    let _ = reply.send(capture(xid, max_side));
                });
            }
        }
    });
}

/// The window's X11 id; a sentence when it is not an X11 window.
fn x11_window(window: &gpui::Window) -> Result<u64, WindowError> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = HasWindowHandle::window_handle(window)
        .map_err(|e| format!("the window has no native handle: {e}"))?;
    match handle.as_raw() {
        // `c_ulong`: u64 on 64-bit Linux, narrower elsewhere.
        #[allow(clippy::unnecessary_cast)]
        RawWindowHandle::Xlib(h) => Ok(h.window as u64),
        RawWindowHandle::Xcb(h) => Ok(u64::from(h.window.get())),
        _ => Err(
            "window screenshots need X11 (this session is Wayland); render_frame shows \
                  the picture on any display"
                .into(),
        ),
    }
}

/// Capture the window with ImageMagick's `import`, scaled so its longest
/// side is at most `max_side` (0: as captured).
fn capture(xid: u64, max_side: u32) -> Result<Value, String> {
    let output = std::process::Command::new("import")
        .args(["-window", &xid.to_string(), "png:-"])
        .output()
        .map_err(|e| format!("window screenshots need ImageMagick's `import` ({e})"))?;
    if !output.status.success() {
        return Err(format!(
            "import failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let mut image = image::load_from_memory(&output.stdout)
        .map_err(|e| format!("the screenshot is not an image: {e}"))?;
    let (w, h) = (image.width(), image.height());
    if max_side > 0 && w.max(h) > max_side {
        image = image.resize(max_side, max_side, image::imageops::FilterType::Triangle);
    }
    let mut png = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| format!("encoding the screenshot failed: {e}"))?;
    Ok(json!({
        "png": base64::engine::general_purpose::STANDARD.encode(png),
        "width": image.width(),
        "height": image.height(),
    }))
}

/// A key as FilmCraft spells it (`Cmd+K`, `Shift+Delete`, `Space`) or as
/// GPUI does (`ctrl-k`). `Cmd` is Ctrl here, as on every Linux editor.
fn parse_key(key: &str) -> Result<Keystroke, String> {
    let key = key.trim();
    if key.is_empty() {
        return Err("ui.key needs \"key\", such as space, s, ctrl-z or Shift+Delete".into());
    }
    let spelled = if key.len() > 1 && key.contains('+') {
        key.split('+')
            .map(|part| match part.to_ascii_lowercase().as_str() {
                "cmd" | "command" | "control" => "ctrl".to_string(),
                "option" => "alt".to_string(),
                other => other.to_string(),
            })
            .collect::<Vec<_>>()
            .join("-")
    } else {
        key.to_ascii_lowercase()
    };
    Keystroke::parse(&spelled).map_err(|e| format!("{key:?} is not a key: {e}"))
}

/// One typed character as a keystroke that carries it.
fn char_key(c: char) -> Keystroke {
    Keystroke {
        modifiers: Modifiers::default(),
        key: match c {
            ' ' => "space".into(),
            '\n' => "enter".into(),
            '\t' => "tab".into(),
            c => c.to_lowercase().to_string(),
        },
        key_char: Some(c.to_string()),
    }
}

fn modifiers(params: &Value) -> Modifiers {
    let on = |k: &str| params["modifiers"][k].as_bool() == Some(true);
    Modifiers {
        control: on("ctrl") || on("control") || on("command"),
        alt: on("alt"),
        shift: on("shift"),
        platform: on("super"),
        function: false,
    }
}

fn point(v: &Value, what: &str) -> Result<gpui::Point<gpui::Pixels>, String> {
    let x = v["x"].as_f64().ok_or_else(|| format!("{what} needs x"))?;
    let y = v["y"].as_f64().ok_or_else(|| format!("{what} needs y"))?;
    Ok(gpui::point(gpui::px(x as f32), gpui::px(y as f32)))
}

/// The pointer events of a click, a move or a drag.
fn pointer_events(method: &str, params: &Value) -> Result<Vec<PlatformInput>, String> {
    let modifiers = modifiers(params);
    let button = match params["button"].as_str().unwrap_or("left") {
        "left" => MouseButton::Left,
        "right" => MouseButton::Right,
        "middle" => MouseButton::Middle,
        other => return Err(format!("unknown button {other:?}; left, right or middle")),
    };
    let moved = |position, pressed_button| {
        PlatformInput::MouseMove(MouseMoveEvent {
            position,
            pressed_button,
            modifiers,
        })
    };
    let down = |position, click_count| {
        PlatformInput::MouseDown(MouseDownEvent {
            button,
            position,
            modifiers,
            click_count,
            first_mouse: false,
        })
    };
    let up = |position, click_count| {
        PlatformInput::MouseUp(MouseUpEvent {
            button,
            position,
            modifiers,
            click_count,
        })
    };
    Ok(match method {
        "ui.move" => vec![moved(point(params, "ui.move")?, None)],
        "ui.click" => {
            let at = point(params, "ui.click")?;
            let count = params["count"].as_u64().unwrap_or(1).clamp(1, 3) as usize;
            let mut events = vec![moved(at, None)];
            for n in 1..=count {
                events.push(down(at, n));
                events.push(up(at, n));
            }
            events
        }
        _ => {
            let from = point(&params["from"], "ui.drag from")?;
            let to = point(&params["to"], "ui.drag to")?;
            let steps = params["steps"].as_u64().unwrap_or(12).clamp(1, 200) as usize;
            let mut events = vec![moved(from, None), down(from, 1)];
            for i in 1..=steps {
                let t = i as f32 / steps as f32;
                let p = gpui::point(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t);
                events.push(moved(p, Some(button)));
            }
            events.push(up(to, 1));
            events
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_parse_in_both_spellings() {
        let k = parse_key("Cmd+K").unwrap();
        assert!(k.modifiers.control && k.key == "k", "{k:?}");
        let k = parse_key("Shift+Delete").unwrap();
        assert!(k.modifiers.shift && k.key == "delete", "{k:?}");
        let k = parse_key("ctrl-z").unwrap();
        assert!(k.modifiers.control && k.key == "z", "{k:?}");
        assert_eq!(parse_key("Space").unwrap().key, "space");
        assert_eq!(parse_key("+").unwrap().key, "+");
        assert!(parse_key("").is_err());
    }

    #[test]
    fn a_drag_presses_moves_and_releases() {
        let events = pointer_events(
            "ui.drag",
            &json!({"from": {"x": 10, "y": 10}, "to": {"x": 110, "y": 10}, "steps": 4}),
        )
        .unwrap();
        assert!(matches!(events[1], PlatformInput::MouseDown(_)));
        assert!(
            matches!(events.last(), Some(PlatformInput::MouseUp(e)) if f32::from(e.position.x) == 110.0)
        );
        assert_eq!(events.len(), 2 + 4 + 1);
        let double = pointer_events("ui.click", &json!({"x": 1, "y": 2, "count": 2})).unwrap();
        assert!(matches!(
            double[3],
            PlatformInput::MouseDown(MouseDownEvent { click_count: 2, .. })
        ));
        assert!(pointer_events("ui.click", &json!({"x": 1})).is_err());
    }
}
