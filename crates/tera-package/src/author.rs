use crate::error::{PackageError, Result};
use crate::summary::PACKAGE_MAGIC;

const NAME_FLAGS: u64 = 0x0007_0010_0000_0000;
const PACKAGE_FLAGS: u32 = 0x2288_0009;
const ENGINE_VERSION: u32 = 13249;
const COOKER_VERSION: u32 = 142;
const REFERENCER_NUMBER: i32 = 101241;
const TEXTURE_TAIL_MARKER: i32 = 0x21;

#[derive(Clone, Debug)]
pub struct TextureSpec {
    pub package_name: String,
    pub object_name: String,
    pub width: u32,
    pub height: u32,
    pub format: String,
    pub lod_group: String,
    pub srgb: bool,
    pub never_stream: bool,
    pub source_path: String,
    pub mips: Vec<Vec<u8>>,
}

impl TextureSpec {
    pub fn new(package_name: impl Into<String>, object_name: impl Into<String>) -> Self {
        Self {
            package_name: package_name.into(),
            object_name: object_name.into(),
            width: 0,
            height: 0,
            format: "PF_DXT5".into(),
            lod_group: "TEXTUREGROUP_UI".into(),
            srgb: false,
            never_stream: true,
            source_path: String::new(),
            mips: Vec::new(),
        }
    }

    pub fn mip_tail_base_index(&self) -> i32 {
        let longest = self.width.max(self.height).max(1);
        (31 - longest.leading_zeros()) as i32
    }
}

struct NameTable {
    names: Vec<String>,
}

impl NameTable {
    fn build(entries: &[&str]) -> Self {
        let mut names: Vec<String> = entries.iter().map(|name| name.to_string()).collect();
        names.sort_by(|left, right| {
            left.to_ascii_lowercase()
                .cmp(&right.to_ascii_lowercase())
                .then_with(|| left.cmp(right))
        });
        names.dedup();
        Self { names }
    }

    fn index(&self, name: &str) -> Result<i32> {
        self.names
            .iter()
            .position(|entry| entry == name)
            .map(|position| position as i32)
            .ok_or_else(|| PackageError::UnsupportedPixelFormat(format!("missing name `{name}`")))
    }

    fn serialized_len(&self) -> usize {
        self.names
            .iter()
            .map(|name| 4 + name.len() + 1 + 8)
            .sum::<usize>()
    }

    fn write(&self, out: &mut Vec<u8>) {
        for name in &self.names {
            write_ascii(out, name);
            out.extend_from_slice(&NAME_FLAGS.to_le_bytes());
        }
    }
}

fn write_ascii(out: &mut Vec<u8>, value: &str) {
    out.extend_from_slice(&((value.len() + 1) as i32).to_le_bytes());
    out.extend_from_slice(value.as_bytes());
    out.push(0);
}

fn write_name(out: &mut Vec<u8>, index: i32, number: i32) {
    out.extend_from_slice(&index.to_le_bytes());
    out.extend_from_slice(&number.to_le_bytes());
}

struct Tag<'a> {
    names: &'a NameTable,
    out: &'a mut Vec<u8>,
}

impl Tag<'_> {
    fn header(&mut self, name: &str, kind: &str, size: i32) -> Result<()> {
        write_name(self.out, self.names.index(name)?, 0);
        write_name(self.out, self.names.index(kind)?, 0);
        self.out.extend_from_slice(&size.to_le_bytes());
        self.out.extend_from_slice(&0i32.to_le_bytes());
        Ok(())
    }

    fn integer(&mut self, name: &str, value: i32) -> Result<()> {
        self.header(name, "IntProperty", 4)?;
        self.out.extend_from_slice(&value.to_le_bytes());
        Ok(())
    }

    fn boolean(&mut self, name: &str, value: bool) -> Result<()> {
        write_name(self.out, self.names.index(name)?, 0);
        write_name(self.out, self.names.index("BoolProperty")?, 0);
        self.out.extend_from_slice(&0i32.to_le_bytes());
        self.out.extend_from_slice(&0i32.to_le_bytes());
        self.out.push(u8::from(value));
        Ok(())
    }

    fn enumeration(&mut self, name: &str, enum_name: &str, value: &str) -> Result<()> {
        write_name(self.out, self.names.index(name)?, 0);
        write_name(self.out, self.names.index("ByteProperty")?, 0);
        self.out.extend_from_slice(&8i32.to_le_bytes());
        self.out.extend_from_slice(&0i32.to_le_bytes());
        write_name(self.out, self.names.index(enum_name)?, 0);
        write_name(self.out, self.names.index(value)?, 0);
        Ok(())
    }

    fn text(&mut self, name: &str, value: &str) -> Result<()> {
        let size = if value.is_empty() {
            4
        } else {
            4 + value.len() + 1
        };
        self.header(name, "StrProperty", size as i32)?;
        if value.is_empty() {
            self.out.extend_from_slice(&0i32.to_le_bytes());
        } else {
            write_ascii(self.out, value);
        }
        Ok(())
    }

    fn guid(&mut self, name: &str) -> Result<()> {
        write_name(self.out, self.names.index(name)?, 0);
        write_name(self.out, self.names.index("StructProperty")?, 0);
        self.out.extend_from_slice(&16i32.to_le_bytes());
        self.out.extend_from_slice(&0i32.to_le_bytes());
        write_name(self.out, self.names.index("Guid")?, 0);
        self.out.extend_from_slice(&[0u8; 16]);
        Ok(())
    }

    fn end(&mut self) -> Result<()> {
        write_name(self.out, self.names.index("None")?, 0);
        Ok(())
    }
}

fn write_bulk(out: &mut Vec<u8>, base: usize, payload: &[u8]) {
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(payload.len() as i32).to_le_bytes());
    out.extend_from_slice(&(payload.len() as i32).to_le_bytes());
    let offset = base + out.len() + 4;
    out.extend_from_slice(&(offset as i32).to_le_bytes());
    out.extend_from_slice(payload);
}

fn stable_guid(seed: &str) -> [u8; 16] {
    let mut state = 0xcbf2_9ce4_8422_2325u64;
    let mut guid = [0u8; 16];
    for (index, slot) in guid.iter_mut().enumerate() {
        for byte in seed.bytes().chain(std::iter::once(index as u8)) {
            state = (state ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
        }
        *slot = (state >> 24) as u8;
    }
    guid
}

pub fn build_texture_package(spec: &TextureSpec) -> Result<Vec<u8>> {
    if spec.mips.is_empty() {
        return Err(PackageError::NoPayload(spec.object_name.clone()));
    }
    let names = NameTable::build(&[
        "ArrayProperty",
        "BoolProperty",
        "ByteProperty",
        "Class",
        "Core",
        "Engine",
        "EPixelFormat",
        &spec.package_name,
        "Format",
        "Guid",
        "IntProperty",
        "LightingGuid",
        &spec.object_name,
        "LODGroup",
        "MipTailBaseIdx",
        "NeverStream",
        "None",
        "ObjectReferencer",
        "OriginalSizeX",
        "OriginalSizeY",
        "Package",
        &spec.format,
        "ReferencedObjects",
        "SizeX",
        "SizeY",
        "SourceFilePath",
        "SourceFileTimestamp",
        "SRGB",
        "StrProperty",
        "StructProperty",
        "Texture2D",
        "TextureGroup",
        &spec.lod_group,
    ]);

    let mut summary = Vec::with_capacity(192);
    summary.extend_from_slice(&PACKAGE_MAGIC.to_le_bytes());
    summary.extend_from_slice(&897u16.to_le_bytes());
    summary.extend_from_slice(&17u16.to_le_bytes());
    let header_size_position = summary.len();
    summary.extend_from_slice(&0i32.to_le_bytes());
    write_ascii(&mut summary, "None");
    summary.extend_from_slice(&PACKAGE_FLAGS.to_le_bytes());
    summary.extend_from_slice(&(names.names.len() as i32).to_le_bytes());
    let name_offset_position = summary.len();
    summary.extend_from_slice(&0i32.to_le_bytes());
    summary.extend_from_slice(&3i32.to_le_bytes());
    let export_offset_position = summary.len();
    summary.extend_from_slice(&0i32.to_le_bytes());
    summary.extend_from_slice(&5i32.to_le_bytes());
    let import_offset_position = summary.len();
    summary.extend_from_slice(&0i32.to_le_bytes());
    let depends_offset_position = summary.len();
    summary.extend_from_slice(&0i32.to_le_bytes());
    let guids_offset_position = summary.len();
    summary.extend_from_slice(&0i32.to_le_bytes());
    summary.extend_from_slice(&0i32.to_le_bytes());
    summary.extend_from_slice(&0i32.to_le_bytes());
    summary.extend_from_slice(&0i32.to_le_bytes());
    summary.extend_from_slice(&stable_guid(&spec.package_name));
    summary.extend_from_slice(&1i32.to_le_bytes());
    summary.extend_from_slice(&3i32.to_le_bytes());
    summary.extend_from_slice(&(names.names.len() as i32).to_le_bytes());
    summary.extend_from_slice(&0i32.to_le_bytes());
    summary.extend_from_slice(&ENGINE_VERSION.to_le_bytes());
    summary.extend_from_slice(&COOKER_VERSION.to_le_bytes());
    summary.extend_from_slice(&0u32.to_le_bytes());
    summary.extend_from_slice(&0i32.to_le_bytes());
    let source = u32::from_le_bytes(stable_guid(&spec.object_name)[..4].try_into().unwrap());
    summary.extend_from_slice(&source.to_le_bytes());
    summary.extend_from_slice(&0i32.to_le_bytes());
    summary.extend_from_slice(&0i32.to_le_bytes());

    let name_offset = summary.len();
    let import_offset = name_offset + names.serialized_len();
    let export_offset = import_offset + 5 * 28;
    let depends_offset = export_offset + 3 * 68 + 4;
    let header_size = depends_offset + 3 * 4;

    let referencer_offset = header_size;
    let package_offset = referencer_offset + 48;
    let texture_offset = package_offset + 12;

    let mut referencer = Vec::with_capacity(48);
    referencer.extend_from_slice(&(-1i32).to_le_bytes());
    {
        let mut tag = Tag {
            names: &names,
            out: &mut referencer,
        };
        tag.header("ReferencedObjects", "ArrayProperty", 12)?;
        tag.out.extend_from_slice(&2i32.to_le_bytes());
        tag.out.extend_from_slice(&1i32.to_le_bytes());
        tag.out.extend_from_slice(&3i32.to_le_bytes());
        tag.end()?;
    }

    let mut package_blob = Vec::with_capacity(12);
    package_blob.extend_from_slice(&(-1i32).to_le_bytes());
    write_name(&mut package_blob, names.index("None")?, 0);

    let mut texture = Vec::with_capacity(spec.mips.iter().map(Vec::len).sum::<usize>() + 1024);
    texture.extend_from_slice(&(-1i32).to_le_bytes());
    {
        let mut tag = Tag {
            names: &names,
            out: &mut texture,
        };
        tag.integer("SizeX", spec.width as i32)?;
        tag.integer("SizeY", spec.height as i32)?;
        tag.integer("OriginalSizeX", spec.width as i32)?;
        tag.integer("OriginalSizeY", spec.height as i32)?;
        tag.enumeration("Format", "EPixelFormat", &spec.format)?;
        tag.integer("MipTailBaseIdx", spec.mip_tail_base_index())?;
        tag.boolean("SRGB", spec.srgb)?;
        tag.boolean("NeverStream", spec.never_stream)?;
        tag.enumeration("LODGroup", "TextureGroup", &spec.lod_group)?;
        tag.text("SourceFilePath", &spec.source_path)?;
        tag.text("SourceFileTimestamp", "")?;
        tag.guid("LightingGuid")?;
        tag.end()?;
    }
    write_bulk(&mut texture, texture_offset, &[]);
    write_ascii(&mut texture, &spec.source_path);
    texture.extend_from_slice(&(spec.mips.len() as i32).to_le_bytes());
    let mut level_width = spec.width.max(1);
    let mut level_height = spec.height.max(1);
    for payload in &spec.mips {
        write_bulk(&mut texture, texture_offset, payload);
        texture.extend_from_slice(&(level_width as i32).to_le_bytes());
        texture.extend_from_slice(&(level_height as i32).to_le_bytes());
        level_width = (level_width / 2).max(1);
        level_height = (level_height / 2).max(1);
    }
    texture.extend_from_slice(&[0u8; 16]);
    texture.extend_from_slice(&[0u8; 12]);
    texture.extend_from_slice(&TEXTURE_TAIL_MARKER.to_le_bytes());
    texture.extend_from_slice(&0i32.to_le_bytes());
    texture.extend_from_slice(&(-1i32).to_le_bytes());
    texture.extend_from_slice(&(-1i32).to_le_bytes());
    texture.extend_from_slice(&0i32.to_le_bytes());

    summary[header_size_position..header_size_position + 4]
        .copy_from_slice(&(header_size as i32).to_le_bytes());
    summary[name_offset_position..name_offset_position + 4]
        .copy_from_slice(&(name_offset as i32).to_le_bytes());
    summary[import_offset_position..import_offset_position + 4]
        .copy_from_slice(&(import_offset as i32).to_le_bytes());
    summary[export_offset_position..export_offset_position + 4]
        .copy_from_slice(&(export_offset as i32).to_le_bytes());
    summary[depends_offset_position..depends_offset_position + 4]
        .copy_from_slice(&(depends_offset as i32).to_le_bytes());
    summary[guids_offset_position..guids_offset_position + 4]
        .copy_from_slice(&(header_size as i32).to_le_bytes());

    let mut out = Vec::with_capacity(texture_offset + texture.len());
    out.extend_from_slice(&summary);
    names.write(&mut out);

    let core = names.index("Core")?;
    let engine = names.index("Engine")?;
    let class = names.index("Class")?;
    let package = names.index("Package")?;
    let referencer_class = names.index("ObjectReferencer")?;
    let texture_class = names.index("Texture2D")?;
    let imports: [(i32, i32, i32, i32); 5] = [
        (core, class, -4, package),
        (core, class, -5, referencer_class),
        (core, class, -5, texture_class),
        (core, package, 0, core),
        (core, package, 0, engine),
    ];
    for (class_package, class_name, outer, object) in imports {
        write_name(&mut out, class_package, 0);
        write_name(&mut out, class_name, 0);
        out.extend_from_slice(&outer.to_le_bytes());
        write_name(&mut out, object, 0);
    }

    write_export(
        &mut out,
        -2,
        referencer_class,
        REFERENCER_NUMBER,
        0x0007_0000_0000_0000,
        referencer.len(),
        referencer_offset,
        0,
        0,
        0,
    );
    write_export(
        &mut out,
        -1,
        names.index(&spec.package_name)?,
        0,
        0x0007_0004_0000_0000,
        package_blob.len(),
        package_offset,
        1,
        1,
        PACKAGE_FLAGS,
    );
    write_export(
        &mut out,
        -3,
        names.index(&spec.object_name)?,
        0,
        0x000f_0004_0000_0000,
        texture.len(),
        texture_offset,
        0,
        0,
        0,
    );

    for _ in 0..3 {
        out.extend_from_slice(&0i32.to_le_bytes());
    }
    out.extend_from_slice(&referencer);
    out.extend_from_slice(&package_blob);
    out.extend_from_slice(&texture);
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn write_export(
    out: &mut Vec<u8>,
    class_index: i32,
    name_index: i32,
    name_number: i32,
    object_flags: u64,
    size: usize,
    offset: usize,
    export_flags: u32,
    net_objects: i32,
    package_flags: u32,
) {
    out.extend_from_slice(&class_index.to_le_bytes());
    out.extend_from_slice(&0i32.to_le_bytes());
    out.extend_from_slice(&0i32.to_le_bytes());
    write_name(out, name_index, name_number);
    out.extend_from_slice(&0i32.to_le_bytes());
    out.extend_from_slice(&object_flags.to_le_bytes());
    out.extend_from_slice(&(size as i32).to_le_bytes());
    out.extend_from_slice(&(offset as i32).to_le_bytes());
    out.extend_from_slice(&export_flags.to_le_bytes());
    out.extend_from_slice(&net_objects.to_le_bytes());
    for _ in 0..net_objects {
        out.extend_from_slice(&0i32.to_le_bytes());
    }
    out.extend_from_slice(&[0u8; 16]);
    out.extend_from_slice(&package_flags.to_le_bytes());
}

pub struct NewTexture<'a> {
    pub package: &'a str,
    pub object: &'a str,
    pub format: &'a str,
    pub lod_group: &'a str,
    pub source_path: &'a str,
    pub mip_chain: bool,
}

impl<'a> NewTexture<'a> {
    pub fn new(package: &'a str, object: &'a str) -> Self {
        Self {
            package,
            object,
            format: "PF_DXT5",
            lod_group: "TEXTUREGROUP_UI",
            source_path: "",
            mip_chain: false,
        }
    }
}

fn decode_image(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>)> {
    if bytes.starts_with(b"DDS ") {
        let dds = crate::dds::Dds::parse(bytes)?;
        let block = crate::replace::format_of(crate::dds::unreal_format_for(
            dds.four_cc.as_ref(),
            dds.bits_per_pixel,
        )?);
        let first = dds
            .mips
            .first()
            .ok_or_else(|| PackageError::UnsupportedPixelFormat("empty dds".into()))?;
        let rgba = crate::bc::decode_blocks(
            block,
            first,
            dds.width as usize,
            dds.height as usize,
        )
        .ok_or_else(|| PackageError::UnsupportedPixelFormat("dds decode failed".into()))?;
        return Ok((dds.width, dds.height, rgba));
    }
    let image = crate::png::decode(bytes)?;
    Ok((image.width, image.height, image.rgba))
}

pub fn texture_package(image: &[u8], options: &NewTexture<'_>) -> Result<Vec<u8>> {
    let (width, height, rgba) = decode_image(image)?;
    if !width.is_power_of_two() || !height.is_power_of_two() {
        return Err(PackageError::UnsupportedPixelFormat(format!(
            "dimensions must be powers of two, got {width}x{height}"
        )));
    }
    let mips = match options.format {
        "PF_A8R8G8B8" => {
            let mut out = Vec::with_capacity(rgba.len());
            for pixel in rgba.as_chunks::<4>().0 {
                out.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
            }
            vec![out]
        }
        "PF_DXT1" | "PF_DXT3" | "PF_DXT5" => {
            let block = crate::replace::format_of(options.format);
            let dds = crate::dds::Dds::from_rgba(&rgba, width, height, block);
            if options.mip_chain {
                dds.mips
            } else {
                dds.mips.into_iter().take(1).collect()
            }
        }
        other => {
            return Err(PackageError::UnsupportedPixelFormat(other.to_string()));
        }
    };
    let mut spec = TextureSpec::new(options.package, options.object);
    spec.width = width;
    spec.height = height;
    spec.format = options.format.to_string();
    spec.lod_group = options.lod_group.to_string();
    spec.source_path = options.source_path.to_string();
    spec.mips = mips;
    build_texture_package(&spec)
}
