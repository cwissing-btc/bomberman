//! Small immediate-mode UI: colours, text, buttons, a text field.
//!
//! Everything is sized from [`Ui::unit`], which follows the window height, so
//! the game looks the same on a laptop at 150 % scaling and on a projector.

use macroquad::prelude::*;

use crate::assets::Fonts;

pub mod palette {
    use macroquad::prelude::Color;

    const fn hex(rgb: u32) -> Color {
        Color::new(
            ((rgb >> 16) & 0xFF) as f32 / 255.0,
            ((rgb >> 8) & 0xFF) as f32 / 255.0,
            (rgb & 0xFF) as f32 / 255.0,
            1.0,
        )
    }

    pub const BACKGROUND: Color = hex(0x14161F);
    pub const PANEL: Color = hex(0x1D2030);
    pub const PANEL_LIGHT: Color = hex(0x272B40);
    pub const TEXT: Color = hex(0xE8EAF0);
    pub const TEXT_DIM: Color = hex(0x9AA0B0);
    pub const ACCENT: Color = hex(0x17BEBB);
    pub const WARNING: Color = hex(0xFF6B6B);
    pub const OK: Color = hex(0x3DDC97);
    pub const HIGHLIGHT: Color = hex(0xFFD652);
    pub const FIRE: Color = hex(0xFF8E2A);

    /// The suit colours of the four sprite variants, in player-id order, so a
    /// name tag, a lobby card and a figure always agree.
    pub const PLAYERS: [Color; 4] = [hex(0x3264BE), hex(0xC6323E), hex(0xE4A01E), hex(0x844CC2)];

    pub fn player(id: u8) -> Color {
        PLAYERS[id as usize % 4]
    }

    /// A lighter version for text on dark ground.
    pub fn player_text(id: u8) -> Color {
        let c = player(id);
        Color::new(
            (c.r + 0.35).min(1.0),
            (c.g + 0.35).min(1.0),
            (c.b + 0.35).min(1.0),
            1.0,
        )
    }

    pub fn with_alpha(c: Color, a: f32) -> Color {
        Color::new(c.r, c.g, c.b, a)
    }
}

use palette::*;

pub struct Ui<'a> {
    pub fonts: &'a Fonts,
    /// One unit is 1/720 of the window height.
    pub unit: f32,
    pub mouse: Vec2,
    pub clicked: bool,
    pub time: f64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

impl<'a> Ui<'a> {
    pub fn new(fonts: &'a Fonts, time: f64) -> Ui<'a> {
        let (mx, my) = mouse_position();
        Ui {
            fonts,
            unit: (screen_height() / 720.0).clamp(0.5, 4.0),
            mouse: vec2(mx, my),
            clicked: is_mouse_button_pressed(MouseButton::Left),
            time,
        }
    }

    pub fn u(&self, v: f32) -> f32 {
        v * self.unit
    }

    fn font(&self, bold: bool) -> &Font {
        if bold {
            &self.fonts.bold
        } else {
            &self.fonts.regular
        }
    }

    pub fn measure(&self, text: &str, size: f32, bold: bool) -> f32 {
        measure_text(text, Some(self.font(bold)), size.max(1.0) as u16, 1.0).width
    }

    /// Draw text with its baseline at `y`. `size` is in pixels.
    #[allow(clippy::too_many_arguments)]
    pub fn text(
        &self,
        text: &str,
        x: f32,
        y: f32,
        size: f32,
        color: Color,
        bold: bool,
        align: Align,
    ) {
        let w = self.measure(text, size, bold);
        let x = match align {
            Align::Left => x,
            Align::Center => x - w / 2.0,
            Align::Right => x - w,
        };
        draw_text_ex(
            text,
            x.round(),
            y.round(),
            TextParams {
                font: Some(self.font(bold)),
                font_size: size.max(1.0) as u16,
                color,
                ..Default::default()
            },
        );
    }

    /// Text with a dark drop shadow, readable on any background.
    #[allow(clippy::too_many_arguments)]
    pub fn text_shadow(
        &self,
        text: &str,
        x: f32,
        y: f32,
        size: f32,
        color: Color,
        bold: bool,
        align: Align,
    ) {
        let off = (size / 14.0).max(1.0);
        self.text(
            text,
            x + off,
            y + off,
            size,
            with_alpha(BLACK, 0.7 * color.a),
            bold,
            align,
        );
        self.text(text, x, y, size, color, bold, align);
    }

    /// Shorten `text` to fit `max_w`, cutting on characters and adding an
    /// ellipsis. A name can be 24 characters of W; the column cannot.
    pub fn fit(&self, text: &str, size: f32, bold: bool, max_w: f32) -> String {
        if self.measure(text, size, bold) <= max_w {
            return text.to_string();
        }
        let chars: Vec<char> = text.chars().collect();
        for keep in (0..chars.len()).rev() {
            let candidate: String = chars[..keep].iter().collect::<String>() + "…";
            if self.measure(&candidate, size, bold) <= max_w {
                return candidate;
            }
        }
        "…".into()
    }

    /// The largest size up to `max` at which `text` fits `max_w`.
    pub fn fit_size(&self, text: &str, bold: bool, max: f32, min: f32, max_w: f32) -> f32 {
        let mut size = max;
        while size > min && self.measure(text, size, bold) > max_w {
            size -= 1.0;
        }
        size
    }

    pub fn panel(&self, r: Rect, color: Color) {
        let radius = self.u(10.0).min(r.h / 2.0);
        rounded_rect(r, radius, color);
    }

    /// A button. Returns true when clicked while enabled.
    pub fn button(&self, r: Rect, label: &str, enabled: bool, primary: bool) -> bool {
        let hover = enabled && r.contains(self.mouse);
        let base = if !enabled {
            PANEL_LIGHT
        } else if primary {
            ACCENT
        } else {
            Color::new(0.24, 0.27, 0.40, 1.0)
        };
        let fill = if hover {
            Color::new(
                (base.r + 0.08).min(1.0),
                (base.g + 0.08).min(1.0),
                (base.b + 0.08).min(1.0),
                1.0,
            )
        } else {
            base
        };
        // A pressed-in look: a darker lip under the face.
        let lip = self.u(4.0);
        rounded_rect(
            Rect::new(r.x, r.y + lip, r.w, r.h),
            self.u(10.0),
            Color::new(fill.r * 0.55, fill.g * 0.55, fill.b * 0.55, 1.0),
        );
        let lift = if hover { -self.u(1.0) } else { 0.0 };
        rounded_rect(Rect::new(r.x, r.y + lift, r.w, r.h), self.u(10.0), fill);
        let size = (r.h * 0.42).min(self.u(26.0));
        let color = if enabled {
            if primary {
                BACKGROUND
            } else {
                TEXT
            }
        } else {
            TEXT_DIM
        };
        let label = self.fit(label, size, true, r.w - self.u(16.0));
        self.text(
            &label,
            r.x + r.w / 2.0,
            r.y + lift + r.h / 2.0 + size * 0.36,
            size,
            color,
            true,
            Align::Center,
        );
        hover && self.clicked
    }
}

/// Filled rectangle with rounded corners, as one triangle fan, so a
/// translucent colour stays even (overlapping pieces would darken the corners).
pub fn rounded_rect(r: Rect, radius: f32, color: Color) {
    let radius = radius.min(r.w / 2.0).min(r.h / 2.0).max(0.0);
    if radius < 0.5 {
        draw_rectangle(r.x, r.y, r.w, r.h, color);
        return;
    }
    const SEGMENTS: usize = 6;
    let corners = [
        (r.x + r.w - radius, r.y + r.h - radius, 0.0f32),
        (r.x + radius, r.y + r.h - radius, 90.0),
        (r.x + radius, r.y + radius, 180.0),
        (r.x + r.w - radius, r.y + radius, 270.0),
    ];
    let mut points = Vec::with_capacity(4 * (SEGMENTS + 1));
    for (cx, cy, start) in corners {
        for i in 0..=SEGMENTS {
            let a = (start + 90.0 * i as f32 / SEGMENTS as f32).to_radians();
            points.push(vec2(cx + radius * a.cos(), cy + radius * a.sin()));
        }
    }
    let centre = vec2(r.x + r.w / 2.0, r.y + r.h / 2.0);
    for i in 0..points.len() {
        draw_triangle(centre, points[i], points[(i + 1) % points.len()], color);
    }
}

pub fn rounded_outline(r: Rect, radius: f32, thickness: f32, color: Color) {
    let radius = radius.min(r.w / 2.0).min(r.h / 2.0);
    draw_line(r.x + radius, r.y, r.x + r.w - radius, r.y, thickness, color);
    draw_line(
        r.x + radius,
        r.y + r.h,
        r.x + r.w - radius,
        r.y + r.h,
        thickness,
        color,
    );
    draw_line(r.x, r.y + radius, r.x, r.y + r.h - radius, thickness, color);
    draw_line(
        r.x + r.w,
        r.y + radius,
        r.x + r.w,
        r.y + r.h - radius,
        thickness,
        color,
    );
    for (cx, cy, start) in [
        (r.x + radius, r.y + radius, 180.0),
        (r.x + r.w - radius, r.y + radius, 270.0),
        (r.x + r.w - radius, r.y + r.h - radius, 0.0),
        (r.x + radius, r.y + r.h - radius, 90.0),
    ] {
        draw_arc(cx, cy, 12, radius, start, thickness, 90.0, color);
    }
}

/// A single-line text field. Focus is managed by the owner.
#[derive(Debug, Clone)]
pub struct TextField {
    pub value: String,
    pub max_chars: usize,
}

impl TextField {
    pub fn new(value: &str, max_chars: usize) -> TextField {
        TextField {
            value: value.to_string(),
            max_chars,
        }
    }

    /// Feed this frame's typed characters. Call only for the focused field.
    pub fn edit(&mut self, typed: &[char]) {
        for &c in typed {
            match c {
                '\u{8}' => {
                    self.value.pop();
                }
                c if c.is_control() => {}
                c => {
                    if self.value.chars().count() < self.max_chars {
                        self.value.push(c);
                    }
                }
            }
        }
        if is_key_pressed(KeyCode::Backspace) {
            // Some platforms deliver Backspace only as a key, not a char.
            if !typed.contains(&'\u{8}') {
                self.value.pop();
            }
        }
    }

    /// Returns true when clicked (so the owner can move focus here).
    pub fn draw(&self, ui: &Ui, r: Rect, label: &str, focused: bool) -> bool {
        let label_size = ui.u(15.0);
        ui.text(
            label,
            r.x,
            r.y - ui.u(8.0),
            label_size,
            TEXT_DIM,
            false,
            Align::Left,
        );
        ui.panel(r, if focused { PANEL_LIGHT } else { PANEL });
        if focused {
            rounded_outline(r, ui.u(10.0), ui.u(2.0), ACCENT);
        }
        let size = (r.h * 0.45).min(ui.u(24.0));
        let shown = ui.fit(&self.value, size, false, r.w - ui.u(30.0));
        let y = r.y + r.h / 2.0 + size * 0.36;
        ui.text(&shown, r.x + ui.u(14.0), y, size, TEXT, false, Align::Left);
        if focused && (ui.time * 2.0).fract() < 0.55 {
            let x = r.x + ui.u(16.0) + ui.measure(&shown, size, false);
            draw_rectangle(x, r.y + r.h * 0.22, ui.u(2.0), r.h * 0.56, ACCENT);
        }
        r.contains(ui.mouse) && ui.clicked
    }
}
