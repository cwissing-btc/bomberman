//! Sprites and fonts, compiled into the executable.
//!
//! Everything is `include_bytes!`, so the game is a single `.exe` with no
//! folder next to it that could go missing.

use std::collections::HashMap;

use macroquad::prelude::*;
use serde::Deserialize;

const SHEET_PNG: &[u8] = include_bytes!("../assets/spritesheet.png");
const ATLAS_JSON: &str = include_str!("../assets/atlas.json");
const FONT_BOLD: &[u8] = include_bytes!("../assets/DejaVuSans-Bold.ttf");
const FONT_REGULAR: &[u8] = include_bytes!("../assets/DejaVuSans.ttf");

/// Source size of every frame.
pub const TILE: f32 = 64.0;

#[derive(Deserialize)]
struct Atlas {
    frames: HashMap<String, FrameRect>,
}

#[derive(Deserialize)]
struct FrameRect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

pub struct Sprites {
    sheet: Texture2D,
    frames: HashMap<String, Rect>,
}

impl Sprites {
    pub fn load() -> Sprites {
        let sheet = Texture2D::from_file_with_format(SHEET_PNG, Some(ImageFormat::Png));
        // Pixel art: never smooth when enlarging.
        sheet.set_filter(FilterMode::Nearest);
        let atlas: Atlas = serde_json::from_str(ATLAS_JSON).expect("embedded atlas is valid");
        let frames = atlas
            .frames
            .into_iter()
            .map(|(name, r)| (name, Rect::new(r.x, r.y, r.w, r.h)))
            .collect();
        Sprites { sheet, frames }
    }

    /// Draw a frame into a `w` x `h` box. A missing frame draws magenta, which
    /// is loud on purpose: a typo in a sprite name should be seen, not hidden.
    pub fn draw(&self, name: &str, x: f32, y: f32, w: f32, h: f32, tint: Color) {
        match self.frames.get(name) {
            Some(src) => draw_texture_ex(
                &self.sheet,
                x,
                y,
                tint,
                DrawTextureParams {
                    dest_size: Some(vec2(w, h)),
                    source: Some(*src),
                    ..Default::default()
                },
            ),
            None => draw_rectangle(x, y, w, h, MAGENTA),
        }
    }

    /// Draw mirrored horizontally around the box centre.
    pub fn draw_flipped(&self, name: &str, x: f32, y: f32, w: f32, h: f32, tint: Color) {
        if let Some(src) = self.frames.get(name) {
            draw_texture_ex(
                &self.sheet,
                x,
                y,
                tint,
                DrawTextureParams {
                    dest_size: Some(vec2(w, h)),
                    source: Some(*src),
                    flip_x: true,
                    ..Default::default()
                },
            );
        }
    }
}

pub struct Fonts {
    pub bold: Font,
    pub regular: Font,
}

impl Fonts {
    pub fn load() -> Fonts {
        let mut bold = load_ttf_font_from_bytes(FONT_BOLD).expect("embedded font is valid");
        let mut regular = load_ttf_font_from_bytes(FONT_REGULAR).expect("embedded font is valid");
        bold.set_filter(FilterMode::Linear);
        regular.set_filter(FilterMode::Linear);
        Fonts { bold, regular }
    }
}

/// The window icon: the blue player's front view, cut from the sprite sheet
/// before the window exists (decoding a PNG needs no GPU).
pub fn window_icon() -> Option<miniquad::conf::Icon> {
    let image = Image::from_file_with_format(SHEET_PNG, Some(ImageFormat::Png)).ok()?;
    let atlas: Atlas = serde_json::from_str(ATLAS_JSON).ok()?;
    let r = atlas.frames.get("player_blue_down_0")?;
    let (ox, oy) = (r.x as u32, r.y as u32);

    let sample = |size: u32| -> Vec<u8> {
        // Box filter from 64 px: averaging keeps the silhouette readable at 16 px.
        let scale = 64 / size;
        let mut out = Vec::with_capacity((size * size * 4) as usize);
        for y in 0..size {
            for x in 0..size {
                let mut acc = [0u32; 4];
                for dy in 0..scale {
                    for dx in 0..scale {
                        let c = image.get_pixel(ox + x * scale + dx, oy + y * scale + dy);
                        acc[0] += (c.r * 255.0) as u32;
                        acc[1] += (c.g * 255.0) as u32;
                        acc[2] += (c.b * 255.0) as u32;
                        acc[3] += (c.a * 255.0) as u32;
                    }
                }
                let n = scale * scale;
                out.extend(acc.iter().map(|v| (v / n) as u8));
            }
        }
        out
    };

    Some(miniquad::conf::Icon {
        small: sample(16).try_into().ok()?,
        medium: sample(32).try_into().ok()?,
        big: sample(64).try_into().ok()?,
    })
}

// -- sprite names ---------------------------------------------------------------

pub const PLAYER_VARIANTS: [&str; 4] = ["blue", "red", "yellow", "purple"];

pub fn player_sprite(id: u8, dir: bomber_domain::shared::Direction, frame: usize) -> String {
    use bomber_domain::shared::Direction::*;
    let facing = match dir {
        Down => "down",
        Up => "up",
        Left => "left",
        Right => "right",
    };
    format!(
        "player_{}_{}_{}",
        PLAYER_VARIANTS[id as usize % 4],
        facing,
        frame % 4
    )
}

pub fn item_sprite(kind: bomber_domain::game::PowerupKind, frame: usize) -> String {
    use bomber_domain::game::PowerupKind::*;
    let base = match kind {
        ExtraBomb => "item_bomb_up",
        Flame => "item_fire_up",
        Speed => "item_speed_up",
    };
    format!("{base}_{}", frame % 2)
}
