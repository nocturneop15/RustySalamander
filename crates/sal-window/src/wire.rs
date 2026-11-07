//! What a plugin's window and the program send each other (see
//! `sal_plugin_api::abi::WindowFns`): egui's own types, encoded with
//! bincode. Both sides must use the same egui; [`Init::egui`] says which,
//! and the program refuses a window of another.

use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::egui;

/// The egui both sides encode (its major and minor version).
pub const EGUI: &str = "0.29";

/// How a window opens.
#[derive(Serialize, Deserialize, Clone)]
pub struct Init {
    /// [`EGUI`] of the plugin.
    pub egui: String,
    pub title: String,
    pub icon: egui::IconData,
    /// The inner size, in points.
    pub size: [f32; 2],
}

/// What to draw, from one frame of a window.
#[derive(Serialize, Deserialize, Default)]
pub struct Output {
    pub title: String,
    /// Textures to change before drawing, and to free after.
    pub textures: egui::TexturesDelta,
    /// The triangles, each with the rectangle it is clipped to.
    pub meshes: Vec<(egui::Rect, egui::Mesh)>,
    pub pixels_per_point: f32,
    /// The cursor, copied text, a link to open, the text cursor for IME.
    pub platform: egui::PlatformOutput,
    /// Commands for the window (its theme, focus, size, ...).
    pub commands: Vec<egui::ViewportCommand>,
    /// Draw again after this many seconds; `None`: when something happens.
    pub repaint_after: Option<f64>,
}

impl Output {
    /// The triangles as egui hands them to a renderer.
    pub fn primitives(&self) -> Vec<egui::ClippedPrimitive> {
        self.meshes.iter().map(|(clip, mesh)| egui::ClippedPrimitive { clip_rect: *clip, primitive: egui::epaint::Primitive::Mesh(mesh.clone()) }).collect()
    }
}

/// Encode a message.
pub fn encode<T: Serialize>(v: &T) -> Vec<u8> {
    bincode::serialize(v).expect("a window message encodes")
}

/// Decode a message; `None` when it isn't one (another egui).
pub fn decode<T: DeserializeOwned>(b: &[u8]) -> Option<T> {
    bincode::deserialize(b).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_goes_through() {
        let mut mesh = egui::Mesh::default();
        mesh.add_colored_rect(egui::Rect::from_min_size(egui::pos2(1.0, 2.0), egui::vec2(3.0, 4.0)), egui::Color32::RED);
        let out = Output {
            title: "Příliš 🦎".into(),
            meshes: vec![(egui::Rect::EVERYTHING, mesh)],
            pixels_per_point: 1.5,
            commands: vec![egui::ViewportCommand::Focus],
            repaint_after: Some(0.25),
            ..Default::default()
        };
        let back: Output = decode(&encode(&out)).unwrap();
        assert_eq!((back.title.as_str(), back.pixels_per_point, back.repaint_after), ("Příliš 🦎", 1.5, Some(0.25)));
        assert_eq!(back.meshes[0].1.vertices, out.meshes[0].1.vertices);
        assert_eq!(back.primitives().len(), 1);
        assert!(decode::<Init>(&[1, 2, 3]).is_none());
    }
}
