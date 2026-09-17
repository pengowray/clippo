use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub const MAX_SIZE: u32 = 256;

pub fn path(dir: &Path, id: i64) -> PathBuf {
    dir.join(format!("{id}.png"))
}

/// Return the cached thumbnail for an entry, creating it from `content` if needed.
pub fn ensure(dir: &Path, id: i64, content: &[u8]) -> Result<PathBuf> {
    let out = path(dir, id);
    if out.exists() {
        return Ok(out);
    }
    std::fs::create_dir_all(dir).with_context(|| format!("could not create {}", dir.display()))?;
    let img = image::load_from_memory(content).context("could not decode image")?;
    let thumb = img.thumbnail(MAX_SIZE, MAX_SIZE);
    // Write then rename so a concurrent reader never sees a partial file.
    let tmp = dir.join(format!(".{id}.{}.tmp.png", std::process::id()));
    thumb.save(&tmp).context("could not write thumbnail")?;
    std::fs::rename(&tmp, &out)?;
    Ok(out)
}

pub fn remove(dir: &Path, ids: &[i64]) {
    for id in ids {
        let _ = std::fs::remove_file(path(dir, *id));
    }
}

pub fn remove_all(dir: &Path) {
    let _ = std::fs::remove_dir_all(dir);
}
