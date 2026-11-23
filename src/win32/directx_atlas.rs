use std::borrow::Cow;
use std::collections::hash_map::Entry;

use crate::ui::collections::FxHashMap;
use crate::ui::sync::Mutex;
use windows::Win32::Graphics::{
    Direct3D11::{
        D3D11_BIND_SHADER_RESOURCE, D3D11_BOX, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
        ID3D11Device, ID3D11DeviceContext, ID3D11ShaderResourceView, ID3D11Texture2D,
    },
    Dxgi::Common::*,
};

use crate::ui::{
    AtlasTextureId, AtlasTextureKind, AtlasTextureList, AtlasTile, Bounds, DevicePixels, Point,
    RenderGlyphParams, Size, TileId,
};

struct Shelf {
    y: i32,
    height: i32,
    next_x: i32,
}

/// Simple append-only shelf-based 2D rectangle allocator.
struct ShelfAtlasAllocator {
    width: i32,
    height: i32,
    next_y: i32,
    next_id: u32,
    shelves: Vec<Shelf>,
}

impl ShelfAtlasAllocator {
    fn new(size: Size<DevicePixels>) -> Self {
        Self {
            width: size.width.0,
            height: size.height.0,
            next_y: 0,
            next_id: 0,
            shelves: Vec::new(),
        }
    }

    fn allocate(&mut self, size: Size<DevicePixels>) -> Option<(TileId, Point<DevicePixels>)> {
        let w = size.width.0;
        let h = size.height.0;
        if w <= 0 || h <= 0 || w > self.width || h > self.height {
            return None;
        }

        let mut best: Option<(usize, i32)> = None;
        for (shelf_ix, shelf) in self.shelves.iter().enumerate() {
            if shelf.height < h || shelf.height > h.saturating_mul(2).saturating_add(8) {
                continue;
            }
            if self.width - shelf.next_x >= w {
                let waste = shelf.height - h;
                if best.is_none_or(|(_, best_waste)| waste < best_waste) {
                    best = Some((shelf_ix, waste));
                    if waste == 0 {
                        break;
                    }
                }
            }
        }

        let (x, y) = if let Some((shelf_ix, _)) = best {
            let shelf = &mut self.shelves[shelf_ix];
            let x = shelf.next_x;
            shelf.next_x += w;
            (x, shelf.y)
        } else {
            let shelf_h = ((h + 3) & !3).min(self.height - self.next_y);
            if shelf_h < h {
                return None;
            }
            let y = self.next_y;
            self.next_y += shelf_h;
            self.shelves.push(Shelf {
                y,
                height: shelf_h,
                next_x: w,
            });
            (0, y)
        };

        let id = self.next_id;
        self.next_id += 1;
        Some((
            TileId(id),
            Point {
                x: DevicePixels(x),
                y: DevicePixels(y),
            },
        ))
    }
}

pub(crate) struct DirectXAtlas(Mutex<DirectXAtlasInner>);

struct DirectXAtlasInner {
    tiles_by_key: FxHashMap<RenderGlyphParams, AtlasTile>,
    device: ID3D11Device,
    device_context: ID3D11DeviceContext,
    monochrome_textures: AtlasTextureList<DirectXAtlasTexture>,
    polychrome_textures: AtlasTextureList<DirectXAtlasTexture>,
}

struct DirectXAtlasTexture {
    id: AtlasTextureId,
    bytes_per_pixel: u32,
    allocator: ShelfAtlasAllocator,
    texture: ID3D11Texture2D,
    view: [Option<ID3D11ShaderResourceView>; 1],
}

impl DirectXAtlas {
    pub(crate) fn new(device: &ID3D11Device, device_context: &ID3D11DeviceContext) -> Self {
        DirectXAtlas(Mutex::new(DirectXAtlasInner {
            tiles_by_key: FxHashMap::default(),
            device: device.clone(),
            device_context: device_context.clone(),
            monochrome_textures: Default::default(),
            polychrome_textures: Default::default(),
        }))
    }

    pub(crate) fn get_or_insert_with<'a>(
        &self,
        key: RenderGlyphParams,
        mut build: impl FnMut() -> crate::ui::anyhow::Result<Option<(Size<DevicePixels>, Cow<'a, [u8]>)>>,
    ) -> crate::ui::anyhow::Result<Option<AtlasTile>> {
        let mut inner = self.0.lock();
        if let Some(&tile) = inner.tiles_by_key.get(&key) {
            return Ok(Some(tile));
        }
        let Some((size, bytes)) = build()? else {
            return Ok(None);
        };
        let tile = inner.insert(key.texture_kind(), size, &bytes)?;
        match inner.tiles_by_key.entry(key) {
            Entry::Occupied(mut entry) => {
                entry.insert(tile);
            }
            Entry::Vacant(entry) => {
                entry.insert(tile);
            }
        }
        Ok(Some(tile))
    }

    pub(crate) fn get_texture_view(
        &self,
        id: AtlasTextureId,
    ) -> Option<[Option<ID3D11ShaderResourceView>; 1]> {
        let lock = self.0.lock();
        let texture = lock.texture(id)?;
        Some(texture.view.clone())
    }

    pub(crate) fn handle_device_lost(
        &self,
        device: &ID3D11Device,
        device_context: &ID3D11DeviceContext,
    ) {
        let mut lock = self.0.lock();
        lock.tiles_by_key.clear();
        lock.device = device.clone();
        lock.device_context = device_context.clone();
        lock.monochrome_textures = AtlasTextureList::default();
        lock.polychrome_textures = AtlasTextureList::default();
    }
}

impl DirectXAtlasInner {
    fn insert(
        &mut self,
        kind: AtlasTextureKind,
        size: Size<DevicePixels>,
        bytes: &[u8],
    ) -> crate::ui::anyhow::Result<AtlasTile> {
        let tile = self
            .allocate(size, kind)
            .ok_or_else(|| crate::ui::anyhow::anyhow!("failed to allocate"))?;
        let texture = self
            .texture(tile.texture_id)
            .ok_or_else(|| crate::ui::anyhow::anyhow!("allocated tile refers to a missing texture"))?;
        texture.upload(&self.device_context, tile.bounds, bytes);
        Ok(tile)
    }

    fn allocate(
        &mut self,
        size: Size<DevicePixels>,
        texture_kind: AtlasTextureKind,
    ) -> Option<AtlasTile> {
        {
            let textures = match texture_kind {
                AtlasTextureKind::Monochrome => &mut self.monochrome_textures,
                AtlasTextureKind::Polychrome => &mut self.polychrome_textures,
            };

            if let Some(tile) = textures
                .iter_mut()
                .rev()
                .find_map(|texture| texture.allocate(size))
            {
                return Some(tile);
            }
        }

        let texture = self.push_texture(size, texture_kind)?;
        texture.allocate(size)
    }

    fn push_texture(
        &mut self,
        min_size: Size<DevicePixels>,
        kind: AtlasTextureKind,
    ) -> Option<&mut DirectXAtlasTexture> {
        const DEFAULT_ATLAS_SIZE: Size<DevicePixels> = Size {
            width: DevicePixels(1024),
            height: DevicePixels(1024),
        };
        const MAX_ATLAS_SIZE: Size<DevicePixels> = Size {
            width: DevicePixels(16384),
            height: DevicePixels(16384),
        };
        let size = min_size.min(&MAX_ATLAS_SIZE).max(&DEFAULT_ATLAS_SIZE);
        let (pixel_format, bytes_per_pixel) = match kind {
            AtlasTextureKind::Monochrome => (DXGI_FORMAT_R8_UNORM, 1),
            AtlasTextureKind::Polychrome => (DXGI_FORMAT_B8G8R8A8_UNORM, 4),
        };
        let texture_desc = D3D11_TEXTURE2D_DESC {
            Width: size.width.0 as u32,
            Height: size.height.0 as u32,
            MipLevels: 1,
            ArraySize: 1,
            Format: pixel_format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut texture: Option<ID3D11Texture2D> = None;
        unsafe {
            self.device
                .CreateTexture2D(&texture_desc, None, Some(&mut texture))
                .ok()?;
        }
        let texture = texture.unwrap();

        let texture_list = match kind {
            AtlasTextureKind::Monochrome => &mut self.monochrome_textures,
            AtlasTextureKind::Polychrome => &mut self.polychrome_textures,
        };
        let view = unsafe {
            let mut view = None;
            self.device
                .CreateShaderResourceView(&texture, None, Some(&mut view))
                .ok()?;
            [view]
        };
        let atlas_texture = DirectXAtlasTexture {
            id: AtlasTextureId {
                index: texture_list.textures.len() as u32,
                kind,
            },
            bytes_per_pixel,
            allocator: ShelfAtlasAllocator::new(size),
            texture,
            view,
        };
        texture_list.textures.push(Some(atlas_texture));
        texture_list.textures.last_mut().unwrap().as_mut()
    }

    fn texture(&self, id: AtlasTextureId) -> Option<&DirectXAtlasTexture> {
        let textures = match id.kind {
            AtlasTextureKind::Monochrome => &self.monochrome_textures,
            AtlasTextureKind::Polychrome => &self.polychrome_textures,
        };
        textures.textures.get(id.index as usize)?.as_ref()
    }
}

impl DirectXAtlasTexture {
    fn allocate(&mut self, size: Size<DevicePixels>) -> Option<AtlasTile> {
        let (tile_id, origin) = self.allocator.allocate(size)?;
        Some(AtlasTile {
            texture_id: self.id,
            tile_id,
            bounds: Bounds { origin, size },
            padding: 0,
        })
    }

    fn upload(
        &self,
        device_context: &ID3D11DeviceContext,
        bounds: Bounds<DevicePixels>,
        bytes: &[u8],
    ) {
        let row_bytes = bounds.size.width.to_bytes(self.bytes_per_pixel as u8) as usize;
        let expected = row_bytes * bounds.size.height.0.max(0) as usize;
        if bytes.len() < expected {
            return;
        }
        unsafe {
            device_context.UpdateSubresource(
                &self.texture,
                0,
                Some(&D3D11_BOX {
                    left: bounds.left().0 as u32,
                    top: bounds.top().0 as u32,
                    front: 0,
                    right: bounds.right().0 as u32,
                    bottom: bounds.bottom().0 as u32,
                    back: 1,
                }),
                bytes.as_ptr() as _,
                bounds.size.width.to_bytes(self.bytes_per_pixel as u8),
                0,
            );
        }
    }
}
