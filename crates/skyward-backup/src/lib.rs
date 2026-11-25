//! Backups: what one holds, as the manifest in its folder records it, and
//! the files each of its parts is kept in.

pub mod archive;
pub mod store;

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use skyward_files::Ownership;
use std::io::Write;
use std::path::{Component, Path};

/// The file in a backup's folder that records what the backup holds. It is
/// written last, so a folder with one holds a whole backup.
pub const MANIFEST: &str = "backup.toml";

/// Where backups go, from the top of the place they are kept in.
pub const FOLDER: &str = "Brina/backups";

/// The layout of a backup this recovery writes and reads.
pub const FORMAT: u32 = 1;

/// What a backup can hold, in the order the backup page lists it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Part {
    /// The slot's boot images, and the partitions that check what the
    /// phone starts.
    Boot,
    /// The slot's partitions in super.
    System,
    /// The data partition's files, all but internal storage.
    Data,
    /// The baseband's settings with the phone's own identity, and the
    /// sensors' calibration.
    Efs,
}

impl Part {
    pub const ALL: [Part; 4] = [Part::Boot, Part::System, Part::Data, Part::Efs];
}

/// What a backup holds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u32,
    /// The phone it was made on, by its codename.
    pub device: String,
    /// When it was made, in Unix seconds.
    pub made: i64,
    /// The slot it was made from, by its suffix.
    pub slot: String,
    /// The system it was made from, as its version reads.
    pub system: Option<String>,
    /// With the data partition's files, the reference of the key the
    /// partition encrypts with: one made afresh, as a factory reset makes
    /// it, has another.
    pub data_key: Option<String>,
    pub items: Vec<Item>,
}

/// One part of what a backup holds: a partition's image, or the data
/// partition's files.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub part: Part,
    /// The partition's name without its slot's suffix, or `data`.
    pub name: String,
    #[serde(flatten)]
    pub stored: store::Stored,
}

/// Whether `name` is a partition's name, as the phone's tables write one.
fn partition_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
}

/// Whether `name` is a file in the backup's own folder, and not a path
/// through or out of it.
fn file_name(name: &str) -> bool {
    matches!(Path::new(name).components().collect::<Vec<_>>().as_slice(), [Component::Normal(_)])
}

impl Manifest {
    /// The manifest in the backup's `folder`.
    pub fn read(folder: &Path) -> Result<Manifest> {
        let path = folder.join(MANIFEST);
        let text = std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let manifest: Manifest = toml::from_str(&text).with_context(|| format!("read {}", path.display()))?;
        ensure!(manifest.format == FORMAT, "{} is in layout {}, which this recovery does not read", path.display(), manifest.format);
        for item in &manifest.items {
            ensure!(partition_name(&item.name), "{} names the partition {:?} in a way this recovery does not read", path.display(), item.name);
            if item.part == Part::Data {
                ensure!(item.name == "data", "{} names the data partition {:?}", path.display(), item.name);
            }
            for file in &item.stored.files {
                ensure!(file_name(file), "{} keeps {:?} outside its folder", path.display(), file);
            }
        }
        Ok(manifest)
    }

    /// Write it into the backup's `folder`, given what `ownership` says.
    pub fn write(&self, folder: &Path, ownership: Ownership) -> Result<()> {
        let path = folder.join(MANIFEST);
        let text = toml::to_string(self).context("write the manifest")?;
        let mut file = skyward_files::make_file(&path, ownership)?;
        file.write_all(text.as_bytes()).with_context(|| format!("write {}", path.display()))?;
        file.sync_all().with_context(|| format!("write {}", path.display()))
    }

    /// Which of [`Part::ALL`] it holds.
    pub fn parts(&self) -> [bool; 4] {
        Part::ALL.map(|p| self.items.iter().any(|i| i.part == p))
    }

    /// The bytes its files take.
    pub fn size(&self) -> u64 {
        self.items.iter().map(|i| i.stored.stored).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use store::Stored;

    #[test]
    fn a_manifest_reads_back_as_written() {
        let at = std::env::temp_dir().join(format!("skyward-manifest-{}", std::process::id()));
        std::fs::create_dir_all(&at).unwrap();
        let stored = |name: &str, size| Stored { files: vec![format!("{name}.000")], size, stored: size / 2, compressed: true, sha256: "ab".repeat(32) };
        let m = Manifest {
            format: FORMAT,
            device: "ishtar".into(),
            made: 1_792_561_872,
            slot: "_b".into(),
            system: Some("OS3.0.305.0.WMACNXM".into()),
            data_key: Some("00112233445566778899aabbccddeeff".into()),
            items: vec![
                Item { part: Part::Boot, name: "boot".into(), stored: stored("boot", 201_326_592) },
                Item { part: Part::Data, name: "data".into(), stored: stored("data", 9_000_000_000) },
            ],
        };
        m.write(&at, Ownership::Kept).unwrap();
        assert_eq!(Manifest::read(&at).unwrap(), m);
        assert_eq!(m.parts(), [true, false, true, false]);
        assert_eq!(m.size(), 100_663_296 + 4_500_000_000);
        std::fs::remove_dir_all(&at).unwrap();
    }

    #[test]
    fn a_manifest_may_not_name_a_path_outside_its_folder() {
        let at = std::env::temp_dir().join(format!("skyward-manifest-outside-{}", std::process::id()));
        std::fs::create_dir_all(&at).unwrap();
        let stored = Stored { files: vec!["boot.000".into()], size: 1, stored: 1, compressed: true, sha256: "ab".repeat(32) };
        let m = Manifest {
            format: FORMAT,
            device: "kebab".into(),
            made: 1,
            slot: "_a".into(),
            system: None,
            data_key: None,
            items: vec![Item { part: Part::Boot, name: "boot".into(), stored }],
        };
        m.write(&at, Ownership::Kept).unwrap();
        let path = at.join(MANIFEST);
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, text.replace("name = \"boot\"", "name = \"../boot\"")).unwrap();
        assert!(Manifest::read(&at).is_err(), "a partition out of the phone's tables is refused");
        std::fs::write(&path, text.replace("boot.000", "../boot.000")).unwrap();
        assert!(Manifest::read(&at).is_err(), "a file out of the backup's folder is refused");
        std::fs::remove_dir_all(&at).unwrap();
    }
}
