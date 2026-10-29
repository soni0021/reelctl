//! The content-addressed store: every output file lives once, read-only, under
//! `objects/<aa>/<blake3>`. A directory output is a tree object, a JSON manifest of
//! its entries, so a whole filmstrip folder or reel folder has one hash too.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Result, StudioError};

/// The folder of a workspace the service owns.
pub const STUDIO_DIR: &str = ".reelstudio";
/// The `kind` of a tree manifest.
pub const TREE_KIND: &str = "reelstudio.tree";

/// A stored object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectInfo {
    pub hash: String,
    pub size: u64,
    /// `file` or `tree`.
    pub kind: String,
}

/// One entry of a tree manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeEntry {
    pub name: String,
    pub hash: String,
    pub size: u64,
    /// `file` or `tree`.
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct TreeManifest {
    kind: String,
    entries: Vec<TreeEntry>,
}

/// The `.reelstudio/` folder of a workspace.
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|error| StudioError::io(path, error))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

/// The BLAKE3 hash (hex) and size of a file, read in pieces.
pub fn hash_file(path: &Path) -> Result<(String, u64)> {
    let mut file = fs::File::open(path).map_err(|error| StudioError::io(path, error))?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0u8; 1 << 16];
    let mut size = 0u64;
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| StudioError::io(path, error))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size += read as u64;
    }
    Ok((hasher.finalize().to_hex().to_string(), size))
}

/// A temporary name no other thread or process of this store uses at once.
fn temporary_name(hash: &str) -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!(".{hash}.{}.{n}.tmp", std::process::id())
}

fn valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl Store {
    /// Open (and make, mode 0700) `<workspace>/.reelstudio` with its `objects/`
    /// and `jobs/` folders.
    pub fn open(workspace: &Path) -> Result<Self> {
        let root = workspace.join(STUDIO_DIR);
        for dir in [root.clone(), root.join("objects"), root.join("jobs")] {
            fs::create_dir_all(&dir).map_err(|error| StudioError::io(&dir, error))?;
        }
        set_mode(&root, 0o700)?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where the object with `hash` lives.
    pub fn path(&self, hash: &str) -> Result<PathBuf> {
        if !valid_hash(hash) {
            return Err(StudioError::invalid(format!(
                "\"{hash}\" is not an object hash"
            )));
        }
        Ok(self.root.join("objects").join(&hash[..2]).join(hash))
    }

    pub fn has(&self, hash: &str) -> bool {
        self.path(hash).is_ok_and(|path| path.is_file())
    }

    /// Every object in the store, with its size.
    pub fn objects(&self) -> Result<Vec<(String, u64)>> {
        let mut found = Vec::new();
        let objects = self.root.join("objects");
        let shards = fs::read_dir(&objects).map_err(|error| StudioError::io(&objects, error))?;
        for shard in shards {
            let shard = shard
                .map_err(|error| StudioError::io(&objects, error))?
                .path();
            if !shard.is_dir() {
                continue;
            }
            let entries = fs::read_dir(&shard).map_err(|error| StudioError::io(&shard, error))?;
            for entry in entries {
                let entry = entry.map_err(|error| StudioError::io(&shard, error))?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if !valid_hash(&name) {
                    continue;
                }
                let size = entry
                    .metadata()
                    .map_err(|error| StudioError::io(&entry.path(), error))?
                    .len();
                found.push((name, size));
            }
        }
        found.sort();
        Ok(found)
    }

    /// Delete the object with `hash` (garbage collection only).
    pub fn remove(&self, hash: &str) -> Result<()> {
        let path = self.path(hash)?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(StudioError::io(&path, error)),
        }
    }

    /// A job's scratch folder, made empty.
    pub fn scratch(&self, job: i64) -> Result<PathBuf> {
        let dir = self.root.join("jobs").join(job.to_string());
        if dir.exists() {
            fs::remove_dir_all(&dir).map_err(|error| StudioError::io(&dir, error))?;
        }
        fs::create_dir_all(&dir).map_err(|error| StudioError::io(&dir, error))?;
        Ok(dir)
    }

    /// Store `bytes`. The same bytes give the same object, stored once.
    pub fn put_bytes(&self, bytes: &[u8], kind: &str) -> Result<ObjectInfo> {
        let hash = blake3::hash(bytes).to_hex().to_string();
        let path = self.path(&hash)?;
        if !path.is_file() {
            let dir = path
                .parent()
                .ok_or_else(|| StudioError::invalid("an object path has no folder"))?;
            fs::create_dir_all(dir).map_err(|error| StudioError::io(dir, error))?;
            let temporary = dir.join(temporary_name(&hash));
            let _registered = progress::temporary(&temporary);
            let mut file =
                fs::File::create(&temporary).map_err(|error| StudioError::io(&temporary, error))?;
            file.write_all(bytes)
                .map_err(|error| StudioError::io(&temporary, error))?;
            file.sync_all()
                .map_err(|error| StudioError::io(&temporary, error))?;
            drop(file);
            set_mode(&temporary, 0o400)?;
            fs::rename(&temporary, &path).map_err(|error| StudioError::io(&path, error))?;
        }
        Ok(ObjectInfo {
            hash,
            size: bytes.len() as u64,
            kind: kind.to_string(),
        })
    }

    /// Store the file at `source` (copied; `source` is left as it is).
    pub fn put_file(&self, source: &Path) -> Result<ObjectInfo> {
        let (hash, size) = hash_file(source)?;
        let path = self.path(&hash)?;
        if !path.is_file() {
            let dir = path
                .parent()
                .ok_or_else(|| StudioError::invalid("an object path has no folder"))?;
            fs::create_dir_all(dir).map_err(|error| StudioError::io(dir, error))?;
            let temporary = dir.join(temporary_name(&hash));
            let _registered = progress::temporary(&temporary);
            fs::copy(source, &temporary).map_err(|error| StudioError::io(source, error))?;
            set_mode(&temporary, 0o400)?;
            // A file that changed while it was copied does not get a wrong name.
            let (copied, _) = hash_file(&temporary)?;
            if copied != hash {
                let _ = fs::remove_file(&temporary);
                return Err(StudioError::invalid(format!(
                    "{} changed while it was stored",
                    source.display()
                )));
            }
            fs::rename(&temporary, &path).map_err(|error| StudioError::io(&path, error))?;
        }
        Ok(ObjectInfo {
            hash,
            size,
            kind: "file".to_string(),
        })
    }

    /// Store the folder at `source` and everything in it, as a tree object. Entries
    /// are sorted by name, so the same contents give the same hash. Symbolic links
    /// are refused (an output never needs one, and following one could store files
    /// from outside the folder).
    pub fn put_tree(&self, source: &Path) -> Result<ObjectInfo> {
        let mut names: Vec<_> = fs::read_dir(source)
            .map_err(|error| StudioError::io(source, error))?
            .map(|entry| entry.map_err(|error| StudioError::io(source, error)))
            .collect::<Result<_>>()?;
        names.sort_by_key(|entry| entry.file_name());
        let mut entries = Vec::with_capacity(names.len());
        for entry in names {
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            let kind = entry
                .file_type()
                .map_err(|error| StudioError::io(&path, error))?;
            let info = if kind.is_symlink() {
                return Err(StudioError::invalid(format!(
                    "{} is a symbolic link; outputs are plain files",
                    path.display()
                )));
            } else if kind.is_dir() {
                self.put_tree(&path)?
            } else {
                self.put_file(&path)?
            };
            entries.push(TreeEntry {
                name,
                hash: info.hash,
                size: info.size,
                kind: info.kind,
            });
        }
        let manifest = TreeManifest {
            kind: TREE_KIND.to_string(),
            entries,
        };
        let bytes = serde_json::to_vec(&manifest)?;
        let info = self.put_bytes(&bytes, "tree")?;
        Ok(ObjectInfo {
            size: manifest.entries.iter().map(|entry| entry.size).sum(),
            ..info
        })
    }

    /// The object's bytes, checked against its hash.
    pub fn read(&self, hash: &str) -> Result<Vec<u8>> {
        let path = self.path(hash)?;
        let bytes = fs::read(&path).map_err(|error| StudioError::io(&path, error))?;
        let actual = blake3::hash(&bytes).to_hex().to_string();
        if actual != hash {
            return Err(StudioError::corrupt(format!(
                "object {hash} reads back as {actual}"
            )));
        }
        Ok(bytes)
    }

    /// The entries of a tree object.
    pub fn tree(&self, hash: &str) -> Result<Vec<TreeEntry>> {
        let manifest: TreeManifest =
            serde_json::from_slice(&self.read(hash)?).map_err(|error| {
                StudioError::corrupt(format!("object {hash} is not a tree: {error}"))
            })?;
        if manifest.kind != TREE_KIND {
            return Err(StudioError::corrupt(format!("object {hash} is not a tree")));
        }
        Ok(manifest.entries)
    }

    /// Lay a tree object out at `dest` (which must not exist): folders made, files
    /// hard-linked from the store (copied where links are not possible), read-only.
    pub fn materialize(&self, hash: &str, dest: &Path) -> Result<()> {
        if dest.exists() {
            return Err(StudioError::invalid(format!(
                "{} already exists",
                dest.display()
            )));
        }
        fs::create_dir_all(dest).map_err(|error| StudioError::io(dest, error))?;
        for entry in self.tree(hash)? {
            if entry.name.contains('/') || entry.name == ".." || entry.name == "." {
                return Err(StudioError::corrupt(format!(
                    "tree {hash} has an entry named \"{}\"",
                    entry.name
                )));
            }
            let target = dest.join(&entry.name);
            if entry.kind == "tree" {
                self.materialize(&entry.hash, &target)?;
            } else {
                let source = self.path(&entry.hash)?;
                fs::hard_link(&source, &target)
                    .or_else(|_| fs::copy(&source, &target).map(|_| ()))
                    .map_err(|error| StudioError::io(&target, error))?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("studio-store-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_same_bytes_are_one_read_only_object() {
        let store = Store::open(&workspace("bytes")).unwrap();
        let a = store.put_bytes(b"hello", "file").unwrap();
        let b = store.put_bytes(b"hello", "file").unwrap();
        assert_eq!(a, b);
        assert_eq!(a.hash, blake3::hash(b"hello").to_hex().to_string());
        assert_eq!(store.read(&a.hash).unwrap(), b"hello");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(store.path(&a.hash).unwrap())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o400);
            let root = fs::metadata(store.root()).unwrap().permissions().mode();
            assert_eq!(root & 0o777, 0o700);
        }
    }

    #[test]
    fn a_file_is_copied_in_and_left_where_it_was() {
        let dir = workspace("file");
        let store = Store::open(&dir).unwrap();
        let source = dir.join("out.json");
        fs::write(&source, b"{\"a\":1}").unwrap();
        let info = store.put_file(&source).unwrap();
        assert!(source.is_file());
        assert_eq!(info.size, 7);
        assert_eq!(store.read(&info.hash).unwrap(), b"{\"a\":1}");
    }

    #[test]
    fn a_folder_is_a_tree_that_lays_out_again() {
        let dir = workspace("tree");
        let store = Store::open(&dir).unwrap();
        let source = dir.join("views");
        fs::create_dir_all(source.join("inner")).unwrap();
        fs::write(source.join("b.png"), b"bbb").unwrap();
        fs::write(source.join("a.png"), b"a").unwrap();
        fs::write(source.join("inner/c.txt"), b"cc").unwrap();
        let tree = store.put_tree(&source).unwrap();
        assert_eq!(tree.kind, "tree");
        assert_eq!(tree.size, 6);
        let names: Vec<String> = store
            .tree(&tree.hash)
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, ["a.png", "b.png", "inner"]);
        // The same contents give the same tree.
        assert_eq!(store.put_tree(&source).unwrap(), tree);
        let copy = dir.join("again");
        store.materialize(&tree.hash, &copy).unwrap();
        assert_eq!(fs::read(copy.join("inner/c.txt")).unwrap(), b"cc");
        assert!(store.materialize(&tree.hash, &copy).is_err());
    }

    #[test]
    fn a_changed_object_and_a_bad_hash_are_errors() {
        let store = Store::open(&workspace("corrupt")).unwrap();
        let info = store.put_bytes(b"right", "file").unwrap();
        let path = store.path(&info.hash).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        fs::write(&path, b"wrong").unwrap();
        assert!(matches!(
            store.read(&info.hash),
            Err(StudioError::Corrupt(_))
        ));
        assert!(store.path("../../etc/passwd").is_err());
        assert!(store.path("ABC").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_symbolic_link_in_an_output_is_refused() {
        let dir = workspace("link");
        let store = Store::open(&dir).unwrap();
        let source = dir.join("out");
        fs::create_dir_all(&source).unwrap();
        std::os::unix::fs::symlink("/etc/hostname", source.join("x")).unwrap();
        assert!(store.put_tree(&source).is_err());
    }
}
