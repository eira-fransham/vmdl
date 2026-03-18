use bytemuck::{Pod, Zeroable};

use crate::{ModelError, Readable};

type Result<T> = std::result::Result<T, ModelError>;

#[derive(Debug, Default, Clone, Copy, Zeroable, Pod)]
#[repr(C)]
pub struct ColorRGBExp32 {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub exponent: i8,
}

impl ColorRGBExp32 {
    pub fn to_rgb32f(&self) -> [f32; 3] {
        let scale = 2f32.powi(self.exponent as i32);

        [self.r, self.g, self.b].map(|v| scale * v as f32)
    }
}

/// The vhv file contains raw vertex colors for props.
#[derive(Debug, Clone)]
pub struct Vhv {
    pub header: VhvHeader,
    pub meshes: Vec<VhvMesh>,
}

impl Vhv {
    pub fn read(data: &[u8]) -> Result<Self> {
        let header = <VhvHeader as Readable>::read(data)?;

        let Ok(mesh_count) = header.mesh_count.try_into() else {
            return Ok(Vhv {
                header,
                meshes: Default::default(),
            });
        };

        if header.vertex_size as usize != std::mem::size_of::<ColorRGBExp32>() {
            return Err(ModelError::IO(std::io::Error::other(format!(
                "Incorrect vertex color type of size {}, only ColorRgbExp32 is supported",
                header.vertex_size
            ))));
        }

        let mut meshes = Vec::with_capacity(mesh_count);

        let mut mesh_header_data = &data[std::mem::size_of::<VhvHeader>()..];

        for _ in 0..header.mesh_count {
            let Some((header_bytes, rest)) =
                mesh_header_data.split_at_checked(std::mem::size_of::<VhvMeshHeader>())
            else {
                break;
            };
            mesh_header_data = rest;

            let mesh_header = <VhvMeshHeader as Readable>::read(header_bytes)?;

            meshes.push(VhvMesh {
                header: mesh_header,
                vertices: Vec::with_capacity(mesh_header.vertex_count as _),
            });
        }

        for mesh in &mut meshes {
            let mut vertex_data = &data[mesh.header.vertex_offset as usize..];

            for _ in 0..mesh.header.vertex_count {
                let Some((vertex_bytes, rest)) =
                    vertex_data.split_at_checked(std::mem::size_of::<ColorRGBExp32>())
                else {
                    break;
                };
                vertex_data = rest;
                mesh.vertices
                    .push(<ColorRGBExp32 as Readable>::read(vertex_bytes)?);
            }
        }

        Ok(Vhv { header, meshes })
    }
}

#[derive(Debug, Clone)]
pub struct VhvMesh {
    pub header: VhvMeshHeader,
    pub vertices: Vec<ColorRGBExp32>,
}

#[derive(Debug, Clone, Copy, Zeroable, Pod)]
#[repr(C)]
pub struct VhvHeader {
    pub version: i32,
    pub checksum: u32,
    // TODO: What are these flags?
    pub flags: u32,
    pub vertex_size: u32,
    pub vertex_count: u32,
    pub mesh_count: i32,
    pub _unused: [u32; 4],
}

#[derive(Debug, Clone, Copy, Zeroable, Pod)]
#[repr(C)]
pub struct VhvMeshHeader {
    pub lod: u32,
    pub vertex_count: u32,
    pub vertex_offset: u32,
    pub _unused: [u32; 4],
}
