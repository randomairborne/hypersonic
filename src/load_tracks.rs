use std::{
    ffi::OsStr,
    io::Cursor,
    path::{Path, PathBuf},
};

use symphonia::core::{
    formats::FormatOptions,
    io::{MediaSourceStream, MediaSourceStreamOptions},
    meta::{Metadata, MetadataOptions, StandardTagKey as Stk, Value as MetaValue},
    probe::Hint,
};

use crate::{Song, SongMetadata};

pub fn get_tracks(path: impl AsRef<Path>) -> Result<Box<[Song]>, TrackLoadError> {
    let root = path.as_ref();
    let file_list = get_file_list(root).map_err(|e| TrackLoadError::Io(root.to_path_buf(), e))?;
    let mut output = Vec::with_capacity(file_list.len());
    for file in file_list
        .iter()
        .filter(|v| v.extension().is_some_and(is_audio_extension))
    {
        let song = add_metadata(file)?;
        debug!(?song, "Loaded song");
        output.push(song);
    }
    Ok(output.into_boxed_slice())
}

fn add_metadata(path: &Path) -> Result<Song, TrackLoadError> {
    let data = std::fs::read(path)
        .map_err(|e| TrackLoadError::Io(path.to_path_buf(), e))?
        .into_boxed_slice();
    let mss = MediaSourceStream::new(
        Box::new(Cursor::new(data.clone())),
        MediaSourceStreamOptions::default(),
    );
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(OsStr::to_str) {
        hint.with_extension(extension);
    }
    let meta_opts = MetadataOptions::default();
    let fmt_opts = FormatOptions::default();
    let mut probed = symphonia::default::get_probe()
        .format(&hint, mss, &fmt_opts, &meta_opts)
        .map_err(|e| TrackLoadError::Symphonia(path.to_path_buf(), e))?;

    let mut artist: Option<Box<str>> = None;
    let mut title: Option<Box<str>> = None;
    let mut album: Option<Box<str>> = None;

    if let Some(metadata_rev) = probed.metadata.get().as_ref().and_then(Metadata::current) {
        for tag in metadata_rev.tags() {
            if let Some(tag_key) = tag.std_key {
                match (tag_key, &tag.value) {
                    (Stk::Album, MetaValue::String(detected_album)) => {
                        album = Some(detected_album.clone().into_boxed_str());
                    }
                    (Stk::Artist, MetaValue::String(detected_artist)) => {
                        artist = Some(detected_artist.clone().into_boxed_str());
                    }
                    (Stk::TrackTitle, MetaValue::String(detected_name)) => {
                        title = Some(detected_name.clone().into_boxed_str());
                    }
                    _ => {}
                }
            }
        }
    }

    let title = title.unwrap_or_else(|| {
        path.file_stem()
            .map_or_else(|| "Unknown".into(), |v| v.to_string_lossy().into())
    });

    let meta = SongMetadata {
        title,
        artist,
        album,
    };
    Ok(Song { data, meta })
}

fn get_file_list(root_path: &Path) -> Result<Vec<PathBuf>, std::io::Error> {
    let mut files = Vec::new();
    let mut stack: Vec<PathBuf> = vec![root_path.to_path_buf()];
    while let Some(item) = stack.pop() {
        let meta = std::fs::metadata(&item)?;
        if meta.is_dir() {
            stack.extend(get_directory_children(&item)?);
        } else if meta.is_file() {
            files.push(item);
        } else {
            eprintln!("File {} has an unknown time", item.display());
        }
    }
    Ok(files)
}

fn get_directory_children(path: &Path) -> Result<Vec<PathBuf>, std::io::Error> {
    let dir_read = std::fs::read_dir(path)?;
    let mut output = Vec::with_capacity(128);
    for path in dir_read {
        let path = path?;
        output.push(path.path());
    }
    Ok(output)
}

fn is_audio_extension(ext: &OsStr) -> bool {
    match ext.to_string_lossy().as_ref() {
        // taken from https://docs.rs/symphonia/latest/symphonia/#formats
        "aiff" | "caf" | "mp3" | "webm" | "mkv" | "ogg" | "wav" => true,
        _ => false,
    }
}

#[derive(thiserror::Error, Debug)]
pub enum TrackLoadError {
    #[error("{0} I/O error: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("{0} Symphonia error: {1}")]
    Symphonia(PathBuf, symphonia::core::errors::Error),
}
