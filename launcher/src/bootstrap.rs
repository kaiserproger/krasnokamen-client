//! First-run content bootstrap. Every stage is idempotent and skips quickly
//! when its result is already installed, so Play after a clean install does
//! not touch the network.
use crate::{
    mojang,
    net::{self, Check, Job},
    paths::{self, GAME_VERSION, Layout},
    pins,
    progress::{Cancelled, Reporter},
};
use anyhow::{Context, Result, anyhow, bail};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

const STAGES: usize = 5;

/// What is installed, so unchanged stages can be skipped.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Installed {
    pub game_version: String,
    pub client_sha1: String,
    /// Bundle name -> sha256 of the installed archive.
    pub bundles: std::collections::BTreeMap<String, String>,
    pub resources_for: String,
}

impl Installed {
    pub fn load(layout: &Layout) -> Self {
        fs::read_to_string(layout.installed_file())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }
    fn save(&self, layout: &Layout) -> Result<()> {
        let tmp = layout.installed_file().with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        fs::rename(&tmp, layout.installed_file())?;
        Ok(())
    }
}

/// Play can start immediately: everything required is on disk and recorded,
/// including a resource pack whose marker names the jar this installation
/// extracted. `ui.rs` starts the client without `run` when this is true, so a
/// stale, copied or half-written pack must make it false and send Play through
/// the install stage that repairs it.
pub fn is_ready(layout: &Layout) -> bool {
    let installed = Installed::load(layout);
    installed.game_version == GAME_VERSION
        && !installed.bundles.is_empty()
        && !installed.resources_for.is_empty()
        && paths::missing(&layout.root).is_empty()
        && pack_ready(layout, &installed)
}

/// The pack's marker, compared with what the local client jar produces. The jar
/// must be present (stage 3 downloads it before stage 5): without it nothing can
/// say the pack is current, so readiness is false and `run` decides.
fn pack_ready(layout: &Layout, installed: &Installed) -> bool {
    let Ok(manifest) = resources_manifest(&layout.client_jar(), &installed.client_sha1) else {
        return false;
    };
    resources_current(layout, installed, &manifest, false, false)
}

pub struct Options {
    pub manifest_url: String,
}

/// One bundle kind and the exact archive this launcher release accepts for it.
struct Pin {
    kind: &'static str,
    name: &'static str,
    sha256: &'static str,
}

/// Client data tables (`facts/`, `data/vanilla/reports/`), installed by every Play.
const FACTS: Pin = Pin {
    kind: "facts",
    name: pins::FACTS_NAME,
    sha256: pins::FACTS_SHA256,
};

/// Singleplayer only: the complete server content cache that
/// `krasnokamen content import` derives, the local server's `vanilla_data_dir`.
const CONTENT: Pin = Pin {
    kind: "content",
    name: pins::CONTENT_NAME,
    sha256: pins::CONTENT_SHA256,
};

#[derive(Debug)]
struct Bundle {
    url: String,
    size: u64,
}

fn fetch_bundle(client: &Client, manifest_url: &str, pin: &Pin) -> Result<Bundle> {
    let bytes = net::fetch_bytes(client, manifest_url, None).with_context(|| {
        format!("cannot reach the Krasnokamen content manifest at {manifest_url}")
    })?;
    parse_bundle(&bytes, pin)
}

/// Pure manifest handling, separate from the network fetch for testing.
fn parse_bundle(bytes: &[u8], pin: &Pin) -> Result<Bundle> {
    let manifest: Value = serde_json::from_slice(bytes).context("parsing content manifest")?;
    if manifest["game_version"] != GAME_VERSION {
        bail!(
            "content manifest is for {}, launcher needs {GAME_VERSION}",
            manifest["game_version"]
        );
    }
    // The manifest travels over plain HTTP: only the pinned archive is accepted.
    let entry = manifest["bundles"]
        .as_array()
        .context("manifest.bundles")?
        .iter()
        .find(|entry| entry["kind"] == pin.kind && entry["name"] == pin.name)
        .ok_or_else(|| {
            anyhow!(
                "content manifest does not list the {} bundle {} this launcher is pinned to; update the launcher",
                pin.kind,
                pin.name
            )
        })?;
    let sha256 = entry["sha256"]
        .as_str()
        .context("manifest bundle missing sha256")?;
    if !sha256.eq_ignore_ascii_case(pin.sha256) {
        bail!(
            "refusing content bundle {}: manifest sha256 {sha256} does not match the hash pinned in this launcher ({})",
            pin.name,
            pin.sha256
        );
    }
    Ok(Bundle {
        url: entry["url"]
            .as_str()
            .context("manifest bundle missing url")?
            .to_owned(),
        size: entry["size"].as_u64().context("manifest bundle size")?,
    })
}

fn is_installed(installed: &Installed, pin: &Pin) -> bool {
    installed
        .bundles
        .get(pin.name)
        .is_some_and(|sha256| sha256 == pin.sha256)
}

/// Download the pinned archive (hash-checked, resumable) and unpack it into a
/// fresh staging directory on the data root's filesystem.
fn download_and_unpack(
    client: &Client,
    layout: &Layout,
    manifest_url: &str,
    pin: &Pin,
    rep: &Reporter,
) -> Result<PathBuf> {
    let bundle = fetch_bundle(client, manifest_url, pin)?;
    let archive = layout.cache_dir().join(format!("{}.tar.zst", pin.name));
    rep.set_total(bundle.size);
    net::download(
        client,
        &Job {
            url: bundle.url,
            dest: archive.clone(),
            check: Check::Sha256(pin.sha256.to_owned()),
            size: bundle.size,
        },
        rep,
    )?;
    let staging = layout.join(".staging-bundle");
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging)?;
    let decoder = zstd::stream::read::Decoder::new(fs::File::open(&archive)?)?;
    tar::Archive::new(decoder)
        .unpack(&staging)
        .with_context(|| format!("unpacking {}", pin.name))?;
    Ok(staging)
}

fn replace_dir(from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }
    if to.exists() {
        fs::remove_dir_all(to)?;
    }
    fs::rename(from, to).with_context(|| format!("installing {}", to.display()))
}

fn install_facts(layout: &Layout, staging: &Path, rep: &Reporter) -> Result<()> {
    for entry in fs::read_dir(staging)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        match name.as_str() {
            "facts" => replace_dir(&entry.path(), &layout.join("facts"))?,
            "data" => {
                for child in fs::read_dir(entry.path())? {
                    let child = child?;
                    replace_dir(&child.path(), &layout.join("data").join(child.file_name()))?;
                }
            }
            "bundle.json" => {}
            other => rep.log(format!("ignoring unexpected bundle entry {other}")),
        }
    }
    Ok(())
}

/// Marker written inside an extracted resource pack. A pack is trusted only when
/// this file matches the client jar that would produce it: `installed.json`'s
/// `resources_for` is a claim, and a pre-populated or half-written pack carries a
/// stale one, which left 26.3-only assets (`models/block/template_sign_*`,
/// `textures/entity/cushion/*`, `textures/entity/sulfur_cube/*`) missing after an
/// upgrade from an older client jar.
const RESOURCES_MARKER: &str = ".resources.json";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct ResourcesManifest {
    game_version: String,
    client_sha1: String,
    entries: u64,
    paths_sha256: String,
}

/// What the jar's `assets/` tree must produce: the file count and a digest of the
/// sorted paths, so a pack built from any other jar never matches.
pub(crate) fn resources_manifest(jar: &Path, client_sha1: &str) -> Result<ResourcesManifest> {
    use sha2::{Digest, Sha256};
    let mut archive = zip::ZipArchive::new(fs::File::open(jar)?)?;
    let mut paths = Vec::new();
    for index in 0..archive.len() {
        let entry = archive.by_index(index)?;
        if entry.is_file() && entry.name().starts_with("assets/") {
            paths.push(entry.name().to_owned());
        }
    }
    paths.sort();
    let mut hasher = Sha256::new();
    for path in &paths {
        hasher.update(path.as_bytes());
        hasher.update(b"\n");
    }
    Ok(ResourcesManifest {
        game_version: GAME_VERSION.to_owned(),
        client_sha1: client_sha1.to_owned(),
        entries: paths.len() as u64,
        paths_sha256: format!("{:x}", hasher.finalize()),
    })
}

/// Whether stage 5 may skip. The pack's own marker must name the jar that would
/// build it: `Installed::resources_for` is a manifest claim that survives a copied
/// or half-written tree, so it takes no part in the decision.
fn resources_current(
    layout: &Layout,
    installed: &Installed,
    expected: &ResourcesManifest,
    client_changed: bool,
    force: bool,
) -> bool {
    let _ = installed;
    !force && !client_changed && pack_is_current(layout, expected)
}

fn pack_is_current(layout: &Layout, expected: &ResourcesManifest) -> bool {
    fs::read_to_string(layout.join("resources").join(RESOURCES_MARKER))
        .ok()
        .and_then(|text| serde_json::from_str::<ResourcesManifest>(&text).ok())
        .is_some_and(|marker| marker == *expected)
}

pub(crate) fn extract_resources(
    layout: &Layout,
    rep: &Reporter,
    expected: &ResourcesManifest,
) -> Result<()> {
    let staging = layout.join("resources.staging");
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    let mut archive = zip::ZipArchive::new(fs::File::open(layout.client_jar())?)?;
    let mut total = 0;
    for index in 0..archive.len() {
        let entry = archive.by_index(index)?;
        if entry.is_file() && entry.name().starts_with("assets/") {
            total += entry.size();
        }
    }
    rep.set_total(total);
    let mut buffer = Vec::new();
    for index in 0..archive.len() {
        rep.check_cancel()
            .map_err(|_: Cancelled| net::UserCancelled)?;
        let mut entry = archive.by_index(index)?;
        if !entry.is_file() || !entry.name().starts_with("assets/") {
            continue;
        }
        let Some(relative) = entry.enclosed_name() else {
            bail!(
                "client jar entry {} escapes the extraction root",
                entry.name()
            );
        };
        let target = staging.join(relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        buffer.clear();
        entry.read_to_end(&mut buffer)?;
        fs::File::create(&target)?.write_all(&buffer)?;
        rep.add_done(buffer.len() as u64);
    }
    fs::write(
        staging.join(RESOURCES_MARKER),
        serde_json::to_vec_pretty(expected)?,
    )?;
    swap_pack(
        &staging,
        &layout.join("resources"),
        &layout.join("resources.old"),
    )
}

/// Swap a finished staging pack in, keeping the previous pack until the new one is
/// in place: an additive extraction would keep entries the jar dropped, and
/// deleting the old pack before the rename leaves the installation with none when
/// that rename fails.
fn swap_pack(staging: &Path, root: &Path, backup: &Path) -> Result<()> {
    if backup.exists() {
        fs::remove_dir_all(backup)?;
    }
    if root.exists() {
        fs::rename(root, backup)?;
    }
    if let Err(error) = fs::rename(staging, root) {
        if backup.exists() {
            let _ = fs::rename(backup, root);
        }
        return Err(error.into());
    }
    if backup.exists() {
        fs::remove_dir_all(backup)?;
    }
    Ok(())
}

/// Run every stage. `force` re-verifies/re-derives even when recorded as done.
pub fn run(layout: &Layout, options: &Options, force: bool, rep: &Reporter) -> Result<()> {
    fs::create_dir_all(&layout.root)?;
    let client = net::client()?;
    let mut installed = Installed::load(layout);

    rep.stage(1, STAGES, "Checking Mojang version metadata");
    let release = mojang::resolve(&client, layout, rep)?;

    rep.stage(2, STAGES, "Downloading Krasnokamen game data");
    let present = is_installed(&installed, &FACTS)
        && layout.join("facts").is_dir()
        && layout.join("data/vanilla/reports").is_dir();
    if present && !force {
        rep.log(format!("{} already installed", FACTS.name));
    } else {
        let staging = download_and_unpack(&client, layout, &options.manifest_url, &FACTS, rep)?;
        install_facts(layout, &staging, rep)?;
        let _ = fs::remove_dir_all(&staging);
        installed
            .bundles
            .insert(FACTS.name.into(), FACTS.sha256.into());
        installed.game_version = GAME_VERSION.into();
        installed.save(layout)?;
    }

    rep.stage(3, STAGES, "Downloading Minecraft client");
    rep.set_total(release.client.size);
    net::download(&client, &release.client.job(layout.client_jar()), rep)?;
    let client_changed = installed.client_sha1 != release.client.sha1;
    installed.client_sha1 = release.client.sha1.clone();

    rep.stage(4, STAGES, "Downloading Minecraft game assets");
    let index_job = release.asset_index.job(layout.asset_index());
    rep.set_total(index_job.size);
    net::download(&client, &index_job, rep)?;
    let jobs = mojang::asset_jobs(layout, &fs::read(layout.asset_index())?)?;
    rep.log(format!("{} asset objects", jobs.len()));
    net::download_all(&client, jobs, 12, rep)?;

    rep.stage(5, STAGES, "Extracting textures, models and fonts");
    let manifest = resources_manifest(&layout.client_jar(), &release.client.sha1)?;
    if resources_current(layout, &installed, &manifest, client_changed, force) {
        rep.log("resources already extracted");
    } else {
        extract_resources(layout, rep, &manifest)?;
        installed.resources_for = release.client.sha1.clone();
        installed.save(layout)?;
    }

    installed.game_version = GAME_VERSION.into();
    installed.save(layout)?;
    let missing = paths::missing(&layout.root);
    if !missing.is_empty() {
        bail!("install incomplete, missing: {}", missing.join(", "));
    }
    rep.log("Game content ready");
    Ok(())
}

/// Singleplayer only (`krasnokamen-launcher --singleplayer-runtime`, started by the client before
/// a local world): the local server's content cache, installed from the pinned content bundle
/// as its `vanilla_data_dir`. Multiplayer never needs it. Returns without touching the
/// network when the pinned bundle is already installed, so the client can call this on
/// every local start and a launcher with a new pin replaces a stale cache.
pub fn ensure_singleplayer_runtime(
    layout: &Layout,
    options: &Options,
    rep: &Reporter,
) -> Result<()> {
    let mut installed = Installed::load(layout);
    let target = layout.server_content();
    if is_installed(&installed, &CONTENT) && target.join("version.json").is_file() {
        return Ok(());
    }
    rep.stage(1, 1, "Downloading singleplayer server data");
    let client = net::client()?;
    let staging = download_and_unpack(&client, layout, &options.manifest_url, &CONTENT, rep)?;
    replace_dir(&staging.join("content"), &target)?;
    let _ = fs::remove_dir_all(&staging);
    // Left over from the JDK-based content import this bundle replaces.
    let _ = fs::remove_dir_all(layout.join("jdk"));
    let _ = fs::remove_file(layout.version_dir().join("server.jar"));
    installed
        .bundles
        .insert(CONTENT.name.into(), CONTENT.sha256.into());
    installed.save(layout)?;
    rep.log("Singleplayer server data ready");
    Ok(())
}

/// Write a jar with the given `assets/**` members (test fixture).
#[cfg(test)]
pub(crate) fn write_test_jar(path: &Path, entries: &[(&str, &[u8])]) {
    let mut writer = zip::ZipWriter::new(fs::File::create(path).unwrap());
    for (name, body) in entries {
        writer
            .start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(body).unwrap();
    }
    writer.finish().unwrap();
}

/// A layout that satisfies every non-pack readiness requirement: every
/// `paths::REQUIRED` entry, the client jar, and an `installed.json` for this
/// release (test fixture).
#[cfg(test)]
pub(crate) fn test_install(root: &Path, client_sha1: &str, jar: &[(&str, &[u8])]) -> Layout {
    let layout = Layout::new(root.to_owned());
    for (relative, directory) in paths::REQUIRED {
        let target = layout.join(relative);
        if *directory {
            fs::create_dir_all(&target).unwrap();
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&target, b"").unwrap();
        }
    }
    fs::create_dir_all(layout.join("mojang/versions").join(GAME_VERSION)).unwrap();
    write_test_jar(&layout.client_jar(), jar);
    Installed {
        game_version: GAME_VERSION.to_owned(),
        client_sha1: client_sha1.to_owned(),
        bundles: std::collections::BTreeMap::from([(
            "solaris-facts-26.3-r1".to_owned(),
            "x".to_owned(),
        )]),
        resources_for: client_sha1.to_owned(),
    }
    .save(&layout)
    .unwrap();
    layout
}

/// An extractable pack for a test jar: `assets/**` only, with a marker.
#[cfg(test)]
pub(crate) fn test_repair(layout: &Layout, client_sha1: &str) {
    let manifest = resources_manifest(&layout.client_jar(), client_sha1).unwrap();
    extract_resources(layout, &Reporter::new(None, true), &manifest).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_jar(path: &Path, entries: &[(&str, &[u8])]) {
        write_test_jar(path, entries);
    }

    #[test]
    fn a_stale_resource_pack_is_replaced_instead_of_merged() {
        let root = std::env::temp_dir().join(format!("krasnokamen-pack-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let layout = Layout::new(root.clone());
        fs::create_dir_all(layout.join("mojang/versions").join(GAME_VERSION)).unwrap();
        write_jar(
            &layout.client_jar(),
            &[
                (
                    "assets/minecraft/models/block/template_sign_rot_0.json",
                    b"{}",
                ),
                (
                    "assets/minecraft/textures/entity/cushion/white_cushion.png",
                    b"x",
                ),
            ],
        );
        // A pack from an older client jar: a file the 26.3 jar no longer has, no marker.
        fs::create_dir_all(layout.join("resources/assets/minecraft/models/block")).unwrap();
        fs::write(
            layout.join("resources/assets/minecraft/models/block/oak_sign.json"),
            b"{}",
        )
        .unwrap();
        let expected = resources_manifest(&layout.client_jar(), &"a".repeat(40)).unwrap();
        assert_eq!(expected.entries, 2);
        assert!(!pack_is_current(&layout, &expected));
        // A marker for another jar never stands in for the content it names.
        let mut stale = expected.clone();
        stale.client_sha1 = "b".repeat(40);
        fs::write(
            layout.join("resources").join(RESOURCES_MARKER),
            serde_json::to_vec(&stale).unwrap(),
        )
        .unwrap();
        assert!(!pack_is_current(&layout, &expected));

        extract_resources(&layout, &Reporter::new(None, true), &expected).unwrap();

        assert!(
            layout
                .join("resources/assets/minecraft/models/block/template_sign_rot_0.json")
                .is_file(),
            "the 26.3-only asset must arrive"
        );
        assert!(
            !layout
                .join("resources/assets/minecraft/models/block/oak_sign.json")
                .exists(),
            "an entry the jar dropped must not survive the rebuild"
        );
        assert!(pack_is_current(&layout, &expected));
        assert!(!layout.join("resources.staging").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn an_installation_whose_pack_lost_its_marker_is_not_ready() {
        let root = std::env::temp_dir().join(format!("krasnokamen-ready-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let sha1 = "a".repeat(40);
        let layout = test_install(
            &root,
            &sha1,
            &[("assets/minecraft/models/block/oak_sign_rot_0.json", b"{}")],
        );
        // The 26.1.2-era pack a player still has: the directory exists, no marker.
        fs::create_dir_all(layout.join("resources/assets/minecraft/models/block")).unwrap();
        fs::write(
            layout.join("resources/assets/minecraft/models/block/oak_sign.json"),
            b"{}",
        )
        .unwrap();
        assert!(
            paths::missing(&layout.root).is_empty(),
            "the fixture must satisfy every non-pack requirement"
        );
        assert!(
            !is_ready(&layout),
            "a markerless pack must not report the installation ready; Play then runs the repair stage"
        );
        test_repair(&layout, &sha1);
        assert!(
            is_ready(&layout),
            "the repaired pack makes Play start the client directly"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_failed_pack_swap_keeps_the_previous_pack() {
        let root = std::env::temp_dir().join(format!("krasnokamen-swap-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let pack = root.join("resources");
        fs::create_dir_all(pack.join("assets/minecraft")).unwrap();
        fs::write(pack.join("assets/minecraft/oak_sign.json"), b"{}").unwrap();
        // A staging directory that vanished (or a failed rename): the swap must put
        // the previous pack back instead of leaving the installation without one.
        let error = swap_pack(
            &root.join("resources.staging"),
            &pack,
            &root.join("resources.old"),
        );
        assert!(error.is_err());
        assert!(
            pack.join("assets/minecraft/oak_sign.json").is_file(),
            "the previous pack is restored"
        );
        assert!(!root.join("resources.old").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_manifest_claim_does_not_make_a_stale_pack_current() {
        let root = std::env::temp_dir().join(format!("krasnokamen-skip-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let layout = Layout::new(root.clone());
        fs::create_dir_all(layout.join("mojang/versions").join(GAME_VERSION)).unwrap();
        write_jar(
            &layout.client_jar(),
            &[("assets/minecraft/models/block/oak_sign_rot_0.json", b"{}")],
        );
        // 26.1.2-era pack: the directory exists and the manifest already names the
        // current client sha1, which is exactly what the old skip trusted.
        fs::create_dir_all(layout.join("resources/assets/minecraft/models/block")).unwrap();
        fs::write(
            layout.join("resources/assets/minecraft/models/block/oak_sign.json"),
            b"{}",
        )
        .unwrap();
        let expected = resources_manifest(&layout.client_jar(), &"a".repeat(40)).unwrap();
        let installed = Installed {
            resources_for: expected.client_sha1.clone(),
            ..Default::default()
        };
        assert!(
            layout.join("resources/assets/minecraft").is_dir(),
            "the pre-fix condition also required this directory"
        );
        assert!(
            !resources_current(&layout, &installed, &expected, false, false),
            "a stale pack must be rebuilt even when the manifest claims the current jar"
        );
        extract_resources(&layout, &Reporter::new(None, true), &expected).unwrap();
        assert!(resources_current(
            &layout, &installed, &expected, false, false
        ));
        assert!(!resources_current(
            &layout, &installed, &expected, false, true
        ));
        let _ = fs::remove_dir_all(&root);
    }

    fn manifest(kind: &str, name: &str, sha: &str) -> Vec<u8> {
        format!(
            r#"{{"game_version":"{GAME_VERSION}","bundles":[{{"name":"{name}","kind":"{kind}","url":"http://h/x","sha256":"{sha}","size":5}}]}}"#
        )
        .into_bytes()
    }

    #[test]
    fn pinned_bundles_are_accepted() {
        for pin in [&FACTS, &CONTENT] {
            let bundle = parse_bundle(&manifest(pin.kind, pin.name, pin.sha256), pin).unwrap();
            assert_eq!((bundle.url.as_str(), bundle.size), ("http://h/x", 5));
        }
    }

    #[test]
    fn manifest_hash_that_differs_from_the_pin_is_refused() {
        let wrong = "1".repeat(64);
        for pin in [&FACTS, &CONTENT] {
            let error = parse_bundle(&manifest(pin.kind, pin.name, &wrong), pin).unwrap_err();
            assert!(error.to_string().contains("pinned"), "{error}");
        }
    }

    #[test]
    fn unpinned_bundle_name_is_never_installed() {
        for pin in [&FACTS, &CONTENT] {
            let name = format!("{}-r99", pin.name);
            assert!(parse_bundle(&manifest(pin.kind, &name, pin.sha256), pin).is_err());
        }
    }

    #[test]
    fn pinned_name_under_another_kind_is_refused() {
        assert!(parse_bundle(&manifest("facts", CONTENT.name, CONTENT.sha256), &CONTENT).is_err());
    }

    #[test]
    fn wrong_game_version_is_refused() {
        let text = br#"{"game_version":"1.0","bundles":[]}"#;
        assert!(parse_bundle(text, &FACTS).is_err());
    }

    #[test]
    fn pins_are_well_formed() {
        for pin in [&FACTS, &CONTENT] {
            assert_eq!(pin.sha256.len(), 64);
            assert!(pin.sha256.bytes().all(|b| b.is_ascii_hexdigit()));
            assert!(
                pin.name.starts_with(&format!("solaris-{}-", pin.kind)),
                "{} has the wrong bundle prefix",
                pin.name
            );
        }
        assert!(
            FACTS.name.contains(GAME_VERSION),
            "{} is not for {GAME_VERSION}",
            FACTS.name
        );
    }
}
