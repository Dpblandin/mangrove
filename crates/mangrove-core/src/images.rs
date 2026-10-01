//! Bounded, read-only image bytes. Decoding belongs to the native UI.
use git2::{DiffDelta, ObjectType, Oid, Repository};

use crate::{
    model::{FileImageData, FileImages},
    Result, MAX_FILE_SIZE,
};

pub(crate) fn blob(repository: &Repository, id: Oid, mode: i32) -> Result<FileImageData> {
    if id.is_zero() || mode == 0 {
        return Ok(FileImageData::Absent);
    }
    if !matches!(mode, 0o100644 | 0o100664 | 0o100755) {
        return Ok(FileImageData::Unavailable(
            "Les liens symboliques et les sous-modules n’ont pas d’aperçu image.".into(),
        ));
    }
    let (size, kind) = repository
        .odb()
        .and_then(|odb| odb.read_header(id))
        .map_err(|error| error.to_string())?;
    if kind != ObjectType::Blob {
        return Ok(FileImageData::Unavailable(
            "Objet Git non pris en charge.".into(),
        ));
    }
    if size > MAX_FILE_SIZE as usize {
        return Ok(oversized());
    }
    let blob = repository
        .find_blob(id)
        .map_err(|error| error.to_string())?;
    Ok(FileImageData::Bytes(blob.content().to_vec()))
}

pub(crate) fn oversized() -> FileImageData {
    FileImageData::Unavailable("Image supérieure à 2 Mio : aperçu non chargé.".into())
}

pub(crate) fn delta(repository: &Repository, delta: &DiffDelta<'_>) -> Result<FileImages> {
    Ok(FileImages {
        before: Some(blob(
            repository,
            delta.old_file().id(),
            delta.old_file().mode().into(),
        )?),
        after: blob(
            repository,
            delta.new_file().id(),
            delta.new_file().mode().into(),
        )?,
    })
}
