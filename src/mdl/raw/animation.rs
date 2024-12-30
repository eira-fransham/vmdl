use crate::compressed_vector::{Quaternion48, Quaternion64, Vector48};
use crate::mdl::{Bone, BoneId};
use crate::{
    index_range, read_relative, read_single, ModelError, Quaternion, RadianEuler, ReadRelative,
    Readable, ReadableRelative, Vector,
};
use bitflags::bitflags;
use bytemuck::{Pod, Zeroable};
use cgmath::Matrix4;
use itertools::Itertools;
use std::mem::size_of;

#[derive(Debug, Clone, Copy, Zeroable, Pod)]
#[repr(C)]
pub struct PoseParameterDescriptionHeader {
    name_index: i32,
    flags: i32,
    start: f32,
    end: f32,
    loop_range: f32,
}

static_assertions::const_assert_eq!(size_of::<PoseParameterDescriptionHeader>(), 20);

#[derive(Clone, Debug)]
pub struct PoseParameterDescription {
    pub name: String,
    pub flags: i32,
    pub start: f32,
    pub end: f32,
    pub loop_range: f32,
}

impl ReadRelative<'_> for PoseParameterDescription {
    type Header = PoseParameterDescriptionHeader;

    fn read(data: &[u8], header: Self::Header) -> Result<Self, ModelError> {
        Ok(PoseParameterDescription {
            name: read_single(data, header.name_index)?,
            flags: header.flags,
            start: header.start,
            end: header.end,
            loop_range: header.loop_range,
        })
    }
}

#[derive(Debug, Clone, Copy, Zeroable, Pod)]
#[repr(C)]
pub struct AnimationDescriptionHeader {
    base_ptr: i32,
    name_offset: i32,
    fps: f32,
    flags: i32,

    frame_count: i32,

    movement_count: i32,
    movement_offset: i32,

    _padding: [i32; 6],

    animation_block: i32,
    animation_index: i32, // non-zero when anim data isn't in sections

    ik_rule_count: i32,
    ik_rule_offset: i32,
    animation_block_ik_rule_index: i32,

    local_hierarchy_count: i32,
    local_hierarchy_offset: i32,

    section_offset: i32,
    section_frames: i32,

    zero_frame_span: i16,
    zero_frame_count: i16,
    zero_frame_offset: i32,

    zero_frame_stall_time: f32,
}

static_assertions::const_assert_eq!(size_of::<AnimationDescriptionHeader>(), 100);

#[derive(Clone, Debug)]
pub struct AnimationDescription {
    pub name: String,
    pub fps: f32,
    pub frame_count: usize,
    pub animations: Vec<Animation>,
}

impl ReadRelative<'_> for AnimationDescription {
    type Header = AnimationDescriptionHeader;

    fn read(data: &[u8], header: Self::Header) -> Result<Self, ModelError> {
        let mut animations = Vec::with_capacity(1);
        let mut offset = header.animation_index as usize;
        loop {
            let (animation, next_offset) = if header.animation_block == 0 {
                read_animation(data, offset, header.frame_count as usize)?
            } else {
                todo!("read animation from animation block");
            };
            animations.push(animation);
            if next_offset == 0 {
                break;
            }
            offset += next_offset;
        }

        Ok(AnimationDescription {
            name: read_single(data, header.name_offset)?,
            fps: header.fps,
            frame_count: header.frame_count as usize,
            animations,
        })
    }
}

#[derive(Debug, Clone, Copy, Zeroable, Pod)]
#[repr(C)]
pub struct AnimationBlock {
    start: i32,
    end: i32,
}

impl ReadableRelative for AnimationBlock {}

#[derive(Debug, Clone, Copy, Zeroable, Pod)]
#[repr(C)]
pub struct AnimationHeader {
    bone: BoneId,
    flags: AnimationFlags,
    next_offset: u16,
}

#[derive(Zeroable, Pod, Copy, Clone, Debug)]
#[repr(C)]
pub struct AnimationFlags(u8);

bitflags! {
    impl AnimationFlags: u8 {
        /// Vector48
        const STUDIO_ANIM_RAWPOS = 	0x00000001;
        /// Quaternion48
        const STUDIO_ANIM_RAWROT = 	0x00000002;
        /// mstudioanim_valueptr_t
        const STUDIO_ANIM_ANIMPOS = 0x00000004;
        /// mstudioanim_valueptr_t
        const STUDIO_ANIM_ANIMROT = 0x00000008;
        const STUDIO_ANIM_DELTA = 	0x00000010;
        /// Quaternion64
        const STUDIO_ANIM_RAWROT2 = 0x00000020;
    }
}

#[derive(Debug, Copy, Clone)]
struct AnimationValuePointers<'a> {
    offsets: [u16; 3],
    data: &'a [u8],
}

impl<'a> ReadRelative<'a> for AnimationValuePointers<'a> {
    type Header = [u16; 3];

    fn read(data: &'a [u8], header: Self::Header) -> Result<Self, ModelError> {
        Ok(AnimationValuePointers {
            offsets: header,
            data,
        })
    }
}

#[derive(Zeroable, Pod, Copy, Clone, Debug, Default)]
#[repr(C)]
struct ValueHeader {
    valid: u8,
    total: u8,
}

static_assertions::const_assert_eq!(size_of::<ValueHeader>(), size_of::<i16>());

impl ReadableRelative for ValueHeader {}

fn read_animation_values(
    frame: usize,
    animation_value_pointers: AnimationValuePointers,
) -> Result<[f32; 3], ModelError> {
    let [x, y, z] = animation_value_pointers
        .offsets
        .map::<_, Result<_, ModelError>>(|offset| {
            if offset == 0 {
                Ok(0)
            } else {
                let values: FrameValues = read_single(animation_value_pointers.data, offset)?;
                Ok(values.get(frame as u8)?)
            }
        });
    let [x, y, z] = [x?, y?, z?];
    Ok([x as f32, y as f32, z as f32])
}

/// I hate this data structure
///
/// Seems to be an array of
///
/// FrameValues {
///     header: ValueHeader,
///     values: [u16; self.header.valid]
/// }
///
/// each item containing `header.total` worth of frames (for frames larger than `header.valid` it re-uses the last valid data)
/// when looking up frame `k` we skip through the list of values until we find the value range for the frame
struct FrameValues<'a> {
    header: ValueHeader,
    data: &'a [u8], // data starting at self.header
}

impl<'a> ReadRelative<'a> for FrameValues<'a> {
    type Header = ValueHeader;

    fn read(data: &'a [u8], header: Self::Header) -> Result<Self, ModelError> {
        Ok(FrameValues {
            header,
            data: &data
                .get(size_of::<ValueHeader>()..)
                .ok_or(ModelError::OutOfBounds {
                    data: "animation frame data",
                    offset: size_of::<ValueHeader>(),
                })?,
        })
    }
}

impl FrameValues<'_> {
    pub fn get(&self, index: u8) -> Result<i16, ModelError> {
        if self.header.total <= index {
            let offset_count = self.header.valid;
            let offset = (offset_count as usize) * size_of::<ValueHeader>();
            let next: FrameValues = read_single(self.data, offset)?;
            if next.header.total == 0 {
                return Ok(0);
            }
            next.get(index - self.header.total)
        } else {
            let offset_count = if index < self.header.valid {
                index
            } else {
                self.header.valid - 1
            };
            let offset = (offset_count as usize) * size_of::<i16>();
            read_single(self.data, offset)
        }
    }
}

#[derive(Clone, Debug)]
pub enum RotationData {
    Quaternion48(Quaternion),
    Quaternion64(Quaternion),
    Animated(Vec<RadianEuler>),
    None,
}

impl From<Quaternion48> for RotationData {
    fn from(value: Quaternion48) -> Self {
        let q = Quaternion::from(value);
        RotationData::Quaternion48(q)
    }
}

impl From<Quaternion64> for RotationData {
    fn from(value: Quaternion64) -> Self {
        let q = Quaternion::from(value);
        RotationData::Quaternion64(q)
    }
}

impl From<Vec<RadianEuler>> for RotationData {
    fn from(value: Vec<RadianEuler>) -> Self {
        // axis get fixed up when applying the scale
        RotationData::Animated(value)
    }
}

impl RotationData {
    pub fn rotation(&self, frame: usize) -> Quaternion {
        match self {
            RotationData::Quaternion48(q) => *q,
            RotationData::Quaternion64(q) => *q,
            RotationData::Animated(values) => values
                .get(frame)
                .copied()
                .unwrap_or_else(|| values.last().copied().unwrap_or_default())
                .into(),
            RotationData::None => Quaternion::default(),
        }
    }

    pub fn size(&self) -> usize {
        match self {
            RotationData::Quaternion48(_) => size_of::<Quaternion48>(),
            RotationData::Quaternion64(_) => size_of::<Quaternion64>(),
            RotationData::Animated(_) => {
                size_of::<<AnimationValuePointers<'_> as ReadRelative>::Header>()
            }
            RotationData::None => 0,
        }
    }

    fn set_scale(&mut self, scale: RadianEuler) {
        if let RotationData::Animated(values) = self {
            values.iter_mut().for_each(|value| {
                *value = RadianEuler {
                    roll: value.roll * scale.roll,
                    pitch: value.pitch * scale.pitch,
                    yaw: value.yaw * scale.yaw,
                }
            });
        }
    }

    fn set_base_rotation(&mut self, base: RadianEuler) {
        if let RotationData::Animated(values) = self {
            values.iter_mut().for_each(|value| {
                *value = RadianEuler {
                    roll: value.roll + base.roll,
                    pitch: value.pitch + base.pitch,
                    yaw: value.yaw + base.yaw,
                }
            });
        }
    }
}

#[derive(Clone, Debug)]
pub enum PositionData {
    Vector48(Vector48),
    PositionValues(Vec<Vector>),
    None,
}

impl PositionData {
    pub fn position(&self, frame: usize) -> Vector {
        match self {
            PositionData::Vector48(vector) => Vector::from(*vector),
            PositionData::PositionValues(values) => values
                .get(frame)
                .copied()
                .unwrap_or_else(|| values.last().copied().unwrap_or_default()),
            PositionData::None => Vector::default(),
        }
    }

    fn set_scale(&mut self, scale: Vector) {
        if let PositionData::PositionValues(values) = self {
            values.iter_mut().for_each(|value| {
                *value = Vector {
                    x: value.x * scale.x,
                    y: value.y * scale.y,
                    z: value.z * scale.z,
                }
            });
        }
    }
}

/// Per bone animation data
#[derive(Clone, Debug)]
pub struct Animation {
    pub bone: BoneId,
    pub flags: AnimationFlags,
    rotation_data: RotationData,
    position_data: PositionData,
}

impl Animation {
    pub fn rotation(&self, frame: usize) -> Quaternion {
        self.rotation_data.rotation(frame)
    }

    pub fn translation(&self, frame: usize) -> Vector {
        self.position_data.position(frame)
    }

    pub fn transform(&self, frame: usize) -> Matrix4<f32> {
        Matrix4::from(self.rotation(frame))
            * Matrix4::from_translation(self.translation(frame).into())
    }

    pub(crate) fn apply_bone_data(&mut self, bone: &Bone) {
        self.rotation_data.set_scale(bone.rot_scale);
        if self.flags.contains(AnimationFlags::STUDIO_ANIM_DELTA) {
            self.rotation_data.set_base_rotation(bone.rot);
        }
        self.position_data.set_scale(bone.pos_scale);
    }
}

fn read_animation(
    data: &[u8],
    header_offset: usize,
    frames: usize,
) -> Result<(Animation, usize), ModelError> {
    let data = data.get(header_offset..).ok_or(ModelError::OutOfBounds {
        data: "animation data",
        offset: header_offset,
    })?;
    let header = <AnimationHeader as Readable>::read(data)?;

    let offset = size_of::<AnimationHeader>();

    let rotation_data = if header.flags.contains(AnimationFlags::STUDIO_ANIM_RAWROT) {
        RotationData::from(read_single::<Quaternion48, _>(data, offset)?)
    } else if header.flags.contains(AnimationFlags::STUDIO_ANIM_RAWROT2) {
        RotationData::from(read_single::<Quaternion64, _>(data, offset)?)
    } else if header.flags.contains(AnimationFlags::STUDIO_ANIM_ANIMROT) {
        let pointers: AnimationValuePointers = read_single(data, offset)?;
        let values: Vec<RadianEuler> = (0..frames)
            .map(|frame| read_animation_values(frame, pointers))
            .map_ok(|[pitch, yaw, roll]| RadianEuler { pitch, yaw, roll })
            .collect::<Result<_, ModelError>>()?;
        RotationData::from(values)
    } else {
        RotationData::None
    };

    let position_offset = offset + rotation_data.size();
    let position_data = if header.flags.contains(AnimationFlags::STUDIO_ANIM_RAWPOS) {
        PositionData::Vector48(read_single(data, position_offset)?)
    } else if header.flags.contains(AnimationFlags::STUDIO_ANIM_ANIMPOS) {
        let pointers: AnimationValuePointers = read_single(data, position_offset)?;
        let values = (0..frames)
            .map(|frame| read_animation_values(frame, pointers))
            .map_ok(Vector::from)
            .collect::<Result<_, ModelError>>()?;
        PositionData::PositionValues(values)
    } else {
        PositionData::None
    };

    Ok((
        Animation {
            bone: header.bone,
            flags: header.flags,
            rotation_data,
            position_data,
        },
        header.next_offset as usize,
    ))
}

#[derive(Zeroable, Pod, Copy, Clone, Debug, Default)]
#[repr(C)]
pub struct AnimationSequenceHeader {
    base: i32,
    label_index: i32,
    activity_name_index: i32,
    flags: i32, // todo
    activity: i32,
    weight: i32,
    event_count: i32,
    event_offset: i32,
    bounding_box_min: Vector,
    bounding_box_max: Vector,
    blend_count: i32,
    animation_index_index: i32,
    movement_index: i32,
    group_size: [i32; 2],
    param_index: [i32; 2],
    param_start: [i32; 2],
    param_end: [i32; 2],
    param_parent: i32,

    fade_in_time: f32,
    fade_out_time: f32,

    local_entry_node: i32,
    local_exit_node: i32,
    node_flags: i32,

    entry_phase: f32,
    exit_phase: f32,

    last_frame: f32,

    next_sequence: i32,
    pose: i32,

    ik_rule_count: i32,

    auto_layer_count: i32,
    auto_layer_offset: i32,

    weight_list_offset: i32,

    pose_key_offset: i32,

    ik_lock_count: i32,
    ik_lock_offset: i32,

    key_value_offset: i32,
    key_value_size: i32,

    cycle_pose_offset: i32,

    activity_modifiers_offset: i32,
    activity_modifiers_count: i32,

    _padding: [i32; 5],
}

impl AnimationSequenceHeader {
    fn bone_weight_indices(&self) -> impl Iterator<Item = usize> {
        // weight/bone count isn't stored here, so we assume the next indexed values is stored after it in the file
        // we trim down the list of weights later
        let other_indices = [
            self.pose_key_offset,
            self.ik_lock_offset,
            self.key_value_offset,
            self.activity_modifiers_offset,
        ];
        let weight_count = if let Some(next_index) = other_indices
            .iter()
            .copied()
            .find(|index| *index > self.weight_list_offset)
        {
            (next_index - self.weight_list_offset) as usize / size_of::<f32>()
        } else {
            0
        };
        index_range(
            self.weight_list_offset,
            weight_count as i32,
            size_of::<f32>(),
        )
    }
}

#[derive(Debug, Clone)]
pub struct AnimationSequence {
    pub name: String,
    pub label: String,
    pub bone_weights: Vec<f32>,
}

impl ReadRelative<'_> for AnimationSequence {
    type Header = AnimationSequenceHeader;

    fn read(data: &[u8], header: Self::Header) -> Result<Self, ModelError> {
        Ok(AnimationSequence {
            name: read_single(data, header.activity_name_index)?,
            label: read_single(data, header.label_index)?,
            bone_weights: read_relative(data, header.bone_weight_indices())?,
        })
    }
}
