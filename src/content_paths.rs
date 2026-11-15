//! One local data root; existing content is symlinked on Unix and copied elsewhere.
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static SELECTED_ROOT: OnceLock<PathBuf> = OnceLock::new();

const INPUTS: &[(&str, &str)] = &[
    ("COLLISION_SHAPES", "facts/block_collision_shapes_26_3.bin"),
    ("TERRAIN_FACTS", "facts/terrain-facts.json"),
    (
        "BLOCK_STATE_PREDICATES",
        "facts/block-state-predicates.json",
    ),
    ("BLOCK_USE_FACTS", "facts/block-use-classes.json"),
    ("ITEM_USE_FACTS", "facts/item-use-owners.json"),
    ("INTERACTION_FACTS", "facts/interaction-facts.json"),
    ("ITEM_PROPERTIES", "facts/item-properties.json"),
    ("CREATIVE_TABS", "facts/creative-facts.json"),
    ("PLACEMENT_FACTS", "facts/placement-state-facts.json"),
    ("ITEM_PLACEMENT_FACTS", "facts/item-placement-facts.json"),
    ("ACTION_STATE_FACTS", "facts/action-state-facts.json"),
    ("ENTITY_USE_STATIC", "facts/entity-use-static.json"),
    ("ASSET_INDEX", "assets/asset-index.json"),
    ("ASSET_ROOT", "assets"),
    ("DATA_DIR", "data/vanilla"),
    ("RESOURCE_ROOT", "resources"),
];

pub(crate) fn default_root() -> Result<PathBuf, String> {
    krasnokamen_paths::data_root(None)
}

/// The launch-selected data root, or the platform default outside a launch.
pub(crate) fn selected_root() -> Result<PathBuf, String> {
    SELECTED_ROOT
        .get()
        .cloned()
        .map(Ok)
        .unwrap_or_else(default_root)
}

/// Called only by the CLI before the event loop or any application threads exist.
/// Explicit environment overrides are retained, including for harness launches.
pub(crate) fn apply_defaults(root: &Path) {
    let _ = SELECTED_ROOT.set(root.to_owned());
    for (suffix, relative) in INPUTS {
        let var = format!("KRASNOKAMEN_CLIENT_{suffix}");
        if std::env::var_os(&var).is_none() {
            // SAFETY: startup is single-threaded; no environment readers exist yet.
            unsafe { std::env::set_var(var, root.join(relative)) };
        }
    }
    // Singleplayer keeps worlds and the managed server beside the content.
    if std::env::var_os("KRASNOKAMEN_CLIENT_ROOT").is_none() {
        // SAFETY: same single-threaded startup window as above.
        unsafe { std::env::set_var("KRASNOKAMEN_CLIENT_ROOT", root) };
    }
}

fn require(path: &Path, directory: bool) -> Result<(), String> {
    let valid = if directory {
        path.is_dir()
    } else {
        path.is_file()
    };
    if valid {
        Ok(())
    } else {
        Err(format!(
            "missing content {}: {} (run krasnokamen-launcher once to download it, or krasnokamen-client setup --from-dist DIR --assets DIR)",
            if directory { "directory" } else { "file" },
            path.display()
        ))
    }
}

/// Fail before creating a window instead of leaving startup on a blank frame.
pub(crate) fn validate(presentation: bool) -> Result<(), String> {
    for (suffix, _) in INPUTS {
        if !presentation && matches!(*suffix, "ASSET_ROOT" | "ASSET_INDEX" | "RESOURCE_ROOT") {
            continue;
        }
        let var = format!("KRASNOKAMEN_CLIENT_{suffix}");
        let path = std::env::var_os(&var).ok_or_else(|| format!("missing content path: {var}"))?;
        require(
            Path::new(&path),
            matches!(*suffix, "ASSET_ROOT" | "DATA_DIR" | "RESOURCE_ROOT"),
        )?;
    }
    let data = PathBuf::from(std::env::var_os("KRASNOKAMEN_CLIENT_DATA_DIR").unwrap());
    for file in [
        "blocks.json",
        "block_mining.json",
        "block_light.json",
        "registries.json",
    ] {
        require(&data.join("reports").join(file), false)?;
    }
    if presentation {
        let assets = PathBuf::from(std::env::var_os("KRASNOKAMEN_CLIENT_ASSET_ROOT").unwrap());
        require(&assets.join("objects"), true)?;
    }
    Ok(())
}

#[cfg(unix)]
fn link(source: &Path, destination: &Path) -> Result<(), String> {
    let source = source
        .canonicalize()
        .map_err(|e| format!("{}: {e}", source.display()))?;
    if let Ok(existing) = destination.canonicalize() {
        if existing == source {
            return Ok(());
        }
        return Err(format!(
            "setup destination already exists: {}",
            destination.display()
        ));
    }
    std::os::unix::fs::symlink(&source, destination)
        .map_err(|e| format!("{}: {e}", destination.display()))?;
    Ok(())
}

#[cfg(not(unix))]
fn link(source: &Path, destination: &Path) -> Result<(), String> {
    copy_content(source, destination)
}

// Also compiled in Linux tests so the non-Unix installation path has regression coverage.
#[cfg(any(not(unix), test))]
fn copy_content(source: &Path, destination: &Path) -> Result<(), String> {
    let copy = || -> std::io::Result<()> {
        let source = source.canonicalize()?;
        let destination = destination
            .parent()
            .ok_or_else(|| std::io::Error::other("destination has no parent"))?
            .canonicalize()?
            .join(
                destination
                    .file_name()
                    .ok_or_else(|| std::io::Error::other("destination has no filename"))?,
            );
        if source == destination {
            return Ok(());
        }
        if destination.starts_with(&source) {
            return Err(std::io::Error::other("destination is inside the source"));
        }
        copy_tree(&source, &destination)
    };
    copy().map_err(|e| {
        format!(
            "copy content {} to {}: {e}",
            source.display(),
            destination.display()
        )
    })
}

#[cfg(any(not(unix), test))]
fn copy_tree(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::{fs, io};
    let kind = fs::symlink_metadata(source)?.file_type();
    let existing = match fs::symlink_metadata(destination) {
        Ok(metadata) => Some(metadata),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    if existing.as_ref().is_some_and(|m| m.file_type().is_symlink()) {
        return Err(io::Error::other(format!(
            "refusing destination symlink: {}",
            destination.display()
        )));
    }
    if kind.is_dir() {
        if !existing.as_ref().is_some_and(|m| m.is_dir()) {
            fs::create_dir(destination)?;
        }
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_tree(&entry.path(), &destination.join(entry.file_name()))?;
        }
        return Ok(());
    }
    if !kind.is_file() {
        return Err(io::Error::other(format!(
            "content is not a regular file or directory: {}",
            source.display()
        )));
    }
    if existing.is_some() {
        if existing.is_some_and(|m| m.is_file()) && same_file_bytes(source, destination)? {
            return Ok(());
        }
        return Err(io::Error::other(format!(
            "setup destination already exists with different content: {}",
            destination.display()
        )));
    }
    let mut input = fs::File::open(source)?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    let result = io::copy(&mut input, &mut output).and_then(|_| output.sync_all());
    drop(output);
    if result.is_err() {
        // Only this invocation's newly created file can be incomplete.
        fs::remove_file(destination)?;
    }
    result
}

#[cfg(any(not(unix), test))]
fn same_file_bytes(left: &Path, right: &Path) -> std::io::Result<bool> {
    use std::{fs::File, io::Read};
    let (mut left, mut right) = (File::open(left)?, File::open(right)?);
    if left.metadata()?.len() != right.metadata()?.len() {
        return Ok(false);
    }
    let (mut a, mut b) = ([0; 8192], [0; 8192]);
    loop {
        let size = left.read(&mut a)?;
        if size == 0 {
            return Ok(right.read(&mut b[..1])? == 0);
        }
        right.read_exact(&mut b[..size])?;
        if a[..size] != b[..size] {
            return Ok(false);
        }
    }
}

pub(crate) fn setup(dist: &Path, assets: &Path, root: &Path) -> Result<(), String> {
    // Validate every source before creating the destination. Never overwrite caches.
    for directory in ["facts", "entities", "data", "resources"] {
        require(&dist.join(directory), true)?;
    }
    require(&dist.join("assets/asset-index.json"), false)?;
    require(&assets.join("objects"), true)?;
    std::fs::create_dir_all(root.join("assets")).map_err(|e| format!("{}: {e}", root.display()))?;
    for directory in ["facts", "entities", "data", "resources"] {
        link(&dist.join(directory), &root.join(directory))?;
    }
    // Pin exactly the index paired with the verified dist facts, not an arbitrary
    // newest Prism index. Its referenced object hashes are resolved in Prism.
    link(
        &dist.join("assets/asset-index.json"),
        &root.join("assets/asset-index.json"),
    )?;
    link(&assets.join("objects"), &root.join("assets/objects"))?;
    #[cfg(unix)]
    println!(
        "Content ready at {} (keep the source caches in place)",
        root.display()
    );
    #[cfg(not(unix))]
    println!("Content copied to {}", root.display());
    Ok(())
}

#[cfg(test)]
#[path = "content_paths_tests.rs"]
mod tests;
