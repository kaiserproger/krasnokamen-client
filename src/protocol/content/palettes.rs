//! Atlas-generated palette sprites. Resource lookup and PNG decoding remain owned by Content.
use super::*;

/// `key` and `palette` hold the physical texture ids of the palette images (see `palette_texture`).
#[derive(Clone)]
pub(super) enum PaletteSprite {
    Generated { base: Arc<str>, key: Arc<str>, palette: Arc<str> },
    Physical(Arc<str>),
}

/// 26.3 `PalettedPermutations.loadPaletteEntryFromImage` reads `palette_key` and every
/// permutation through `Palette.ID_CONVERTER = FileToIdConverter("textures/palettes", ".png")`;
/// only the permuted `textures` use `SpriteSource.TEXTURE_ID_CONVERTER` ("textures").
/// Palette `ns:path` is therefore the physical texture `ns:palettes/path`.
fn palette_texture(id: &str) -> Result<String> {
    let (namespace, path) = split_id(id)?;
    Ok(format!("{namespace}:palettes/{path}"))
}

pub(super) fn index(source: &ResourceSource) -> Result<BTreeMap<String, PaletteSprite>> {
    let mut sprites = BTreeMap::new();
    let textures = source.resource_names("textures", "png")?;
    for atlas in source.resource_names("atlases", "json")? {
        let (namespace, path) = split_id(&atlas)?;
        let relative = format!("assets/{namespace}/atlases/{path}.json");
        for bytes in source.stack(&relative)? {
            if bytes.len() > 1 << 20 {
                return Err(format!("atlas JSON bound {relative}"));
            }
            let json: Value = serde_json::from_slice(&bytes)
                .map_err(|e| format!("atlas {relative}: {e}"))?;
            let entries = json["sources"].as_array()
                .ok_or_else(|| format!("atlas sources missing {relative}"))?;
            for entry in entries {
                match entry["type"].as_str() {
                    Some("minecraft:paletted_permutations" | "paletted_permutations") => {
                        let key: Arc<str> = palette_texture(entry["palette_key"].as_str()
                            .ok_or_else(|| format!("atlas palette_key missing {relative}"))?)?.into();
                        let permutations = entry["permutations"].as_object()
                            .ok_or_else(|| format!("atlas permutations missing {relative}"))?;
                        let bases = entry["textures"].as_array()
                            .ok_or_else(|| format!("atlas textures missing {relative}"))?;
                        let separator = entry["separator"].as_str().unwrap_or("_");
                        for base in bases {
                            let base: Arc<str> = normalize_id(base.as_str()
                                .ok_or_else(|| format!("atlas texture invalid {relative}"))?)?.into();
                            for (suffix, palette) in permutations {
                                let name = normalize_id(&format!("{base}{separator}{suffix}"))?;
                                let palette = palette_texture(palette.as_str()
                                    .ok_or_else(|| format!("atlas palette invalid {relative}"))?)?;
                                sprites.insert(name, PaletteSprite::Generated {
                                    base: Arc::clone(&base), key: Arc::clone(&key), palette: palette.into(),
                                });
                                if sprites.len() > source.limits.max_resources {
                                    return Err(format!("atlas generated sprite count bound {relative}"));
                                }
                            }
                        }
                    }
                    Some("minecraft:single" | "single") => {
                        let resource = entry["resource"].as_str()
                            .ok_or_else(|| format!("atlas single resource missing {relative}"))?;
                        let id = normalize_id(entry["sprite"].as_str().unwrap_or(resource))?;
                        if let std::collections::btree_map::Entry::Occupied(mut entry) = sprites.entry(id) {
                            entry.insert(PaletteSprite::Physical(normalize_id(resource)?.into()));
                        }
                    }
                    Some("minecraft:directory" | "directory") => {
                        let directory = entry["source"].as_str()
                            .ok_or_else(|| format!("atlas directory source missing {relative}"))?;
                        let prefix = entry["prefix"].as_str()
                            .ok_or_else(|| format!("atlas directory prefix missing {relative}"))?;
                        let directory = normalize_id(directory)?;
                        let (ns, dir) = split_id(&directory)?;
                        let path_prefix = format!("{ns}:{dir}/");
                        for texture in textures.range(path_prefix.clone()..) {
                            let Some(rest) = texture.strip_prefix(&path_prefix) else { break };
                            let id = normalize_id(&format!("{ns}:{prefix}{rest}"))?;
                            if sprites.contains_key(&id) {
                                sprites.insert(id, PaletteSprite::Physical(texture.as_str().into()));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(sprites)
}

pub(super) fn decode(source: &ResourceSource, name: &str, sprite: &PaletteSprite) -> Result<Image> {
    let (base, key, palette) = match sprite {
        PaletteSprite::Generated { base, key, palette } => (base, key, palette),
        PaletteSprite::Physical(resource) => return decode_physical_texture(source, resource),
    };
    // Never resolve palette inputs as sprite IDs: the vanilla source reads PNG resources directly.
    let mut image = decode_physical_texture(source, base)?;
    let key_image = decode_physical_texture(source, key)?;
    let palette_image = decode_physical_texture(source, palette)?;
    if key_image.rgba.len() != palette_image.rgba.len() {
        return Err(format!("palette size mismatch for {name}: {key} and {palette}"));
    }
    // Account for the lookup table as well as all concurrently live decoded images.
    let pixels = key_image.rgba.len() / 4;
    let peak = image.rgba.len().checked_add(key_image.rgba.len())
        .and_then(|v| v.checked_add(palette_image.rgba.len()))
        .and_then(|v| pixels.checked_mul(32).and_then(|map| v.checked_add(map)))
        .ok_or_else(|| format!("palette preprocessing overflow {name}"))?;
    if peak > source.limits.max_decoded_bytes {
        return Err(format!("palette preprocessing CPU byte budget exceeded {name}"));
    }
    let mut mapping = BTreeMap::new();
    for (key, value) in key_image.rgba.chunks_exact(4).zip(palette_image.rgba.chunks_exact(4)) {
        if key[3] != 0 {
            mapping.insert([key[0], key[1], key[2]], [value[0], value[1], value[2], value[3]]);
        }
    }
    for pixel in image.rgba.chunks_exact_mut(4) {
        if pixel[3] == 0 { continue; }
        if let Some(color) = mapping.get(&[pixel[0], pixel[1], pixel[2]]) {
            pixel[0..3].copy_from_slice(&color[..3]);
            pixel[3] = (u16::from(pixel[3]) * u16::from(color[3]) / 255) as u8;
        }
    }
    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Root(PathBuf);
    impl Drop for Root {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }
    static NEXT: AtomicU64 = AtomicU64::new(0);

    fn write(root: &Path, path: &str, bytes: &[u8]) {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn png(root: &Path, path: &str, width: u32, height: u32, pixels: &[u8]) {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.write_header().unwrap().write_image_data(pixels).unwrap();
        }
        write(root, path, &bytes);
    }

    #[test]
    fn generated_item_uses_palette_for_geometry_and_atlas_with_pack_overrides() {
        let root = Root(std::env::temp_dir().join(format!(
            "solaris-palette-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)
        )));
        let base = root.0.join("base");
        let pack = root.0.join("pack");
        write(&pack, "pack.mcmeta", br#"{"pack":{"pack_format":84}}"#);
        write(&base, "assets/test/atlases/items.json", br#"{"sources":[
            {"type":"minecraft:paletted_permutations","textures":["test:trim"],
             "palette_key":"test:key","permutations":{"gold":"test:gold"}}
        ]}"#);
        write(&base, "assets/test/models/item/chestplate.json", br#"{
            "parent":"minecraft:builtin/generated","textures":{"layer0":"test:trim_gold"}
        }"#);
        png(&base, "assets/test/textures/trim.png", 2, 2, &[
            10, 20, 30, 255, 10, 20, 30, 128,
            10, 20, 30, 0, 7, 8, 9, 255,
        ]);
        png(&base, "assets/test/textures/palettes/key.png", 1, 1, &[10, 20, 30, 255]);
        png(&base, "assets/test/textures/palettes/gold.png", 1, 1, &[1, 2, 3, 255]);
        png(&pack, "assets/test/textures/palettes/gold.png", 1, 1, &[200, 150, 40, 128]);
        // The 26.1.2 palette locations are ordinary 26.3 sprites, never palette inputs.
        png(&base, "assets/test/textures/key.png", 1, 1, &[7, 8, 9, 255]);
        png(&pack, "assets/test/textures/gold.png", 1, 1, &[9, 9, 9, 255]);
        // A physical PNG is not an atlas sprite source and cannot override a permutation.
        png(&pack, "assets/test/textures/trim_gold.png", 1, 1, &[255, 0, 0, 255]);
        let source = ResourceSource::new(vec![base, pack], None, ResourceLimits::default()).unwrap();
        let image = decode_texture(&source, "test:trim_gold").unwrap();
        assert_eq!(image.rgba, [
            200, 150, 40, 128, 200, 150, 40, 64,
            10, 20, 30, 0, 7, 8, 9, 255,
        ]);
        let sizes = BTreeSet::from(["test:gold".to_owned(), "test:trim_gold".to_owned()]);
        assert_eq!(atlas_packing_order(&source, &sizes).unwrap(), ["test:trim_gold", "test:gold"]);
        let raw = Baker::new(&source).model("test:item/chestplate").unwrap();
        let names = BTreeSet::from(["test:trim_gold".to_owned()]);
        let atlas = Atlas::build(&source, &names).unwrap();
        let baked = raw.finish(&atlas.textures, &atlas.materials).unwrap();
        assert!(baked.quads.len() > 2, "generated layer must have visible edge geometry");
        let region = atlas.textures["test:trim_gold"];
        assert_eq!((region.width, region.height), (2, 2));
        let x = (region.uv[0] * atlas.side as f32) as usize;
        let y = (region.uv[1] * atlas.side as f32) as usize;
        let offset = ((region.layer as usize * atlas.side as usize + y) * atlas.side as usize + x) * 4;
        assert_eq!(&atlas.rgba[offset..offset + 4], &[200, 150, 40, 128]);
        assert!(baked.quads.iter().all(|quad| quad.texture_layer == region.layer));
        write(&root.0.join("pack"), "assets/test/atlases/items.json", br#"{"sources":[
            {"type":"minecraft:single","resource":"test:override","sprite":"test:trim_gold"}
        ]}"#);
        png(&root.0.join("pack"), "assets/test/textures/override.png", 1, 1, &[4, 5, 6, 255]);
        let source = ResourceSource::new(
            vec![root.0.join("base"), root.0.join("pack")], None, ResourceLimits::default()
        ).unwrap();
        assert_eq!(decode_texture(&source, "test:trim_gold").unwrap().rgba, [4, 5, 6, 255]);
    }

    /// Pinned 26.3 `items.json` shape: `minecraft:trim_base` and `minecraft:trim/<material>`
    /// exist only under `textures/palettes/`; the 26.1.2 `textures/` location must not satisfy them.
    #[test]
    fn vanilla_26_3_trim_palettes_resolve_under_textures_palettes() {
        let root = Root(std::env::temp_dir().join(format!(
            "solaris-palette-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)
        )));
        let base = root.0.join("base");
        write(&base, "assets/minecraft/atlases/items.json", br#"{"sources":[
            {"type":"minecraft:paletted_permutations","palette_key":"minecraft:trim_base",
             "permutations":{"gold":"minecraft:trim/gold"},
             "textures":["minecraft:trims/items/helmet_trim"]}
        ]}"#);
        png(&base, "assets/minecraft/textures/trims/items/helmet_trim.png", 2, 1, &[
            10, 20, 30, 255, 40, 50, 60, 128,
        ]);
        png(&base, "assets/minecraft/textures/palettes/trim_base.png", 2, 1, &[
            10, 20, 30, 255, 40, 50, 60, 255,
        ]);
        png(&base, "assets/minecraft/textures/palettes/trim/gold.png", 2, 1, &[
            200, 150, 40, 255, 100, 75, 20, 255,
        ]);
        let source = ResourceSource::new(vec![base.clone()], None, ResourceLimits::default()).unwrap();
        assert_eq!(
            decode_texture(&source, "minecraft:trims/items/helmet_trim_gold").unwrap().rgba,
            [200, 150, 40, 255, 100, 75, 20, 128],
        );
        fs::rename(
            base.join("assets/minecraft/textures/palettes/trim_base.png"),
            base.join("assets/minecraft/textures/trim_base.png"),
        ).unwrap();
        let source = ResourceSource::new(vec![base], None, ResourceLimits::default()).unwrap();
        let error = decode_texture(&source, "minecraft:trims/items/helmet_trim_gold").err().unwrap();
        assert_eq!(error, "missing mandatory resource assets/minecraft/textures/palettes/trim_base.png");
    }
}
