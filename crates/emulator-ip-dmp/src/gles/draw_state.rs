//! DMP register state captured for one draw.
use super::{Engine, MAX_BYTES};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct Float24(u32);

impl Float24 {
    fn from_register(value: u32) -> Self {
        Self(value)
    }

    pub(super) fn value(self) -> f32 {
        super::float24(self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct Viewport {
    pub(super) x_origin: u16,
    pub(super) y_origin: u16,
    x_scale: Float24,
    y_scale: Float24,
    z_scale: Float24,
    z_offset: Float24,
}

impl Viewport {
    pub(super) fn project(&self, position: [f32; 4], point: bool) -> [f32; 4] {
        let w = if point { 1. } else { position[3] };
        [
            (position[0] / w + 1.) * self.x_scale.value() + self.x_origin as f32,
            (position[1] / w + 1.) * self.y_scale.value() + self.y_origin as f32,
            position[2] / w * self.z_scale.value() + self.z_offset.value(),
            1. / w,
        ]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct TargetExtent {
    pub(super) packed: u32,
    pub(super) width: usize,
    pub(super) height: usize,
}

impl TargetExtent {
    pub(super) fn dimensions(self) -> Result<(usize, usize), String> {
        if self.width == 0
            || self.height == 0
            || !self.width.is_multiple_of(8)
            || !self.height.is_multiple_of(8)
            || self.width * self.height * 4 > MAX_BYTES
        {
            return Err(format!(
                "DMP unsupported 3D target extent {:#x}",
                self.packed
            ));
        }
        Ok((self.width, self.height))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum ColorTargetFormat {
    Rgba8,
    Rgb565,
    Rgb5A1,
    Rgba4,
    Other(u32),
}

impl ColorTargetFormat {
    fn from_register(value: u32) -> Self {
        match value {
            0 => Self::Rgba8,
            1 => Self::Rgb565,
            2 => Self::Rgb5A1,
            3 => Self::Rgba4,
            other => Self::Other(other),
        }
    }

    pub(super) fn code(self) -> u32 {
        match self {
            Self::Rgba8 => 0,
            Self::Rgb565 => 1,
            Self::Rgb5A1 => 2,
            Self::Rgba4 => 3,
            Self::Other(value) => value,
        }
    }

    pub(super) fn bytes_per_pixel(self) -> Result<usize, String> {
        match self {
            Self::Rgba8 => Ok(4),
            Self::Rgb565 | Self::Rgb5A1 | Self::Rgba4 => Ok(2),
            Self::Other(value) => Err(format!("DMP unsupported color target format {value}")),
        }
    }

    pub(super) fn unpack_code(self) -> Result<u32, String> {
        match self {
            Self::Rgba8 => Ok(0),
            Self::Rgb565 => Ok(3),
            Self::Rgb5A1 => Ok(2),
            Self::Rgba4 => Ok(4),
            Self::Other(value) => Err(format!("DMP unsupported color target format {value}")),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum DepthTargetFormat {
    Depth16,
    Depth24,
    Depth24Stencil8,
    Other(u32),
}

impl DepthTargetFormat {
    fn from_register(value: u32) -> Self {
        match value {
            0 => Self::Depth16,
            2 => Self::Depth24,
            3 => Self::Depth24Stencil8,
            other => Self::Other(other),
        }
    }

    pub(super) fn code(self) -> u32 {
        match self {
            Self::Depth16 => 0,
            Self::Depth24 => 2,
            Self::Depth24Stencil8 => 3,
            Self::Other(value) => value,
        }
    }

    pub(super) fn bytes_per_pixel(self) -> Result<usize, String> {
        match self {
            Self::Depth16 => Ok(2),
            Self::Depth24 => Ok(3),
            Self::Depth24Stencil8 => Ok(4),
            Self::Other(value) => Err(format!("DMP unsupported depth format {value:#x}")),
        }
    }

    pub(super) fn depth_maximum(self) -> Result<u32, String> {
        match self {
            Self::Depth16 => Ok(65535),
            Self::Depth24 | Self::Depth24Stencil8 => Ok(0xffffff),
            Self::Other(value) => Err(format!("DMP unsupported depth format {value:#x}")),
        }
    }

    pub(super) fn has_stencil(self) -> bool {
        self == Self::Depth24Stencil8
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct RenderTargets {
    pub(super) color_address: u64,
    pub(super) color_format: ColorTargetFormat,
    pub(super) depth_address: u64,
    pub(super) depth_format: DepthTargetFormat,
    pub(super) extent: TargetExtent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum TextureFormat {
    Rgba8,
    Rgb8,
    Rgb565,
    Rgb5A1,
    Rgba4,
    LuminanceAlpha8,
    Luminance8,
    Alpha8,
    Other(u32),
}

impl TextureFormat {
    fn from_register(value: u32) -> Self {
        match value {
            0 => Self::Rgba8,
            1 => Self::Rgb8,
            2 => Self::Rgb5A1,
            3 => Self::Rgb565,
            4 => Self::Rgba4,
            5 => Self::LuminanceAlpha8,
            7 => Self::Luminance8,
            8 => Self::Alpha8,
            other => Self::Other(other),
        }
    }

    pub(super) fn code(self) -> u32 {
        match self {
            Self::Rgba8 => 0,
            Self::Rgb8 => 1,
            Self::Rgb5A1 => 2,
            Self::Rgb565 => 3,
            Self::Rgba4 => 4,
            Self::LuminanceAlpha8 => 5,
            Self::Luminance8 => 7,
            Self::Alpha8 => 8,
            Self::Other(value) => value,
        }
    }

    pub(super) fn bytes_per_pixel(self) -> Result<usize, String> {
        match self {
            Self::Rgba8 => Ok(4),
            Self::Rgb8 => Ok(3),
            Self::Rgb565 | Self::Rgb5A1 | Self::Rgba4 | Self::LuminanceAlpha8 => Ok(2),
            Self::Luminance8 | Self::Alpha8 => Ok(1),
            Self::Other(value) => Err(format!("DMP unsupported sampler format {value}")),
        }
    }

    pub(super) fn unpack_code(self) -> Result<u32, String> {
        match self {
            Self::Rgba8 => Ok(0),
            Self::Rgb8 => Ok(1),
            Self::Rgb5A1 => Ok(2),
            Self::Rgb565 => Ok(3),
            Self::Rgba4 => Ok(4),
            Self::LuminanceAlpha8 => Ok(5),
            Self::Luminance8 => Ok(7),
            Self::Alpha8 => Ok(8),
            Self::Other(value) => Err(format!("DMP unsupported texture format {value}")),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum WrapMode {
    Clamp,
    Border,
    Repeat,
    Mirror,
    Other(u8),
}

impl WrapMode {
    fn from_code(code: u32) -> Self {
        match code {
            0 => Self::Clamp,
            1 => Self::Border,
            2 => Self::Repeat,
            3 => Self::Mirror,
            other => Self::Other(other as u8),
        }
    }

    pub(super) fn coordinate(self, value: i32, extent: usize) -> Option<usize> {
        match self {
            Self::Clamp => Some(value.clamp(0, extent as i32 - 1) as usize),
            Self::Border => {
                if value < 0 || value >= extent as i32 {
                    None
                } else {
                    Some(value as usize)
                }
            }
            Self::Repeat => Some(value.rem_euclid(extent as i32) as usize),
            Self::Mirror => {
                let position = value.rem_euclid(2 * extent as i32) as usize;
                Some(if position < extent {
                    position
                } else {
                    2 * extent - 1 - position
                })
            }
            Self::Other(_) => None,
        }
    }

    pub(super) fn is_supported(self) -> bool {
        !matches!(self, Self::Other(_))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct TextureUnit {
    pub(super) enabled: bool,
    pub(super) border_color: [u8; 4],
    pub(super) width: usize,
    pub(super) height: usize,
    pub(super) wrap_x: WrapMode,
    pub(super) wrap_y: WrapMode,
    pub(super) magnification_linear: bool,
    pub(super) minification_linear: bool,
    pub(super) mip_linear: bool,
    pub(super) lod_bias: i16,
    pub(super) max_level: u8,
    pub(super) base_address: u64,
    pub(super) mip_address: u64,
    pub(super) format: TextureFormat,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct CombinerStage {
    pub(super) rgb_sources: [u8; 3],
    pub(super) alpha_sources: [u8; 3],
    pub(super) rgb_operands: [u8; 3],
    pub(super) alpha_operands: [u8; 3],
    pub(super) rgb_operation: u8,
    pub(super) alpha_operation: u8,
    pub(super) constant_color: [u8; 4],
    pub(super) rgb_scale: u8,
    pub(super) alpha_scale: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct FogState {
    pub(super) enabled: bool,
    pub(super) table_enabled: bool,
    pub(super) color: [u8; 4],
    pub(super) scale: i16,
    pub(super) bias: i16,
    pub(super) table: Vec<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum CompareFunction {
    Never,
    Always,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
}

impl CompareFunction {
    fn from_code(code: u32) -> Self {
        match code & 7 {
            0 => Self::Never,
            1 => Self::Always,
            2 => Self::Equal,
            3 => Self::NotEqual,
            4 => Self::Less,
            5 => Self::LessEqual,
            6 => Self::Greater,
            _ => Self::GreaterEqual,
        }
    }

    pub(super) fn passes(self, a: u32, b: u32) -> bool {
        match self {
            Self::Never => false,
            Self::Always => true,
            Self::Equal => a == b,
            Self::NotEqual => a != b,
            Self::Less => a < b,
            Self::LessEqual => a <= b,
            Self::Greater => a > b,
            Self::GreaterEqual => a >= b,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct AlphaTest {
    pub(super) enabled: bool,
    pub(super) function: CompareFunction,
    pub(super) reference: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct DepthTest {
    pub(super) enabled: bool,
    pub(super) function: CompareFunction,
    pub(super) write_enabled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum StencilOperation {
    Keep,
    Zero,
    Replace,
    IncrementClamp,
    DecrementClamp,
    Invert,
    IncrementWrap,
    DecrementWrap,
}

impl StencilOperation {
    fn from_code(code: u32) -> Self {
        match code & 7 {
            0 => Self::Keep,
            1 => Self::Zero,
            2 => Self::Replace,
            3 => Self::IncrementClamp,
            4 => Self::DecrementClamp,
            5 => Self::Invert,
            6 => Self::IncrementWrap,
            _ => Self::DecrementWrap,
        }
    }

    pub(super) fn apply(self, old: u8, reference: u8) -> u8 {
        match self {
            Self::Keep => old,
            Self::Zero => 0,
            Self::Replace => reference,
            Self::IncrementClamp => old.saturating_add(1),
            Self::DecrementClamp => old.saturating_sub(1),
            Self::Invert => !old,
            Self::IncrementWrap => old.wrapping_add(1),
            Self::DecrementWrap => old.wrapping_sub(1),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct StencilTest {
    pub(super) enabled: bool,
    pub(super) function: CompareFunction,
    pub(super) reference: u8,
    pub(super) value_mask: u8,
    pub(super) write_mask: u8,
    pub(super) operations: [StencilOperation; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct ColorMask(pub(super) [bool; 4]);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum BlendFactor {
    Zero,
    One,
    SourceColor,
    OneMinusSourceColor,
    DestinationColor,
    OneMinusDestinationColor,
    SourceAlpha,
    OneMinusSourceAlpha,
    DestinationAlpha,
    OneMinusDestinationAlpha,
    ConstantColor,
    OneMinusConstantColor,
    ConstantAlpha,
    OneMinusConstantAlpha,
    SourceAlphaSaturate,
    Other(u8),
}

impl BlendFactor {
    fn from_code(code: u32) -> Self {
        match code {
            0 => Self::Zero,
            1 => Self::One,
            2 => Self::SourceColor,
            3 => Self::OneMinusSourceColor,
            4 => Self::DestinationColor,
            5 => Self::OneMinusDestinationColor,
            6 => Self::SourceAlpha,
            7 => Self::OneMinusSourceAlpha,
            8 => Self::DestinationAlpha,
            9 => Self::OneMinusDestinationAlpha,
            10 => Self::ConstantColor,
            11 => Self::OneMinusConstantColor,
            12 => Self::ConstantAlpha,
            13 => Self::OneMinusConstantAlpha,
            14 => Self::SourceAlphaSaturate,
            other => Self::Other(other as u8),
        }
    }

    pub(super) fn factor(
        self,
        channel: usize,
        source: [f32; 4],
        destination: [f32; 4],
        constant: [f32; 4],
    ) -> Result<f32, String> {
        Ok(match self {
            Self::Zero => 0.,
            Self::One => 1.,
            Self::SourceColor => source[channel],
            Self::OneMinusSourceColor => 1. - source[channel],
            Self::DestinationColor => destination[channel],
            Self::OneMinusDestinationColor => 1. - destination[channel],
            Self::SourceAlpha => source[3],
            Self::OneMinusSourceAlpha => 1. - source[3],
            Self::DestinationAlpha => destination[3],
            Self::OneMinusDestinationAlpha => 1. - destination[3],
            Self::ConstantColor => constant[channel],
            Self::OneMinusConstantColor => 1. - constant[channel],
            Self::ConstantAlpha => constant[3],
            Self::OneMinusConstantAlpha => 1. - constant[3],
            Self::SourceAlphaSaturate => {
                if channel == 3 {
                    1.
                } else {
                    source[3].min(1. - destination[3])
                }
            }
            Self::Other(_) => return Err("DMP unsupported blend factor".into()),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum BlendEquation {
    Add,
    Subtract,
    ReverseSubtract,
    Min,
    Max,
    Other(u8),
}

impl BlendEquation {
    fn from_code(code: u32) -> Self {
        match code {
            0 => Self::Add,
            1 => Self::Subtract,
            2 => Self::ReverseSubtract,
            3 => Self::Min,
            4 => Self::Max,
            other => Self::Other(other as u8),
        }
    }

    pub(super) fn apply(self, source: f32, destination: f32, original: f32) -> Result<f32, String> {
        match self {
            Self::Add => Ok(source + destination),
            Self::Subtract => Ok(source - destination),
            Self::ReverseSubtract => Ok(destination - source),
            Self::Min => Ok(original.min(destination)),
            Self::Max => Ok(original.max(destination)),
            Self::Other(_) => Err("DMP unsupported blend equation".into()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct BlendState {
    pub(super) enabled: bool,
    pub(super) logic_operation: u32,
    pub(super) source_rgb: BlendFactor,
    pub(super) destination_rgb: BlendFactor,
    pub(super) source_alpha: BlendFactor,
    pub(super) destination_alpha: BlendFactor,
    pub(super) rgb_equation: BlendEquation,
    pub(super) alpha_equation: BlendEquation,
    pub(super) constant_color: [u8; 4],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct Scissor {
    pub(super) left: u16,
    pub(super) top: u16,
    pub(super) right: u16,
    pub(super) bottom: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum AttributeKind {
    Signed8,
    Unsigned8,
    Signed16,
    Float32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct AttributeFormat {
    pub(super) components: u8,
    pub(super) kind: AttributeKind,
}

impl AttributeFormat {
    fn from_code(code: u32) -> Self {
        let kind = match code & 3 {
            0 => AttributeKind::Signed8,
            1 => AttributeKind::Unsigned8,
            2 => AttributeKind::Signed16,
            _ => AttributeKind::Float32,
        };
        Self {
            components: ((code >> 2) + 1) as u8,
            kind,
        }
    }

    pub(super) fn bytes_per_component(self) -> usize {
        match self.kind {
            AttributeKind::Signed8 | AttributeKind::Unsigned8 => 1,
            AttributeKind::Signed16 => 2,
            AttributeKind::Float32 => 4,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct VertexFetchGroup {
    pub(super) byte_offset: u64,
    pub(super) order: u32,
    pub(super) attribute_count: u8,
    pub(super) stride: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct VertexOutputMap(pub(super) [[u8; 4]; 7]);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct VertexLayout {
    pub(super) base_address: u64,
    pub(super) formats: [AttributeFormat; 8],
    pub(super) groups: [VertexFetchGroup; 8],
    pub(super) input_map: [u8; 8],
    pub(super) output_map: VertexOutputMap,
    pub(super) immediate_attribute_count: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum CullMode {
    Disabled,
    Front,
    Back,
    Other(u8),
}

impl CullMode {
    fn from_code(code: u32) -> Self {
        match code {
            0 => Self::Disabled,
            1 => Self::Front,
            2 => Self::Back,
            other => Self::Other(other as u8),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum PrimitiveMode {
    Triangles,
    TriangleStrip,
    TriangleFan,
    Lines,
    LineStrip,
    Points,
    Other(u8),
}

impl PrimitiveMode {
    fn from_code(code: u32) -> Self {
        match code {
            0 | 3 => Self::Triangles,
            1 => Self::TriangleStrip,
            2 => Self::TriangleFan,
            4 => Self::Lines,
            5 => Self::LineStrip,
            8 => Self::Points,
            other => Self::Other(other as u8),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct RasterState {
    pub(super) cull_mode: CullMode,
    pub(super) flat_shading: bool,
    pub(super) primitive_mode: PrimitiveMode,
    pub(super) line_stipple: bool,
    pub(super) line_half_width_32nds: u16,
    pub(super) line_smooth: bool,
    pub(super) point_coord_replace: [bool; 2],
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct DrawState {
    pub(super) viewport: Viewport,
    pub(super) targets: RenderTargets,
    pub(super) textures: [TextureUnit; 2],
    pub(super) combiners: [CombinerStage; 2],
    pub(super) fog: FogState,
    pub(super) alpha_test: AlphaTest,
    pub(super) depth_test: DepthTest,
    pub(super) stencil_test: StencilTest,
    pub(super) color_mask: ColorMask,
    pub(super) blend: BlendState,
    pub(super) scissor: Scissor,
    pub(super) vertex: VertexLayout,
    pub(super) raster: RasterState,
}

fn texture(engine: &Engine, unit: usize) -> TextureUnit {
    let (dimensions, control, levels, base_address, mip_address, format) = if unit == 0 {
        (0x282, 0x283, 0x284, 0x285, 0x289, 0x28e)
    } else {
        (0x292, 0x293, 0x294, 0x295, 0x299, 0x296)
    };
    let dimensions = engine.reg(dimensions);
    let control_value = engine.reg(control);
    let levels_value = engine.reg(levels);
    let format_value = engine.reg(format) & 15;
    TextureUnit {
        enabled: engine.reg(0x280) & (1 << unit) != 0,
        border_color: engine
            .reg(if unit == 0 { 0x281 } else { 0x291 })
            .to_le_bytes(),
        width: (dimensions >> 16) as usize,
        height: (dimensions & 65535) as usize,
        wrap_x: WrapMode::from_code(control_value >> 12 & 7),
        wrap_y: WrapMode::from_code(control_value >> 8 & 7),
        magnification_linear: control_value & 2 != 0,
        minification_linear: control_value & 4 != 0,
        mip_linear: control_value & (1 << 24) != 0,
        lod_bias: (((levels_value & 8191) as i32) << 19 >> 19) as i16,
        max_level: ((levels_value >> 16) & 15) as u8,
        base_address: engine.reg(base_address) as u64,
        mip_address: engine.reg(mip_address) as u64,
        format: TextureFormat::from_register(format_value),
    }
}

fn combiner(engine: &Engine, stage: usize) -> CombinerStage {
    let base = 0x2c0 + stage as u16 * 8;
    let sources = engine.reg(base);
    let operands = engine.reg(base + 1);
    let operations = engine.reg(base + 2);
    let scale = engine.reg(base + 4);
    let rgb_operation = (operations & 15) as u8;
    CombinerStage {
        rgb_sources: std::array::from_fn(|index| ((sources >> (index * 4)) & 15) as u8),
        alpha_sources: std::array::from_fn(|index| ((sources >> (16 + index * 4)) & 15) as u8),
        rgb_operands: std::array::from_fn(|index| ((operands >> (index * 4)) & 15) as u8),
        alpha_operands: std::array::from_fn(|index| ((operands >> (12 + index * 4)) & 15) as u8),
        rgb_operation,
        alpha_operation: if rgb_operation != 7 {
            (operations >> 16 & 15) as u8
        } else {
            rgb_operation
        },
        constant_color: engine.reg(base + 3).to_le_bytes(),
        rgb_scale: (scale & 3) as u8,
        alpha_scale: (scale >> 16 & 3) as u8,
    }
}

impl DrawState {
    fn decode(engine: &Engine) -> Self {
        let viewport_origin = engine.reg(0x268);
        let extent = engine.reg(0x31e);
        let alpha = engine.reg(0x304);
        let depth_control = engine.reg(0x307);
        let stencil = engine.reg(0x305);
        let operations = engine.reg(0x306);
        let blend_control = engine.reg(0x301);
        let scissor_origin = engine.reg(0x266);
        let scissor_end = engine.reg(0x267);
        let attribute_format = engine.reg(0x401);
        let fog_control = engine.reg(0x2e0);
        let stencil_ops = [operations & 7, operations >> 4 & 7, operations >> 8 & 7];
        Self {
            viewport: Viewport {
                x_origin: viewport_origin as u16,
                y_origin: (viewport_origin >> 16) as u16,
                x_scale: Float24::from_register(engine.reg(0x241)),
                y_scale: Float24::from_register(engine.reg(0x243)),
                z_scale: Float24::from_register(engine.reg(0x24d)),
                z_offset: Float24::from_register(engine.reg(0x24e)),
            },
            targets: RenderTargets {
                color_address: engine.reg(0x31d) as u64,
                color_format: ColorTargetFormat::from_register(engine.reg(0x317) >> 16 & 15),
                depth_address: engine.reg(0x31c) as u64,
                depth_format: DepthTargetFormat::from_register(engine.reg(0x316)),
                extent: TargetExtent {
                    packed: extent,
                    width: (extent & 8191) as usize,
                    height: (extent >> 16 & 8191) as usize,
                },
            },
            textures: [texture(engine, 0), texture(engine, 1)],
            combiners: [combiner(engine, 0), combiner(engine, 1)],
            fog: FogState {
                enabled: fog_control & 1 != 0,
                table_enabled: fog_control & 4 != 0,
                color: engine.reg(0x2e1).to_le_bytes(),
                scale: engine.reg(0x2e2) as u16 as i16,
                bias: engine.reg(0x2e3) as u16 as i16,
                table: engine.fog.clone(),
            },
            alpha_test: AlphaTest {
                enabled: alpha & 1 != 0,
                function: CompareFunction::from_code(alpha >> 4),
                reference: (alpha >> 8) as u8,
            },
            depth_test: DepthTest {
                enabled: depth_control & 1 != 0,
                function: CompareFunction::from_code(depth_control >> 4),
                write_enabled: depth_control & 0x1001 == 0x1001,
            },
            stencil_test: StencilTest {
                enabled: stencil & 1 != 0,
                function: CompareFunction::from_code(stencil >> 4),
                reference: (stencil >> 16) as u8,
                value_mask: (stencil >> 24) as u8,
                write_mask: (stencil >> 8) as u8,
                operations: stencil_ops.map(StencilOperation::from_code),
            },
            color_mask: ColorMask(std::array::from_fn(|channel| {
                depth_control >> (8 + channel) & 1 != 0
            })),
            blend: BlendState {
                enabled: engine.reg(0x300) & 0x100 != 0,
                logic_operation: engine.reg(0x302),
                source_rgb: BlendFactor::from_code(blend_control >> 16 & 15),
                destination_rgb: BlendFactor::from_code(blend_control >> 20 & 15),
                source_alpha: BlendFactor::from_code(blend_control >> 24 & 15),
                destination_alpha: BlendFactor::from_code(blend_control >> 28 & 15),
                rgb_equation: BlendEquation::from_code(blend_control & 255),
                alpha_equation: BlendEquation::from_code(blend_control >> 8 & 255),
                constant_color: engine.reg(0x303).to_le_bytes(),
            },
            scissor: Scissor {
                left: scissor_origin as u16,
                top: (scissor_origin >> 16) as u16,
                right: scissor_end as u16,
                bottom: (scissor_end >> 16) as u16,
            },
            vertex: VertexLayout {
                base_address: engine.reg(0x400) as u64 * 8,
                formats: std::array::from_fn(|slot| {
                    AttributeFormat::from_code(attribute_format >> (slot * 4) & 15)
                }),
                groups: std::array::from_fn(|group| {
                    let base = 0x403 + group as u16 * 3;
                    let extent = engine.reg(base + 2);
                    VertexFetchGroup {
                        byte_offset: engine.reg(base) as u64,
                        order: engine.reg(base + 1),
                        attribute_count: (extent >> 28) as u8,
                        stride: (extent >> 16 & 4095) as usize,
                    }
                }),
                input_map: std::array::from_fn(|slot| (engine.reg(0x48b) >> (slot * 4) & 15) as u8),
                output_map: VertexOutputMap(std::array::from_fn(|slot| {
                    let map = engine.reg(0x250 + slot as u16);
                    std::array::from_fn(|component| ((map >> (component * 8)) & 31) as u8)
                })),
                immediate_attribute_count: ((engine.reg(0x402) >> 28) + 1) as u8,
            },
            raster: RasterState {
                cull_mode: CullMode::from_code(engine.reg(0x240) & 3),
                flat_shading: engine.reg(0x258) & 1 != 0,
                primitive_mode: PrimitiveMode::from_code((engine.reg(0x45e) >> 8) & 15),
                line_stipple: engine.reg(0x271) & 1 != 0,
                line_half_width_32nds: engine.reg(0x278) as u16,
                line_smooth: engine.reg(0x270) & 0x20000 != 0,
                point_coord_replace: [
                    engine.reg(0x270) & (1 << 24) != 0,
                    engine.reg(0x270) & (1 << 25) != 0,
                ],
            },
        }
    }
}

impl Engine {
    pub(super) fn draw_state(&self) -> DrawState {
        DrawState::decode(self)
    }
}
