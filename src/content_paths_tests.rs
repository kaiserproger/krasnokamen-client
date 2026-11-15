use super::*;
use std::fs;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("krasnokamen-setup-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn content(&self) -> (PathBuf, PathBuf, PathBuf) {
        let dist = self.0.join("исходные данные");
        let assets = self.0.join("asset cache");
        let root = self.0.join("client data");
        for directory in [
            "facts",
            "entities",
            "data/vanilla",
            "resources/assets",
            "assets",
        ] {
            fs::create_dir_all(dist.join(directory)).unwrap();
        }
        fs::create_dir_all(assets.join("objects/ab")).unwrap();
        fs::write(dist.join("facts/terrain-facts.json"), b"facts").unwrap();
        fs::write(dist.join("data/vanilla/registries.json"), b"registry").unwrap();
        fs::write(dist.join("resources/assets/texture.bin"), b"texture").unwrap();
        fs::write(dist.join("assets/asset-index.json"), b"paired-index").unwrap();
        fs::write(assets.join("objects/ab/abcdef"), b"audio").unwrap();
        (dist, assets, root)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn assert_content(root: &Path) {
    for (path, expected) in [
        ("facts/terrain-facts.json", "facts"),
        ("data/vanilla/registries.json", "registry"),
        ("resources/assets/texture.bin", "texture"),
        ("assets/asset-index.json", "paired-index"),
        ("assets/objects/ab/abcdef", "audio"),
    ] {
        assert_eq!(fs::read(root.join(path)).unwrap(), expected.as_bytes());
    }
    assert!(root.join("entities").is_dir());
}

#[test]
fn non_unix_copy_installs_nested_content_and_paired_index_without_source_dependency() {
    let fixture = Fixture::new();
    let (dist, assets, root) = fixture.content();
    fs::create_dir_all(root.join("assets")).unwrap();
    // The production non-Unix link delegates to this same copy implementation.
    for _ in 0..2 {
        for directory in ["facts", "entities", "data", "resources"] {
            copy_content(&dist.join(directory), &root.join(directory)).unwrap();
        }
        copy_content(
            &dist.join("assets/asset-index.json"),
            &root.join("assets/asset-index.json"),
        )
        .unwrap();
        copy_content(&assets.join("objects"), &root.join("assets/objects")).unwrap();
        assert_content(&root);
    }
    fs::remove_dir_all(dist).unwrap();
    fs::remove_dir_all(assets).unwrap();
    assert_content(&root);
}

#[test]
fn non_unix_copy_refuses_conflicts_without_overwriting_either_cache() {
    let fixture = Fixture::new();
    let source = fixture.0.join("source");
    let destination = fixture.0.join("destination");
    let bytes = vec![7; 20_000];
    fs::write(&source, &bytes).unwrap();
    copy_content(&source, &destination).unwrap();
    copy_content(&source, &destination).unwrap();
    let mut different = bytes.clone();
    different[19_000] = 8;
    fs::write(&destination, &different).unwrap();
    let error = copy_content(&source, &destination).unwrap_err();
    assert!(error.contains("different content"), "{error}");
    assert!(error.contains("destination"), "{error}");
    assert_eq!(fs::read(&source).unwrap(), bytes);
    assert_eq!(fs::read(&destination).unwrap(), different);
    fs::write(&destination, b"shorter").unwrap();
    assert!(copy_content(&source, &destination).is_err());
    assert_eq!(fs::read(&destination).unwrap(), b"shorter");
}

#[test]
fn non_unix_copy_refuses_recursive_destination_and_missing_sources() {
    let fixture = Fixture::new();
    let source = fixture.0.join("source");
    fs::create_dir(&source).unwrap();
    let nested = source.join("nested");
    let error = copy_content(&source, &nested).unwrap_err();
    assert!(error.contains("inside the source"), "{error}");
    assert!(!nested.exists());
    copy_content(&source, &source).unwrap();
    let absent = fixture.0.join("absent");
    assert!(copy_content(&absent, &nested).is_err());
    assert!(!nested.exists());
}

#[cfg(unix)]
#[test]
fn non_unix_copy_does_not_follow_nested_source_or_destination_symlinks() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let source = fixture.0.join("source");
    let destination = fixture.0.join("destination");
    let other = fixture.0.join("other");
    fs::create_dir(&source).unwrap();
    fs::create_dir(&other).unwrap();
    symlink(&other, &destination).unwrap();
    assert!(copy_content(&source, &destination).is_err());
    fs::remove_file(&destination).unwrap();
    symlink(&source, source.join("cycle")).unwrap();
    assert!(copy_content(&source, &destination).is_err());
    assert!(!destination.join("cycle").exists());
}

#[test]
fn setup_uses_platform_installation_and_keeps_the_paired_index() {
    let fixture = Fixture::new();
    let (dist, assets, root) = fixture.content();
    setup(&dist, &assets, &root).unwrap();
    setup(&dist, &assets, &root).unwrap();
    assert_content(&root);
    #[cfg(unix)]
    {
        assert!(
            fs::symlink_metadata(root.join("facts"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            root.join("facts").canonicalize().unwrap(),
            dist.join("facts").canonicalize().unwrap()
        );
    }
    #[cfg(not(unix))]
    {
        fs::remove_dir_all(dist).unwrap();
        fs::remove_dir_all(assets).unwrap();
        assert_content(&root);
    }
}

#[test]
fn setup_validates_inputs_before_creating_destination() {
    let fixture = Fixture::new();
    let (dist, assets, root) = fixture.content();
    fs::remove_file(dist.join("assets/asset-index.json")).unwrap();
    let error = setup(&dist, &assets, &root).unwrap_err();
    assert!(error.contains("asset-index.json"), "{error}");
    assert!(!root.exists());
}
