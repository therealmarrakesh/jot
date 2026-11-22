use std::borrow::Cow;
use std::collections::hash_map::Entry;

use gpui::collections::FxHashMap;
use gpui::sync::Mutex;
use windows::Win32::Graphics::{
    Direct3D11::{
        D3D11_BIND_SHADER_RESOURCE, D3D11_BOX, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
        ID3D11Device, ID3D11DeviceContext, ID3D11ShaderResourceView, ID3D11Texture2D,
    },
    Dxgi::Common::*,
};

use gpui::{
    AtlasKey, AtlasTextureId, AtlasTextureKind, AtlasTextureList, AtlasTile, Bounds, DevicePixels,
    Point, Size, TileId,
};

struct Shelf {
    y: i32,
    height: i32,
    free_spans: Vec<(i32, i32)>,
}

struct ShelfSlot {
    shelf_ix: usize,
    x: i32,
    width: i32,
}

/// Simple shelf-based 2D rectangle allocator with deallocation and span coalescing.
struct ShelfAtlasAllocator {
    width: i32,
    height: i32,
    next_y: i32,
    shelves: Vec<Shelf>,
    slots: Vec<Option<ShelfSlot>>,
    free_slot_ids: Vec<u32>,
}

impl ShelfAtlasAllocator {
    fn new(size: Size<DevicePixels>) -> Self {
        Self {
            width: size.width.0,
            height: size.height.0,
            next_y: 0,
            shelves: Vec::new(),
            slots: Vec::new(),
            free_slot_ids: Vec::new(),
        }
    }

    fn allocate(&mut self, size: Size<DevicePixels>) -> Option<(TileId, Point<DevicePixels>)> {
        let w = size.width.0;
        let h = size.height.0;
        if w <= 0 || h <= 0 || w > self.width || h > self.height {
            return None;
        }

        let mut best: Option<(usize, usize, i32)> = None;
        for (shelf_ix, shelf) in self.shelves.iter().enumerate() {
            if shelf.height < h || shelf.height > h.saturating_mul(2).saturating_add(8) {
                continue;
            }
            if let Some((span_ix, _)) = shelf
                .free_spans
                .iter()
                .enumerate()
                .find(|(_, (_, span_w))| *span_w >= w)
            {
                let waste = shelf.height - h;
                if best.is_none_or(|(_, _, best_waste)| waste < best_waste) {
                    best = Some((shelf_ix, span_ix, waste));
                    if waste == 0 {
                        break;
                    }
                }
            }
        }

        let (shelf_ix, x, y) = if let Some((shelf_ix, span_ix, _)) = best {
            let shelf = &mut self.shelves[shelf_ix];
            let (span_x, span_w) = shelf.free_spans[span_ix];
            if span_w == w {
                shelf.free_spans.remove(span_ix);
            } else {
                shelf.free_spans[span_ix] = (span_x + w, span_w - w);
            }
            (shelf_ix, span_x, shelf.y)
        } else {
            let shelf_h = ((h + 3) & !3).min(self.height - self.next_y);
            if shelf_h < h {
                return None;
            }
            let y = self.next_y;
            self.next_y += shelf_h;
            let mut free_spans = Vec::new();
            if w < self.width {
                free_spans.push((w, self.width - w));
            }
            let shelf_ix = self.shelves.len();
            self.shelves.push(Shelf {
                y,
                height: shelf_h,
                free_spans,
            });
            (shelf_ix, 0, y)
        };

        let slot = ShelfSlot {
            shelf_ix,
            x,
            width: w,
        };
        let id = if let Some(id) = self.free_slot_ids.pop() {
            self.slots[id as usize] = Some(slot);
            id
        } else {
            let id = self.slots.len() as u32;
            self.slots.push(Some(slot));
            id
        };
        Some((
            TileId(id),
            Point {
                x: DevicePixels(x),
                y: DevicePixels(y),
            },
        ))
    }

    fn deallocate(&mut self, id: TileId) {
        let Some(slot) = self.slots.get_mut(id.0 as usize).and_then(Option::take) else {
            return;
        };
        self.free_slot_ids.push(id.0);

        let shelf = &mut self.shelves[slot.shelf_ix];
        let pos = shelf
            .free_spans
            .partition_point(|&(span_x, _)| span_x < slot.x);
        shelf.free_spans.insert(pos, (slot.x, slot.width));

        if pos + 1 < shelf.free_spans.len() {
            let (lx, lw) = shelf.free_spans[pos];
            let (rx, rw) = shelf.free_spans[pos + 1];
            if lx + lw == rx {
                shelf.free_spans[pos] = (lx, lw + rw);
                shelf.free_spans.remove(pos + 1);
            }
        }
        if pos > 0 {
            let (lx, lw) = shelf.free_spans[pos - 1];
            let (rx, rw) = shelf.free_spans[pos];
            if lx + lw == rx {
                shelf.free_spans[pos - 1] = (lx, lw + rw);
                shelf.free_spans.remove(pos);
            }
        }
    }
}

pub(crate) struct DirectXAtlas(Mutex<DirectXAtlasInner>);

struct DirectXAtlasInner {
    tiles_by_key: FxHashMap<AtlasKey, AtlasTile>,
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
    live_atlas_keys: u32,
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
        key: AtlasKey,
        mut build: impl FnMut() -> crate::anyhow::Result<Option<(Size<DevicePixels>, Cow<'a, [u8]>)>>,
    ) -> crate::anyhow::Result<Option<AtlasTile>> {
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

    pub(crate) fn remove(&self, key: &AtlasKey) {
        let mut inner = self.0.lock();
        if let Some(tile) = inner.tiles_by_key.remove(key) {
            inner.remove_tile(tile);
        }
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
    ) -> crate::anyhow::Result<AtlasTile> {
        let tile = self
            .allocate(size, kind)
            .ok_or_else(|| crate::anyhow::anyhow!("failed to allocate"))?;
        let texture = self
            .texture(tile.texture_id)
            .ok_or_else(|| crate::anyhow::anyhow!("allocated tile refers to a missing texture"))?;
        texture.upload(&self.device_context, tile.bounds, bytes);
        Ok(tile)
    }

    fn remove_tile(&mut self, tile: AtlasTile) {
        let id = tile.texture_id;
        let textures = match id.kind {
            AtlasTextureKind::Monochrome => &mut self.monochrome_textures,
            AtlasTextureKind::Polychrome => &mut self.polychrome_textures,
        };

        let Some(texture_slot) = textures.textures.get_mut(id.index as usize) else {
            return;
        };

        if let Some(mut texture) = texture_slot.take() {
            texture.allocator.deallocate(tile.tile_id);
            texture.decrement_ref_count();
            if texture.is_unreferenced() {
                textures.free_list.push(texture.id.index as usize);
            } else {
                *texture_slot = Some(texture);
            }
        }
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
        let index = texture_list.free_list.pop();
        let view = unsafe {
            let mut view = None;
            self.device
                .CreateShaderResourceView(&texture, None, Some(&mut view))
                .ok()?;
            [view]
        };
        let atlas_texture = DirectXAtlasTexture {
            id: AtlasTextureId {
                index: index.unwrap_or(texture_list.textures.len()) as u32,
                kind,
            },
            bytes_per_pixel,
            allocator: ShelfAtlasAllocator::new(size),
            texture,
            view,
            live_atlas_keys: 0,
        };
        if let Some(ix) = index {
            texture_list.textures[ix] = Some(atlas_texture);
            texture_list.textures.get_mut(ix).unwrap().as_mut()
        } else {
            texture_list.textures.push(Some(atlas_texture));
            texture_list.textures.last_mut().unwrap().as_mut()
        }
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
        let tile = AtlasTile {
            texture_id: self.id,
            tile_id,
            bounds: Bounds { origin, size },
            padding: 0,
        };
        self.live_atlas_keys += 1;
        Some(tile)
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
            crate::log::error!(
                "DirectXAtlasTexture::upload: source slice is {} bytes but the {}x{} region \
                 requires {} bytes; skipping upload to avoid a driver over-read",
                bytes.len(),
                bounds.size.width.0,
                bounds.size.height.0,
                expected,
            );
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

    fn decrement_ref_count(&mut self) {
        self.live_atlas_keys -= 1;
    }

    fn is_unreferenced(&mut self) -> bool {
        self.live_atlas_keys == 0
    }
}
