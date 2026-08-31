use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};
use memmap2::Mmap;
use std::fs::File;
use rayon::prelude::*;
use std::collections::BTreeMap;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use tera_package::properties::read_export_properties;
use tera_package::{Bundle, Package, Texture2D};

#[derive(Subcommand)]
pub enum GpkCommand {
    Info(TargetArgs),
    List(ListArgs),
    Props(PropsArgs),
    Mips(PropsArgs),
    Gfx(GfxArgs),
    Materials(GfxArgs),
    Script(GfxArgs),
    Extract(ExtractArgs),
    Index(IndexArgs),
    Mesh(MeshArgs),
    NewTexture(NewTextureArgs),
    Repack(RepackArgs),
    ReplaceTexture(ReplaceTextureArgs),
}

#[derive(Args)]
pub struct IndexArgs {
    #[arg(help = "CookedPC directory to scan")]
    pub root: PathBuf,
    #[arg(long, short, help = "Where to write the index (tsv)")]
    pub out: PathBuf,
    #[arg(long, help = "Also record every export path (much bigger)")]
    pub objects: bool,
}

#[derive(Args)]
pub struct MeshArgs {
    pub file: PathBuf,
    #[arg(long, help = "Object path or suffix; omit to list every mesh found")]
    pub object: Option<String>,
    #[arg(long, help = "Write a Wavefront OBJ next to this path")]
    pub obj: Option<PathBuf>,
    #[arg(long, help = "Write a binary glTF (glb) with embedded diffuse texture")]
    pub glb: Option<PathBuf>,
}

#[derive(Args)]
pub struct NewTextureArgs {
    #[arg(help = "PNG or DDS to import")]
    pub image: PathBuf,
    #[arg(long, help = "Package name the client will look up")]
    pub package: String,
    #[arg(long, help = "Object name inside that package")]
    pub object: String,
    #[arg(long, short)]
    pub out: PathBuf,
    #[arg(long, default_value = "PF_DXT5", help = "PF_DXT1, PF_DXT5")]
    pub format: String,
    #[arg(long, default_value = "TEXTUREGROUP_UI")]
    pub lod_group: String,
}

#[derive(Args)]
pub struct RepackArgs {
    pub file: PathBuf,
    #[arg(long, short)]
    pub out: PathBuf,
}

#[derive(Args)]
pub struct ReplaceTextureArgs {
    pub file: PathBuf,
    #[arg(long, help = "Object path or suffix of the Texture2D to replace")]
    pub object: String,
    #[arg(long, help = "DDS file with the new pixels")]
    pub dds: PathBuf,
    #[arg(long, short)]
    pub out: PathBuf,
}

#[derive(Args)]
pub struct TargetArgs {
    pub file: PathBuf,
}

#[derive(Args)]
pub struct ListArgs {
    pub file: PathBuf,
    #[arg(long, help = "Only list this sub-package index")]
    pub package: Option<usize>,
    #[arg(long, help = "Only list exports whose class matches")]
    pub class: Option<String>,
    #[arg(long, short, help = "Show outer, super, archetype and flags")]
    pub verbose: bool,
}

#[derive(Args)]
pub struct PropsArgs {
    pub file: PathBuf,
    pub object: String,
    #[arg(long, help = "Expand struct arrays into their own properties")]
    pub deep: bool,
}

#[derive(Args)]
pub struct ExtractArgs {
    pub file: PathBuf,
    #[arg(long, short)]
    pub out: PathBuf,
    #[arg(long, help = "Write raw serialized object blobs")]
    pub raw: bool,
    #[arg(long, help = "Decode Texture2D objects to DDS")]
    pub textures: bool,
    #[arg(long, help = "Decode SoundNodeWave objects to Ogg Vorbis")]
    pub sounds: bool,
    #[arg(long, help = "Decode Texture2D objects to PNG instead of DDS")]
    pub png: bool,
    #[arg(long, help = "Only extract objects whose path contains this text")]
    pub filter: Option<String>,
}

fn map(path: &PathBuf) -> Result<Mmap> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    Ok(unsafe { Mmap::map(&file)? })
}

pub fn run(command: GpkCommand) -> Result<()> {
    match command {
        GpkCommand::Info(args) => info(&args),
        GpkCommand::List(args) => list(&args),
        GpkCommand::Props(args) => props(&args),
        GpkCommand::Mips(args) => mips(&args),
        GpkCommand::Gfx(args) => gfx(&args),
        GpkCommand::Materials(args) => list_materials(&args),
        GpkCommand::Script(args) => script(&args),
        GpkCommand::Extract(args) => extract(&args),
        GpkCommand::Index(args) => index(&args),
        GpkCommand::Mesh(args) => mesh(&args),
        GpkCommand::NewTexture(args) => new_texture(&args),
        GpkCommand::Repack(args) => repack(&args),
        GpkCommand::ReplaceTexture(args) => replace_texture(&args),
    }
}

fn index(args: &IndexArgs) -> Result<()> {
    let mut files = Vec::new();
    collect_packages(&args.root, &mut files)?;
    files.sort();
    println!("scanning {} package files", files.len());
    let rows: Vec<String> = files
        .par_iter()
        .flat_map(|path| {
            let mut rows = Vec::new();
            let Ok(file) = File::open(path) else {
                return rows;
            };
            let Ok(data) = (unsafe { Mmap::map(&file) }) else {
                return rows;
            };
            let relative = path
                .strip_prefix(&args.root)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/");
            let stem = path
                .file_stem()
                .map(|value| value.to_string_lossy().to_string())
                .unwrap_or_default();
            let packages: Vec<_> = Bundle::tables_only(&data).collect();
            let single = packages.len() == 1;
            for package in packages {
                let Ok(mut package) = package else { break };
                if single {
                    package.name_hint = Some(stem.clone());
                }
                rows.push(format!(
                    "{}\t{}\t{}\t{}\t{}",
                    package.package_name(),
                    relative,
                    package.base,
                    package.span,
                    package.exports.len()
                ));
                if args.objects {
                    for export_index in 0..package.exports.len() {
                        rows.push(format!(
                            "\t{}\t{}\t{}",
                            package.export_path(export_index),
                            package.export_class(&package.exports[export_index]),
                            relative
                        ));
                    }
                }
            }
            rows
        })
        .collect();
    let mut out = BufWriter::new(File::create(&args.out)?);
    writeln!(out, "package\tfile\toffset\tspan\texports")?;
    for row in &rows {
        writeln!(out, "{row}")?;
    }
    out.flush()?;
    println!("wrote {} rows to {}", rows.len(), args.out.display());
    Ok(())
}

fn collect_packages(root: &PathBuf, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_packages(&path, out)?;
        } else {
            let extension = path
                .extension()
                .map(|value| value.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            if extension == "gpk" || extension == "upk" || extension == "umap" {
                out.push(path);
            }
        }
    }
    Ok(())
}

fn mesh(args: &MeshArgs) -> Result<()> {
    let data = map(&args.file)?;
    let needle = args.object.as_ref().map(|value| value.to_ascii_lowercase());
    let mut found = 0usize;
    for package in Bundle::new(&data) {
        let package = package?;
        for (export_index, export) in package.exports.iter().enumerate() {
            let class = package.export_class(export);
            if class != "StaticMesh" && class != "SkeletalMesh" {
                continue;
            }
            let path = package.export_path(export_index);
            if let Some(needle) = &needle {
                if !path.to_ascii_lowercase().contains(needle) {
                    continue;
                }
            }
            let parsed = if class == "SkeletalMesh" {
                tera_package::parse_skeletal_mesh(&package, export)
            } else {
                tera_package::parse_static_mesh(&package, export)
            };
            match parsed {
                Some(mesh) => {
                    let (low, high) = mesh.bounds();
                    let skin = match &mesh.skin {
                        Some(skin) => format!("  skin: {} bones, {} weighted", skin.bones.len(), skin.joints.len()),
                        None => String::new(),
                    };
                    let secs = format!("  sections: {} (mats {:?})", mesh.sections.len(),
                        mesh.sections.iter().map(|x| x.material).collect::<Vec<_>>());
                    println!(
                        "{path} [{class}] {} verts {} tris  bounds {:.0},{:.0},{:.0} .. {:.0},{:.0},{:.0}{skin}{secs}",
                        mesh.vertices.len(),
                        mesh.triangle_count(),
                        low[0], low[1], low[2], high[0], high[1], high[2]
                    );
                    if let Some(target) = &args.obj {
                        let name = path.rsplit('.').next().unwrap_or("mesh");
                        std::fs::write(target, mesh.to_obj(name))?;
                        println!("wrote {}", target.display());
                        return Ok(());
                    }
                    if let Some(target) = &args.glb {
                        let name = path.rsplit('.').next().unwrap_or("mesh");
                        let cooked = args.file.parent().unwrap_or_else(|| std::path::Path::new("."));
                        let materials = tera_package::mesh_material_inputs(&package, &mesh, cooked);
                        let glb = if materials.iter().any(|m| m.diffuse.is_some() || m.normal.is_some()) {
                            let diff = materials.iter().filter(|m| m.diffuse.is_some()).count();
                            let norm = materials.iter().filter(|m| m.normal.is_some()).count();
                            println!("{} materials ({diff} diffuse, {norm} normal)", materials.len());
                            tera_package::write_glb_multi(&mesh, name, &materials)
                        } else {
                            let texture = tera_package::mesh_diffuse_rgba(&package, name, cooked)
                                .and_then(|(w, h, rgba)| {
                                    tera_package::png::encode(&rgba, w, h).ok().map(|png| (w, h, png))
                                });
                            match &texture {
                                Some((w, h, _)) => println!("diffuse texture {w}x{h} embedded (by name)"),
                                None => println!("no texture found (geometry only)"),
                            }
                            tera_package::write_glb(
                                &mesh,
                                name,
                                texture.as_ref().map(|(w, h, png)| (*w, *h, png.as_slice())),
                            )
                        };
                        std::fs::write(target, glb)?;
                        println!("wrote {}", target.display());
                        return Ok(());
                    }
                }
                None => println!("{path} [{class}] no mesh recovered"),
            }
            found += 1;
        }
    }
    if found == 0 {
        bail!("no StaticMesh or SkeletalMesh found");
    }
    Ok(())
}

fn new_texture(args: &NewTextureArgs) -> Result<()> {
    let bytes = std::fs::read(&args.image)?;
    let (width, height, rgba) = if bytes.starts_with(b"DDS ") {
        let dds = tera_package::Dds::parse(&bytes)?;
        let block = match dds.four_cc {
            Some(code) if &code == b"DXT1" => tera_package::BlockFormat::Bc1,
            Some(code) if &code == b"DXT5" => tera_package::BlockFormat::Bc3,
            _ => bail!("unsupported dds format"),
        };
        let rgba = tera_package::decode_blocks(
            block,
            dds.mips.first().context("empty dds")?,
            dds.width as usize,
            dds.height as usize,
        )
        .context("dds decode failed")?;
        (dds.width, dds.height, rgba)
    } else {
        let image = tera_package::png::decode(&bytes)?;
        (image.width, image.height, image.rgba)
    };
    if !width.is_power_of_two() || !height.is_power_of_two() {
        bail!("dimensions must be powers of two, got {width}x{height}");
    }
    let payload = match args.format.as_str() {
        "PF_DXT1" => tera_package::encode_blocks(
            tera_package::BlockFormat::Bc1,
            &rgba,
            width as usize,
            height as usize,
        ),
        "PF_DXT5" => tera_package::encode_blocks(
            tera_package::BlockFormat::Bc3,
            &rgba,
            width as usize,
            height as usize,
        ),
        "PF_A8R8G8B8" => {
            let mut out = Vec::with_capacity(rgba.len());
            for pixel in rgba.as_chunks::<4>().0 {
                out.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
            }
            out
        }
        other => bail!("unsupported format {other}"),
    };
    let mut spec = tera_package::TextureSpec::new(&args.package, &args.object);
    spec.width = width;
    spec.height = height;
    spec.format = args.format.clone();
    spec.lod_group = args.lod_group.clone();
    spec.source_path = args.image.to_string_lossy().to_string();
    spec.mips = vec![payload];
    let package = tera_package::build_texture_package(&spec)?;
    std::fs::write(&args.out, &package)?;
    println!(
        "wrote {} ({} bytes) — package {} object {} {}x{} {}",
        args.out.display(),
        package.len(),
        args.package,
        args.object,
        width,
        height,
        args.format
    );
    Ok(())
}

fn repack(args: &RepackArgs) -> Result<()> {
    let data = map(&args.file)?;
    let mut out = Vec::new();
    let mut count = 0usize;
    for package in Bundle::new(&data) {
        let package = package?;
        out.extend_from_slice(&tera_package::rebuild(&package, &BTreeMap::new())?);
        count += 1;
    }
    std::fs::write(&args.out, &out)?;
    println!(
        "repacked {count} package(s), {} bytes -> {} bytes ({})",
        data.len(),
        out.len(),
        args.out.display()
    );
    Ok(())
}

fn replace_texture(args: &ReplaceTextureArgs) -> Result<()> {
    let data = map(&args.file)?;
    let dds = tera_package::Dds::parse(&std::fs::read(&args.dds)?)?;
    let needle = args.object.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut replaced = 0usize;
    for package in Bundle::new(&data) {
        let package = package?;
        let mut overrides: BTreeMap<usize, Vec<u8>> = BTreeMap::new();
        for (export_index, export) in package.exports.iter().enumerate() {
            if package.export_class(export) != "Texture2D" {
                continue;
            }
            let path = package.export_path(export_index);
            if !path.to_ascii_lowercase().contains(&needle) {
                continue;
            }
            let texture = Texture2D::parse(&package, export)?;
            let blob = package.export_data(export)?;
            let (patched, mips) = texture.replace_mips(blob, &dds)?;
            println!(
                "{path}: replaced {mips} mip(s), {}x{} {}",
                dds.width,
                dds.height,
                dds.format_name()
            );
            overrides.insert(export_index, patched);
            replaced += 1;
        }
        out.extend_from_slice(&tera_package::rebuild(&package, &overrides)?);
    }
    if replaced == 0 {
        bail!("no Texture2D matching `{}`", args.object);
    }
    std::fs::write(&args.out, &out)?;
    println!("wrote {} ({} bytes)", args.out.display(), out.len());
    Ok(())
}

fn info(args: &TargetArgs) -> Result<()> {
    let data = map(&args.file)?;
    println!("{} ({} bytes)", args.file.display(), data.len());
    for (index, package) in Bundle::new(&data).enumerate() {
        let package = package?;
        println!(
            "[{index:>4}] @{:<12} span {:<10} ue3 {}.{} pkg {:<40} names {:<6} imports {:<5} exports {:<5} comp {:#04x} chunks {}",
            package.base,
            package.span,
            package.summary.version,
            package.summary.licensee,
            package.package_name(),
            package.names.len(),
            package.imports.len(),
            package.exports.len(),
            package.summary.compression_flags,
            package.summary.compressed_chunks.len()
        );
        println!(
            "        header {} names@{} imports@{} exports@{} depends@{} guids@{} thumbs@{} source {:#x} folder {}",
            package.summary.total_header_size,
            package.summary.name_offset,
            package.summary.import_offset,
            package.summary.export_offset,
            package.summary.depends_offset,
            package.summary.import_export_guids_offset,
            package.summary.thumbnail_table_offset,
            package.summary.package_source,
            package.summary.folder_name
        );
        for chunk in &package.summary.compressed_chunks {
            println!(
                "        chunk uncompressed@{} size {} -> compressed@{} size {}",
                chunk.uncompressed_offset,
                chunk.uncompressed_size,
                chunk.compressed_offset,
                chunk.compressed_size
            );
        }
    }
    Ok(())
}

fn list(args: &ListArgs) -> Result<()> {
    let data = map(&args.file)?;
    for (index, package) in Bundle::new(&data).enumerate() {
        let package = package?;
        if let Some(only) = args.package {
            if only != index {
                continue;
            }
        }
        for (export_index, export) in package.exports.iter().enumerate() {
            let class = package.export_class(export);
            if let Some(filter) = &args.class {
                if !class.eq_ignore_ascii_case(filter) {
                    continue;
                }
            }
            if args.verbose {
                println!(
                    "{index:>4} {export_index:>5} {class:<28} {:<56} outer {:>5} super {:>5} arch {:>5} flags {:#018x} {:>10} bytes @{}",
                    package.export_path(export_index),
                    export.outer_index,
                    export.super_index,
                    export.archetype_index,
                    export.object_flags,
                    export.serial_size,
                    export.serial_offset
                );
            } else {
                println!(
                    "{index:>4} {export_index:>5} {class:<28} {:<64} {:>10} bytes @{}",
                    package.export_path(export_index),
                    export.serial_size,
                    export.serial_offset
                );
            }
        }
    }
    Ok(())
}

#[derive(Args)]
pub struct GfxArgs {
    pub file: PathBuf,
    #[arg(long, help = "List compiled functions and their bytecode instead of source")]
    pub functions: bool,
    #[arg(long, help = "Only movies whose path contains this text")]
    pub filter: Option<String>,
    #[arg(long, short, help = "Write each movie here instead of listing them")]
    pub out: Option<PathBuf>,
}

fn script(args: &GfxArgs) -> Result<()> {
    let data = map(&args.file)?;
    if args.functions {
        return script_functions(args, &data);
    }
    let sources = tera_package::sources(&data);
    let wanted: Vec<&tera_package::Source> = sources
        .iter()
        .filter(|source| {
            args.filter
                .as_ref()
                .map(|needle| {
                    source
                        .owner
                        .to_ascii_lowercase()
                        .contains(&needle.to_ascii_lowercase())
                })
                .unwrap_or(true)
        })
        .collect();
    if wanted.is_empty() {
        bail!("no UnrealScript source in {}", args.file.display());
    }
    match &args.out {
        None => {
            let total: usize = wanted.iter().map(|source| source.text.len()).sum();
            for source in &wanted {
                println!("{:<48} {:>8} bytes", source.owner, source.text.len());
            }
            println!("{} classes, {total} bytes of source", wanted.len());
        }
        Some(out) => {
            std::fs::create_dir_all(out)?;
            for source in &wanted {
                let target = out.join(format!("{}.uc", source.owner));
                std::fs::write(&target, &source.text)?;
            }
            println!("wrote {} files to {}", wanted.len(), out.display());
        }
    }
    Ok(())
}

fn script_functions(args: &GfxArgs, data: &[u8]) -> Result<()> {
    let functions = tera_package::functions(data);
    let wanted: Vec<&tera_package::Function> = functions
        .iter()
        .filter(|function| {
            args.filter
                .as_ref()
                .map(|needle| {
                    function
                        .path
                        .to_ascii_lowercase()
                        .contains(&needle.to_ascii_lowercase())
                })
                .unwrap_or(true)
        })
        .collect();
    if wanted.is_empty() {
        bail!("no compiled functions in {}", args.file.display());
    }
    match &args.out {
        None => {
            for function in &wanted {
                println!(
                    "{:<60} line {:<6} {:>6} bytes of bytecode  flags {:#010x}",
                    function.path,
                    function.line,
                    function.bytecode.len(),
                    function.flags
                );
            }
            println!(
                "{} functions, {} bytes of bytecode",
                wanted.len(),
                wanted
                    .iter()
                    .map(|function| function.bytecode.len())
                    .sum::<usize>()
            );
        }
        Some(out) => {
            std::fs::create_dir_all(out)?;
            for function in &wanted {
                let safe = function.path.replace(['/', '\\', ':'], "_");
                std::fs::write(out.join(format!("{safe}.bytecode")), &function.bytecode)?;
            }
            println!("wrote {} files to {}", wanted.len(), out.display());
        }
    }
    Ok(())
}

fn list_materials(args: &GfxArgs) -> Result<()> {
    let data = map(&args.file)?;
    let mut shown = 0usize;
    for material in tera_package::materials(&data) {
        if let Some(needle) = &args.filter {
            if !material
                .path
                .to_ascii_lowercase()
                .contains(&needle.to_ascii_lowercase())
            {
                continue;
            }
        }
        println!("{}", material.path);
        for parameter in &material.parameters {
            println!(
                "  {:<28} {:<8} {}",
                parameter.name,
                parameter.kind.label(),
                parameter.value
            );
        }
        shown += 1;
    }
    if shown == 0 {
        bail!("no material instances in {}", args.file.display());
    }
    Ok(())
}

fn gfx(args: &GfxArgs) -> Result<()> {
    let data = map(&args.file)?;
    let movies = tera_package::movies(&data);
    let wanted: Vec<&tera_package::Movie> = movies
        .iter()
        .filter(|movie| {
            args.filter
                .as_ref()
                .map(|needle| {
                    movie
                        .path
                        .to_ascii_lowercase()
                        .contains(&needle.to_ascii_lowercase())
                })
                .unwrap_or(true)
        })
        .collect();
    if wanted.is_empty() {
        bail!("no GFxMovieInfo in {}", args.file.display());
    }
    match &args.out {
        None => {
            for movie in wanted {
                println!(
                    "{}  {} bytes  {}",
                    movie.path,
                    movie.data.len(),
                    movie.kind()
                );
                if !movie.source_file.is_empty() {
                    println!("    from {}", movie.source_file);
                }
            }
        }
        Some(out) => {
            std::fs::create_dir_all(out)?;
            for movie in wanted {
                let safe = movie.path.replace(['/', '\\', ':'], "_");
                let target = out.join(format!("{safe}.{}", movie.extension()));
                std::fs::write(&target, &movie.data)?;
                println!("{} ({} bytes)", target.display(), movie.data.len());
            }
        }
    }
    Ok(())
}

fn mips(args: &PropsArgs) -> Result<()> {
    let data = map(&args.file)?;
    let needle = args.object.to_ascii_lowercase();
    for package in Bundle::new(&data) {
        let Ok(package) = package else { break };
        for (export_index, export) in package.exports.iter().enumerate() {
            if package.export_class(export) != "Texture2D" {
                continue;
            }
            let path = package.export_path(export_index);
            if !path.to_ascii_lowercase().contains(&needle) {
                continue;
            }
            let texture = Texture2D::parse(&package, export)?;
            println!("{path} : {} {}x{}", texture.format, texture.width, texture.height);
            for (index, mip) in texture.mips.iter().enumerate() {
                println!(
                    "  {index:>2} {:>5}x{:<5} flags {:#06x} size {:>9} offset {:>10} {}",
                    mip.width,
                    mip.height,
                    mip.data.flags,
                    mip.data.size_on_disk,
                    mip.data.offset_in_file,
                    if mip.data.is_inline() { "inline" } else { "elsewhere" }
                );
            }
        }
    }
    Ok(())
}

fn props(args: &PropsArgs) -> Result<()> {
    let data = map(&args.file)?;
    let mut found = false;
    for package in Bundle::new(&data) {
        let package = package?;
        for (export_index, export) in package.exports.iter().enumerate() {
            let path = package.export_path(export_index);
            if !path.to_ascii_lowercase().contains(&args.object.to_ascii_lowercase()) {
                continue;
            }
            found = true;
            println!("{} : {}", path, package.export_class(export));
            let blob = package.export_data(export)?;
            let (properties, consumed) = read_export_properties(&package, blob)?;
            for property in &properties {
                println!(
                    "  {:<32} {:<18} {}",
                    property.name,
                    property.type_name,
                    property.value.describe()
                );
                if !args.deep {
                    continue;
                }
                if let tera_package::PropertyValue::Array { count, element_size, raw } =
                    &property.value
                {
                    for index in 0..*count as usize {
                        let start = index * element_size;
                        let Some(element) = raw.get(start..start + element_size) else {
                            break;
                        };
                        let Ok((fields, _)) =
                            tera_package::read_properties(&package, element)
                        else {
                            continue;
                        };
                        if fields.is_empty() {
                            continue;
                        }
                        println!("    [{index}]");
                        for field in &fields {
                            println!(
                                "      {:<28} {:<16} {}",
                                field.name,
                                field.type_name,
                                field.value.describe()
                            );
                        }
                    }
                }
            }
            println!(
                "  -- {} properties, {} of {} bytes consumed",
                properties.len(),
                consumed,
                blob.len()
            );
        }
    }
    if !found {
        bail!("no object matching `{}`", args.object);
    }
    Ok(())
}

fn extract(args: &ExtractArgs) -> Result<()> {
    let data = map(&args.file)?;
    std::fs::create_dir_all(&args.out)?;
    let mut caches = tera_package::texture::Caches::at(
        args.file.parent().unwrap_or(std::path::Path::new(".")),
    );
    let mut written = 0usize;
    for package in Bundle::new(&data) {
        let Ok(package) = package else { break };
        for (export_index, export) in package.exports.iter().enumerate() {
            let path = package.export_path(export_index);
            if let Some(filter) = &args.filter {
                if !path.to_ascii_lowercase().contains(&filter.to_ascii_lowercase()) {
                    continue;
                }
            }
            let class = package.export_class(export);
            let safe = path.replace(['/', '\\', ':'], "_");
            if (args.textures || args.png) && class == "Texture2D" {
                match extract_texture(&package, export, &args.out, &safe, args.png, &mut caches) {
                    Ok(true) => written += 1,
                    Ok(false) => {}
                    Err(error) => eprintln!("{path}: {error}"),
                }
            }
            if args.sounds && class == "SoundNodeWave" {
                match tera_package::SoundNodeWave::parse(&package, export)
                    .and_then(|sound| sound.payload().map(|data| data.to_vec()))
                {
                    Ok(payload) => {
                        std::fs::write(args.out.join(format!("{safe}.ogg")), payload)?;
                        written += 1;
                    }
                    Err(error) => eprintln!("{path}: {error}"),
                }
            }
            if args.raw {
                let target = args.out.join(format!("{safe}.{class}.bin"));
                std::fs::write(&target, package.export_data(export)?)?;
                written += 1;
            }
        }
    }
    println!("wrote {written} files to {}", args.out.display());
    Ok(())
}

fn extract_texture(
    package: &Package,
    export: &tera_package::Export,
    out: &std::path::Path,
    safe: &str,
    as_png: bool,
    caches: &mut tera_package::texture::Caches,
) -> Result<bool> {
    let texture = Texture2D::parse(package, export)?;
    if texture.best_mip(true).is_none() {
        return Ok(false);
    }
    if as_png {
        std::fs::write(out.join(format!("{safe}.png")), texture.to_png_with(Some(caches))?)?;
    } else {
        std::fs::write(
            out.join(format!("{safe}.dds")),
            tera_package::texture::export_dds_with(&texture, Some(caches))?,
        )?;
    }
    Ok(true)
}
