//! Orb Livestream events.

#![warn(clippy::pedantic)]
#![allow(clippy::from_over_into, clippy::used_underscore_binding)]

use rkyv::{Archive, Deserialize, Serialize};

#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Archive, Serialize, Deserialize)]
#[archive(check_bytes)]
pub struct Modifiers {
    pub alt: bool,
    pub ctrl: bool,
    pub shift: bool,
    pub mac_cmd: bool,
    pub command: bool,
}

#[derive(Clone, Debug, Archive, Serialize, Deserialize)]
#[archive(check_bytes)]
pub enum Event {
    Copy,
    Cut,
    Paste(String),
    Text(String),
    Key { key: Key, physical_key: Option<Key>, pressed: bool, repeat: bool, modifiers: Modifiers },
    PointerMoved(Pos2),
    PointerButton { pos: Pos2, button: PointerButton, pressed: bool, modifiers: Modifiers },
    PointerGone,
    Scroll(Vec2),
    Zoom(f32),
    CompositionStart,
    CompositionUpdate(String),
    CompositionEnd(String),
    MouseWheel { unit: MouseWheelUnit, delta: Vec2, modifiers: Modifiers },
}

#[derive(Clone, Debug, Archive, Serialize, Deserialize)]
#[archive(check_bytes)]
pub enum Key {
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    Escape,
    Tab,
    Backspace,
    Enter,
    Space,
    Insert,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,
    Copy,
    Cut,
    Paste,
    Colon,
    Comma,
    Backslash,
    Slash,
    Pipe,
    Questionmark,
    OpenBracket,
    CloseBracket,
    Backtick,
    Minus,
    Period,
    Plus,
    Equals,
    Semicolon,
    Num0,
    Num1,
    Num2,
    Num3,
    Num4,
    Num5,
    Num6,
    Num7,
    Num8,
    Num9,
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    V,
    W,
    X,
    Y,
    Z,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    F13,
    F14,
    F15,
    F16,
    F17,
    F18,
    F19,
    F20,
    F21,
    F22,
    F23,
    F24,
    F25,
    F26,
    F27,
    F28,
    F29,
    F30,
    F31,
    F32,
    F33,
    F34,
    F35,
}

#[derive(Clone, Debug, Archive, Serialize, Deserialize)]
#[archive(check_bytes)]
pub struct Pos2 {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Debug, Archive, Serialize, Deserialize)]
#[archive(check_bytes)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Debug, Archive, Serialize, Deserialize)]
#[archive(check_bytes)]
pub enum PointerButton {
    Primary = 0,
    Secondary = 1,
    Middle = 2,
    Extra1 = 3,
    Extra2 = 4,
}

#[derive(Clone, Debug, Archive, Serialize, Deserialize)]
#[archive(check_bytes)]
pub enum MouseWheelUnit {
    Point,
    Line,
    Page,
}

impl TryFrom<&egui::Event> for Event {
    type Error = ();

    fn try_from(event: &egui::Event) -> Result<Self, Self::Error> {
        match event {
            egui::Event::Copy => Ok(Event::Copy),
            egui::Event::Cut => Ok(Event::Cut),
            egui::Event::Paste(string) => Ok(Event::Paste(string.clone())),
            egui::Event::Text(string) => Ok(Event::Text(string.clone())),
            egui::Event::Key { key, physical_key, pressed, repeat, modifiers } => Ok(Event::Key {
                key: key.into(),
                physical_key: physical_key.as_ref().map(Into::into),
                pressed: *pressed,
                repeat: *repeat,
                modifiers: modifiers.into(),
            }),
            egui::Event::PointerMoved(pos2) => Ok(Event::PointerMoved(pos2.into())),
            egui::Event::PointerButton { pos, button, pressed, modifiers } => {
                Ok(Event::PointerButton {
                    pos: pos.into(),
                    button: button.into(),
                    pressed: *pressed,
                    modifiers: modifiers.into(),
                })
            }
            egui::Event::PointerGone => Ok(Event::PointerGone),
            egui::Event::Scroll(vec2) => Ok(Event::Scroll(vec2.into())),
            egui::Event::Zoom(scale) => Ok(Event::Zoom(*scale)),
            egui::Event::CompositionStart => Ok(Event::CompositionStart),
            egui::Event::CompositionUpdate(string) => Ok(Event::CompositionUpdate(string.clone())),
            egui::Event::CompositionEnd(string) => Ok(Event::CompositionEnd(string.clone())),
            egui::Event::MouseWheel { unit, delta, modifiers } => Ok(Event::MouseWheel {
                unit: unit.into(),
                delta: delta.into(),
                modifiers: modifiers.into(),
            }),
            _ => Err(()),
        }
    }
}

impl Into<egui::Event> for Event {
    fn into(self) -> egui::Event {
        match self {
            Event::Copy => egui::Event::Copy,
            Event::Cut => egui::Event::Cut,
            Event::Paste(string) => egui::Event::Paste(string),
            Event::Text(string) => egui::Event::Text(string),
            Event::Key { key, physical_key, pressed, repeat, modifiers } => egui::Event::Key {
                key: key.into(),
                physical_key: physical_key.map(Into::into),
                pressed,
                repeat,
                modifiers: modifiers.into(),
            },
            Event::PointerMoved(pos2) => egui::Event::PointerMoved(pos2.into()),
            Event::PointerButton { pos, button, pressed, modifiers } => {
                egui::Event::PointerButton {
                    pos: pos.into(),
                    button: button.into(),
                    pressed,
                    modifiers: modifiers.into(),
                }
            }
            Event::PointerGone => egui::Event::PointerGone,
            Event::Scroll(vec2) => egui::Event::Scroll(vec2.into()),
            Event::Zoom(scale) => egui::Event::Zoom(scale),
            Event::CompositionStart => egui::Event::CompositionStart,
            Event::CompositionUpdate(string) => egui::Event::CompositionUpdate(string),
            Event::CompositionEnd(string) => egui::Event::CompositionEnd(string),
            Event::MouseWheel { unit, delta, modifiers } => egui::Event::MouseWheel {
                unit: unit.into(),
                delta: delta.into(),
                modifiers: modifiers.into(),
            },
        }
    }
}

impl From<&egui::Key> for Key {
    #[allow(clippy::too_many_lines)]
    fn from(key: &egui::Key) -> Self {
        match key {
            egui::Key::ArrowDown => Key::ArrowDown,
            egui::Key::ArrowLeft => Key::ArrowLeft,
            egui::Key::ArrowRight => Key::ArrowRight,
            egui::Key::ArrowUp => Key::ArrowUp,
            egui::Key::Escape => Key::Escape,
            egui::Key::Tab => Key::Tab,
            egui::Key::Backspace => Key::Backspace,
            egui::Key::Enter => Key::Enter,
            egui::Key::Space => Key::Space,
            egui::Key::Insert => Key::Insert,
            egui::Key::Delete => Key::Delete,
            egui::Key::Home => Key::Home,
            egui::Key::End => Key::End,
            egui::Key::PageUp => Key::PageUp,
            egui::Key::PageDown => Key::PageDown,
            egui::Key::Copy => Key::Copy,
            egui::Key::Cut => Key::Cut,
            egui::Key::Paste => Key::Paste,
            egui::Key::Colon => Key::Colon,
            egui::Key::Comma => Key::Comma,
            egui::Key::Backslash => Key::Backslash,
            egui::Key::Slash => Key::Slash,
            egui::Key::Pipe => Key::Pipe,
            egui::Key::Questionmark => Key::Questionmark,
            egui::Key::OpenBracket => Key::OpenBracket,
            egui::Key::CloseBracket => Key::CloseBracket,
            egui::Key::Backtick => Key::Backtick,
            egui::Key::Minus => Key::Minus,
            egui::Key::Period => Key::Period,
            egui::Key::Plus => Key::Plus,
            egui::Key::Equals => Key::Equals,
            egui::Key::Semicolon => Key::Semicolon,
            egui::Key::Num0 => Key::Num0,
            egui::Key::Num1 => Key::Num1,
            egui::Key::Num2 => Key::Num2,
            egui::Key::Num3 => Key::Num3,
            egui::Key::Num4 => Key::Num4,
            egui::Key::Num5 => Key::Num5,
            egui::Key::Num6 => Key::Num6,
            egui::Key::Num7 => Key::Num7,
            egui::Key::Num8 => Key::Num8,
            egui::Key::Num9 => Key::Num9,
            egui::Key::A => Key::A,
            egui::Key::B => Key::B,
            egui::Key::C => Key::C,
            egui::Key::D => Key::D,
            egui::Key::E => Key::E,
            egui::Key::F => Key::F,
            egui::Key::G => Key::G,
            egui::Key::H => Key::H,
            egui::Key::I => Key::I,
            egui::Key::J => Key::J,
            egui::Key::K => Key::K,
            egui::Key::L => Key::L,
            egui::Key::M => Key::M,
            egui::Key::N => Key::N,
            egui::Key::O => Key::O,
            egui::Key::P => Key::P,
            egui::Key::Q => Key::Q,
            egui::Key::R => Key::R,
            egui::Key::S => Key::S,
            egui::Key::T => Key::T,
            egui::Key::U => Key::U,
            egui::Key::V => Key::V,
            egui::Key::W => Key::W,
            egui::Key::X => Key::X,
            egui::Key::Y => Key::Y,
            egui::Key::Z => Key::Z,
            egui::Key::F1 => Key::F1,
            egui::Key::F2 => Key::F2,
            egui::Key::F3 => Key::F3,
            egui::Key::F4 => Key::F4,
            egui::Key::F5 => Key::F5,
            egui::Key::F6 => Key::F6,
            egui::Key::F7 => Key::F7,
            egui::Key::F8 => Key::F8,
            egui::Key::F9 => Key::F9,
            egui::Key::F10 => Key::F10,
            egui::Key::F11 => Key::F11,
            egui::Key::F12 => Key::F12,
            egui::Key::F13 => Key::F13,
            egui::Key::F14 => Key::F14,
            egui::Key::F15 => Key::F15,
            egui::Key::F16 => Key::F16,
            egui::Key::F17 => Key::F17,
            egui::Key::F18 => Key::F18,
            egui::Key::F19 => Key::F19,
            egui::Key::F20 => Key::F20,
            egui::Key::F21 => Key::F21,
            egui::Key::F22 => Key::F22,
            egui::Key::F23 => Key::F23,
            egui::Key::F24 => Key::F24,
            egui::Key::F25 => Key::F25,
            egui::Key::F26 => Key::F26,
            egui::Key::F27 => Key::F27,
            egui::Key::F28 => Key::F28,
            egui::Key::F29 => Key::F29,
            egui::Key::F30 => Key::F30,
            egui::Key::F31 => Key::F31,
            egui::Key::F32 => Key::F32,
            egui::Key::F33 => Key::F33,
            egui::Key::F34 => Key::F34,
            egui::Key::F35 => Key::F35,
        }
    }
}

impl Into<egui::Key> for Key {
    #[allow(clippy::too_many_lines)]
    fn into(self) -> egui::Key {
        match self {
            Key::ArrowDown => egui::Key::ArrowDown,
            Key::ArrowLeft => egui::Key::ArrowLeft,
            Key::ArrowRight => egui::Key::ArrowRight,
            Key::ArrowUp => egui::Key::ArrowUp,
            Key::Escape => egui::Key::Escape,
            Key::Tab => egui::Key::Tab,
            Key::Backspace => egui::Key::Backspace,
            Key::Enter => egui::Key::Enter,
            Key::Space => egui::Key::Space,
            Key::Insert => egui::Key::Insert,
            Key::Delete => egui::Key::Delete,
            Key::Home => egui::Key::Home,
            Key::End => egui::Key::End,
            Key::PageUp => egui::Key::PageUp,
            Key::PageDown => egui::Key::PageDown,
            Key::Copy => egui::Key::Copy,
            Key::Cut => egui::Key::Cut,
            Key::Paste => egui::Key::Paste,
            Key::Colon => egui::Key::Colon,
            Key::Comma => egui::Key::Comma,
            Key::Backslash => egui::Key::Backslash,
            Key::Slash => egui::Key::Slash,
            Key::Pipe => egui::Key::Pipe,
            Key::Questionmark => egui::Key::Questionmark,
            Key::OpenBracket => egui::Key::OpenBracket,
            Key::CloseBracket => egui::Key::CloseBracket,
            Key::Backtick => egui::Key::Backtick,
            Key::Minus => egui::Key::Minus,
            Key::Period => egui::Key::Period,
            Key::Plus => egui::Key::Plus,
            Key::Equals => egui::Key::Equals,
            Key::Semicolon => egui::Key::Semicolon,
            Key::Num0 => egui::Key::Num0,
            Key::Num1 => egui::Key::Num1,
            Key::Num2 => egui::Key::Num2,
            Key::Num3 => egui::Key::Num3,
            Key::Num4 => egui::Key::Num4,
            Key::Num5 => egui::Key::Num5,
            Key::Num6 => egui::Key::Num6,
            Key::Num7 => egui::Key::Num7,
            Key::Num8 => egui::Key::Num8,
            Key::Num9 => egui::Key::Num9,
            Key::A => egui::Key::A,
            Key::B => egui::Key::B,
            Key::C => egui::Key::C,
            Key::D => egui::Key::D,
            Key::E => egui::Key::E,
            Key::F => egui::Key::F,
            Key::G => egui::Key::G,
            Key::H => egui::Key::H,
            Key::I => egui::Key::I,
            Key::J => egui::Key::J,
            Key::K => egui::Key::K,
            Key::L => egui::Key::L,
            Key::M => egui::Key::M,
            Key::N => egui::Key::N,
            Key::O => egui::Key::O,
            Key::P => egui::Key::P,
            Key::Q => egui::Key::Q,
            Key::R => egui::Key::R,
            Key::S => egui::Key::S,
            Key::T => egui::Key::T,
            Key::U => egui::Key::U,
            Key::V => egui::Key::V,
            Key::W => egui::Key::W,
            Key::X => egui::Key::X,
            Key::Y => egui::Key::Y,
            Key::Z => egui::Key::Z,
            Key::F1 => egui::Key::F1,
            Key::F2 => egui::Key::F2,
            Key::F3 => egui::Key::F3,
            Key::F4 => egui::Key::F4,
            Key::F5 => egui::Key::F5,
            Key::F6 => egui::Key::F6,
            Key::F7 => egui::Key::F7,
            Key::F8 => egui::Key::F8,
            Key::F9 => egui::Key::F9,
            Key::F10 => egui::Key::F10,
            Key::F11 => egui::Key::F11,
            Key::F12 => egui::Key::F12,
            Key::F13 => egui::Key::F13,
            Key::F14 => egui::Key::F14,
            Key::F15 => egui::Key::F15,
            Key::F16 => egui::Key::F16,
            Key::F17 => egui::Key::F17,
            Key::F18 => egui::Key::F18,
            Key::F19 => egui::Key::F19,
            Key::F20 => egui::Key::F20,
            Key::F21 => egui::Key::F21,
            Key::F22 => egui::Key::F22,
            Key::F23 => egui::Key::F23,
            Key::F24 => egui::Key::F24,
            Key::F25 => egui::Key::F25,
            Key::F26 => egui::Key::F26,
            Key::F27 => egui::Key::F27,
            Key::F28 => egui::Key::F28,
            Key::F29 => egui::Key::F29,
            Key::F30 => egui::Key::F30,
            Key::F31 => egui::Key::F31,
            Key::F32 => egui::Key::F32,
            Key::F33 => egui::Key::F33,
            Key::F34 => egui::Key::F34,
            Key::F35 => egui::Key::F35,
        }
    }
}

impl From<&egui::Modifiers> for Modifiers {
    fn from(modifiers: &egui::Modifiers) -> Self {
        let egui::Modifiers { alt, ctrl, shift, mac_cmd, command } = modifiers;
        Self { alt: *alt, ctrl: *ctrl, shift: *shift, mac_cmd: *mac_cmd, command: *command }
    }
}

impl Into<egui::Modifiers> for Modifiers {
    fn into(self) -> egui::Modifiers {
        let Self { alt, ctrl, shift, mac_cmd, command } = self;
        egui::Modifiers { alt, ctrl, shift, mac_cmd, command }
    }
}

impl From<&egui::Pos2> for Pos2 {
    fn from(pos2: &egui::Pos2) -> Self {
        let egui::Pos2 { x, y } = pos2;
        Self { x: *x, y: *y }
    }
}

impl Into<egui::Pos2> for Pos2 {
    fn into(self) -> egui::Pos2 {
        let Self { x, y } = self;
        egui::Pos2 { x, y }
    }
}

impl From<&egui::Vec2> for Vec2 {
    fn from(vec2: &egui::Vec2) -> Self {
        let egui::Vec2 { x, y } = vec2;
        Self { x: *x, y: *y }
    }
}

impl Into<egui::Vec2> for Vec2 {
    fn into(self) -> egui::Vec2 {
        let Self { x, y } = self;
        egui::Vec2 { x, y }
    }
}

impl From<&egui::PointerButton> for PointerButton {
    fn from(button: &egui::PointerButton) -> Self {
        match button {
            egui::PointerButton::Primary => PointerButton::Primary,
            egui::PointerButton::Secondary => PointerButton::Secondary,
            egui::PointerButton::Middle => PointerButton::Middle,
            egui::PointerButton::Extra1 => PointerButton::Extra1,
            egui::PointerButton::Extra2 => PointerButton::Extra2,
        }
    }
}

impl Into<egui::PointerButton> for PointerButton {
    fn into(self) -> egui::PointerButton {
        match self {
            PointerButton::Primary => egui::PointerButton::Primary,
            PointerButton::Secondary => egui::PointerButton::Secondary,
            PointerButton::Middle => egui::PointerButton::Middle,
            PointerButton::Extra1 => egui::PointerButton::Extra1,
            PointerButton::Extra2 => egui::PointerButton::Extra2,
        }
    }
}

impl From<&egui::MouseWheelUnit> for MouseWheelUnit {
    fn from(unit: &egui::MouseWheelUnit) -> Self {
        match unit {
            egui::MouseWheelUnit::Point => MouseWheelUnit::Point,
            egui::MouseWheelUnit::Line => MouseWheelUnit::Line,
            egui::MouseWheelUnit::Page => MouseWheelUnit::Page,
        }
    }
}

impl Into<egui::MouseWheelUnit> for MouseWheelUnit {
    fn into(self) -> egui::MouseWheelUnit {
        match self {
            MouseWheelUnit::Point => egui::MouseWheelUnit::Point,
            MouseWheelUnit::Line => egui::MouseWheelUnit::Line,
            MouseWheelUnit::Page => egui::MouseWheelUnit::Page,
        }
    }
}
