//! The interpreter's output: a flat vertex array plus draw batches, ready
//! for [`crate::Renderer`]. Nothing here depends on the output resolution.

use crate::combiner::CombinerKey;
use crate::rdp::{BlendState, DepthState};
use crate::texture::TileBinding;
use bytemuck::{Pod, Zeroable};
use pw64_formats::Image;
use std::collections::HashMap;
use std::sync::Arc;

/// N64 framebuffer size all screen coordinates refer to.
pub const N64_WIDTH: f32 = 320.0;
pub const N64_HEIGHT: f32 = 240.0;

/// One GPU vertex.
///
/// `pos` is in "N64 screen clip space": `pos.xy / pos.w` are N64 pixel
/// coordinates (0..320, 0..240, y down) and `pos.z / pos.w` is reversed
/// depth (1 = near, 0 = far). The renderer maps this to the target.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct Vertex {
    pub pos: [f32; 4],
    /// Shade RGBA 0..1 (alpha = fog factor when `G_FOG` is on).
    pub color: [f32; 4],
    /// Texel coordinates after `gSPTexture` scaling (before tile shift).
    pub st: [f32; 2],
    /// S/T clamp range (min s, min t, max s, max t), same space as `st`.
    /// Texture rectangles only sample their own texels on the RDP; at
    /// higher resolutions bilinear filtering would reach past the rect's
    /// edge texels, so they are clamped. [`Vertex::NO_CLAMP`] otherwise.
    pub st_clamp: [f32; 4],
}

impl Vertex {
    pub const NO_CLAMP: [f32; 4] = [-1.0e9, -1.0e9, 1.0e9, 1.0e9];
}

/// Per-draw uniforms (WGSL `struct Draw`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct DrawUniforms {
    /// Screen transform, filled in by the renderer: `x' = x*k.x + w*k.y`,
    /// `y' = y*k.z + w*k.w`.
    pub screen: [f32; 4],
    pub prim: [f32; 4],
    pub env: [f32; 4],
    pub fog: [f32; 4],
    pub blend: [f32; 4],
    pub fill: [f32; 4],
    /// x = prim LOD fraction, y = unused (LOD_FRACTION is computed per
    /// pixel from `lodp`), z = K4, w = K5.
    pub lod: [f32; 4],
    /// Per tile: (shift S, shift T, origin S, origin T), (1/w, 1/h, 0, 0).
    pub tile0: [f32; 4],
    pub size0: [f32; 4],
    pub tile1: [f32; 4],
    pub size1: [f32; 4],
    /// Per-pixel RDP LOD (see `shader.rs` `rdp_lod`): x = [`LodMode`] as
    /// f32, y = max level (`gSPTexture` level), z = ½ if the chain is
    /// sampled bilinearly (per-level texel-center offset, `LodMode::Chain`),
    /// w = 0. All zero = LOD_FRACTION 0 (non-mipmapped draws).
    pub lodp: [f32; 4],
    /// Filled in by the renderer (like `screen`): N64 3-point filter per
    /// texel (x = TEXEL0, y = TEXEL1: 1 = on) and their wrap codes
    /// (z, w: S mode + 3 × T mode; 0 clamp, 1 repeat, 2 mirror).
    pub filt: [f32; 4],
    /// Birdman64VR, filled in by the renderer for a stereo eye pass
    /// (`Renderer::render_stereo`): x = 1 = the vertex position is
    /// `pos * eye` instead of the screen transform. 0 everywhere else.
    pub vr: [f32; 4],
    /// The eye pass's matrix (row-vector convention, like `matrix::Mat4`).
    pub eye: [[f32; 4]; 4],
    /// Birdman64VR, island model (`StereoClip`): `pos * clip` = the point
    /// relative to the model's centre in game units, z up. Fragments
    /// farther out than `vr.y` or higher than `vr.z` are dropped. Only read
    /// by the clipping shader variant.
    pub clip: [[f32; 4]; 4],
}

/// How a draw gets its LOD fraction / mip tiles (`DrawUniforms::lodp.x`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LodMode {
    /// LOD_FRACTION = 0, TEXEL0/1 = the bound tiles (rects, max level 0,
    /// combiners that don't read LOD_FRACTION).
    Off = 0,
    /// `G_TL_TILE`: TEXEL0/1 = render tile / +1, LOD_FRACTION per pixel.
    Tile = 1,
    /// `G_TL_LOD` with the tiles forming a proper mip chain, bound as one
    /// texture with those levels ([`Frame::chains`]): the shader picks the
    /// levels per pixel.
    Chain = 2,
}

/// How the fragment shader produces its color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShaderMode {
    /// Combiner + blender.
    Normal,
    /// `G_CYC_FILL`: the fill color.
    Fill,
    /// `G_CYC_COPY`: TEXEL0 as-is.
    Copy,
    /// Fill rectangle into the depth buffer: writes the fill word's depth
    /// (vertex z), no color.
    DepthClear,
    /// Combiner/blender draw with the color image pointed at the z image
    /// (`uvGfxStateDrawDL` shadow volumes): the RDP depth-tests the triangle
    /// against the z-buffer and writes the blended "color" as the new z.
    /// The renderer does this in two steps (stencil mark, then depth write).
    DepthImage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ShaderKey {
    pub mode: ShaderMode,
    pub combiner: CombinerKey,
    pub blend: BlendState,
    /// `ALPHA_CVG_SEL` without `CVG_X_ALPHA`: blender alpha = coverage (1).
    pub alpha_cvg_sel: bool,
}

/// Everything that selects a GPU pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PipelineKey {
    pub shader: ShaderKey,
    pub depth: DepthState,
    /// `G_CULL_FRONT` = 1, `G_CULL_BACK` = 2 (both = nothing drawn).
    pub cull: u8,
}

/// A run of triangles sharing all state.
#[derive(Debug, Clone, PartialEq)]
pub struct DrawCall {
    pub pipeline: PipelineKey,
    pub first_vertex: u32,
    pub vertex_count: u32,
    pub uniforms: DrawUniforms,
    pub textures: [Option<TileBinding>; 2],
    /// Scissor in N64 pixels: x0, y0, x1, y1.
    pub scissor: [f32; 4],
    /// 3D geometry (clipped to the viewport × clip ratio) vs 2D rectangles.
    pub is_3d: bool,
    /// Widescreen behaviour (only used with `RenderOptions::widescreen`).
    pub wide: Wide,
    /// Widescreen HUD edge anchor (only moves `Wide::Fixed` draws).
    pub anchor: Anchor,
    /// Fill-view HUD vertical anchor (only moves `Wide::Fixed` draws).
    pub vanchor: VAnchor,
    /// For [`Wide::Stretch`]: the vertices' min/max row in N64 pixels
    /// (a scissor must span the main view vertically for the fill view to
    /// stretch it; see [`Wide::MAIN_VIEW_Y`]). `[0, 0]` otherwise.
    pub stretch_y: [f32; 2],
    /// Flight-HUD draw (the native game's `G_NOOP` "PWH" tags,
    /// `interp::HUD_TAG_ON`, `pw64_hud_tag`): the only draws the OLED care
    /// options (drift + dim) apply to. Ignored when OLED care is off
    /// (default options are bit-identical).
    pub hud: bool,
    /// Split-screen view rect (N64 px x0, row0, x1, row1; the native game's
    /// `G_NOOP` "PWS" tags, `interp::SPLIT_TAG`); `None` = full screen.
    /// With `hud`: a HUD draw laid out into the view
    /// (`Renderer::split_placement`, `pw64_hud_split_tag`). Without: a
    /// left/right world view stretched to the output edge
    /// (`Renderer::split_world_placement`, `pw64_widescreen_tag`).
    pub split: Option<[f32; 4]>,
    /// Movable flight-HUD element (the native game's `G_NOOP` "PWE" tags,
    /// `interp::HUD_ELEM_TAG`, `pw64_hud_element`); 0 = none. A hud
    /// `Wide::Fixed` draw outside split-screen moves by
    /// `RenderOptions::hud_offsets[elem]` (`crate::hud`).
    pub elem: u8,
    /// Birdman64VR: a draw of the 3D world view (perspective, inside the
    /// world-view tags): 1 + its index in [`Frame::views`]; 0 for
    /// everything else. A stereo eye pass draws these through the eye's
    /// own camera (`Renderer::render_stereo`).
    pub view: u16,
}

/// Horizontal HUD anchor in widescreen: a `Wide::Fixed` draw is shifted by
/// the side margin (output width beyond 4:3, halved) towards its edge, its
/// scissor with it. Set by the native game's `G_NOOP` anchor markers
/// (`interp::ANCHOR_TAG`, `pw64_widescreen.c`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Anchor {
    Left,
    #[default]
    Centre,
    Right,
}

impl Anchor {
    /// Multiple of the side margin a draw is moved by.
    pub fn sign(self) -> f32 {
        match self {
            Anchor::Left => -1.0,
            Anchor::Centre => 0.0,
            Anchor::Right => 1.0,
        }
    }
}

/// Vertical HUD anchor for the fill view (S17): a `Wide::Fixed` draw moves
/// from its old letterboxed position to the true screen edge, scissor with
/// it. Set by the native game's `G_NOOP` marker (`interp::VANCHOR_TAG`,
/// "PWV"); only applies when `RenderOptions::fill_view` is on (with the
/// letterbox bars there is nothing to gain).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VAnchor {
    Top,
    #[default]
    Middle,
    Bottom,
}

impl VAnchor {
    /// Rows a draw moves by (N64 top-down rows): Top up to the true top
    /// edge, Bottom down to the true bottom edge. The timer keeps roughly
    /// its old gap to the edge; the bottom gauges end ~8 rows from the edge
    /// instead of 20 (tune visually).
    pub fn rows(self) -> f32 {
        match self {
            VAnchor::Top => -8.0,
            VAnchor::Middle => 0.0,
            VAnchor::Bottom => 12.0,
        }
    }
}

/// How a draw is placed when the output is wider than 4:3.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Wide {
    /// HUD/2D: stays in the centred 4:3 area.
    Fixed,
    /// Perspective 3D: same mapping as 4:3, but not cropped at the 4:3
    /// edges when its scissor is the main view (Hor+).
    Extend,
    /// Untextured and spanning the main view (clears, fades, letterbox
    /// bars): N64 x range `[x0, x1]` is stretched over the full output width.
    Stretch([f32; 2]),
}

impl Wide {
    /// The main view's x range (N64 pixels): a scissor or a stretched draw
    /// must reach at least this far both ways. Covers the full width and
    /// the flight camera's overscan-inset subscreen (x 10..310).
    pub const MAIN_VIEW: [f32; 2] = [16.0, N64_WIDTH - 16.0];
    /// The main view's row range for the fill view (S17): a scissor or a
    /// stretched fill must reach at least this far up/down to be given the
    /// output area's full height / stretched onto it. Covers the world
    /// view's rows (8..222), the replay fades (7..223) and the sky clip box
    /// (3..227).
    pub const MAIN_VIEW_Y: [f32; 2] = [10.0, 220.0];
    /// The flight HUD's layout frame (N64 px x0, row0, x1, row1): the
    /// overscan-inset subscreen. Split-screen maps it into each view.
    pub const SUBSCREEN: [f32; 4] = [10.0, 8.0, 310.0, 222.0];
}

/// One processed display list.
#[derive(Default, Clone)]
pub struct Frame {
    pub vertices: Vec<Vertex>,
    pub draws: Vec<DrawCall>,
    /// Every texture the draws reference, by content key.
    pub textures: HashMap<u64, Arc<Image>>,
    /// `LodMode::Chain` textures: chain key → the level textures' content
    /// keys (level 0 first; each in `textures`, each half the previous).
    pub chains: HashMap<u64, Arc<[u64]>>,
    /// Birdman64VR: per projection + viewport the world view was drawn
    /// with ([`DrawCall::view`]), the matrix from a vertex's `pos` back to
    /// camera space: `pos * views[i]` = the point as the game's camera
    /// sees it, x right, y up, looking down -z, in game units (metres).
    pub views: Vec<[[f32; 4]; 4]>,
    /// Birdman64VR: the world view is to be shown as a small model of the
    /// island (outside flight; `pw64_vr_diorama` on the C side).
    pub diorama: Option<Diorama>,
}

/// Birdman64VR: what the game told the renderer about a world view shown
/// as a model of the island (`pw64_diorama_tag`), in game units (metres,
/// z up).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Diorama {
    /// The game camera's world matrix: rows right, forward, up, position.
    pub camera: [[f32; 3]; 4],
    /// The island's bounds: min x, y, z, max x, y, z.
    pub bounds: [f32; 6],
    /// The title screen: the headset follows the game camera (level,
    /// a little behind it) at full size instead of showing a model; the
    /// game's title moves that camera slower in VR.
    pub title: bool,
    /// A badge celebration: followed at full size too (its camera stands
    /// still), without the title's offset.
    pub celebration: bool,
    /// The game state and, on the title, its phase (-1 otherwise): for
    /// the log.
    pub state: i32,
    pub title_phase: i32,
}

impl Diorama {
    /// From the 21 values in tag order.
    pub fn from_values(v: &[f32; 21]) -> Self {
        let r = |i: usize| [v[i * 3], v[i * 3 + 1], v[i * 3 + 2]];
        Self {
            camera: [r(0), r(1), r(2), r(3)],
            bounds: [v[12], v[13], v[14], v[15], v[16], v[17]],
            title: v[18] == 1.0,
            celebration: v[18] == 2.0,
            state: v[19] as i32,
            title_phase: v[20] as i32,
        }
    }

    /// Camera space ([`Frame::views`]: x right, y up, -z forward) → game
    /// world (z up), row-vector convention.
    pub fn camera_to_world(&self) -> [[f32; 4]; 4] {
        Self::to_world(&self.camera)
    }

    /// [`Diorama::camera_to_world`] for a camera world matrix `camera`
    /// (rows right, forward, up, position).
    pub fn to_world(camera: &[[f32; 3]; 4]) -> [[f32; 4]; 4] {
        let [right, forward, up, pos] = *camera;
        let row = |v: [f32; 3], w: f32| [v[0], v[1], v[2], w];
        [
            row(right, 0.0),
            row(up, 0.0),
            row(forward.map(|c| -c), 0.0),
            row(pos, 1.0),
        ]
    }
}

impl Frame {
    pub fn triangle_count(&self) -> usize {
        self.vertices.len() / 3
    }

    /// Birdman64VR: the frame has a single full-screen 3D world view (no
    /// split-screen), so it can be shown in stereo.
    pub fn has_world(&self) -> bool {
        !self.views.is_empty()
            && self.draws.iter().any(|d| d.view != 0)
            && self.draws.iter().all(|d| d.split.is_none())
    }
}
