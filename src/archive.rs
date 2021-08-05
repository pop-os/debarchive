use ar;
use libflate::gzip::Decoder as GzDecoder;
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader};
use std::path::Path;
use tar;
use xz2::read::XzDecoder;

pub struct Archive<'a> {
    path: &'a Path,
    data: (u8, Codec),
    control: (u8, Codec)
}

impl<'a> Archive<'a> {
    /// The path given must be a valid Debian ar archive. It will be scanned to verify that the
    /// inner data.tar and control.tar entries are reachable, and records their position.
    pub fn new(path: &'a Path) -> io::Result<Self> {
        let mut archive = ar::Archive::new(File::open(path)?);

        let mut control = None;
        let mut data = None;
        let mut entry_id = 0;

        while let Some(entry_result) = archive.next_entry() {
            if let Ok(entry) = entry_result {
                match entry.header().identifier() {
                    b"data.tar.xz" => data = Some((entry_id, Codec::Xz)),
                    b"data.tar.gz" => data = Some((entry_id, Codec::Gz)),
                    b"data.tar.zst" => data = Some((entry_id, Codec::Zstd)),
                    b"control.tar.xz" => control = Some((entry_id, Codec::Xz)),
                    b"control.tar.gz" => control = Some((entry_id, Codec::Gz)),
                    b"control.tar.zst" => control = Some((entry_id, Codec::Zstd)),
                    _ => {
                        entry_id += 1;
                        continue
                    }
                }

                if data.is_some() && control.is_some() { break }
            }

            entry_id += 1;
        }

        let data = data.ok_or_else(|| io::Error::new(
            io::ErrorKind::InvalidData,
            format!("data archive not found in {}", path.display())
        ))?;

        let control = control.ok_or_else(|| io::Error::new(
            io::ErrorKind::InvalidData,
            format!("control archive not found in {}", path.display())
        ))?;

        Ok(Archive { path, control, data })
    }

    /// Enables the caller to process entries from the inner control archive.
    pub fn control<F: FnMut(&mut tar::Entry<&mut dyn io::Read>) -> io::Result<()>>(&self, action: F) -> io::Result<()> {
        self.inner_control(action).map_err(|why| io::Error::new(
            io::ErrorKind::Other,
            format!("error reading control archive within {}: {}", self.path.display(), why)
        ))
    }

    /// Unpacks the inner control archive to the given path.
    pub fn control_extract<P: AsRef<Path>>(&self, path: P) -> io::Result<()> {
        self.extract(path, self.control.0, self.control.1)
    }

    // Enables the caller to get the contents of the control file in the control archive as a map
    pub fn control_map(&self) -> io::Result<BTreeMap<String, String>> {
        self.inner_control_map().map_err(|why| io::Error::new(
            io::ErrorKind::Other,
            format!("error reading control archive within {}: {}", self.path.display(), why)
        ))
    }

    /// Enables the caller to process entries from the inner data archive.
    pub fn data<F: FnMut(&mut tar::Entry<&mut dyn io::Read>) -> io::Result<()>>(&self, action: F) -> io::Result<()> {
        self.inner_data(action).map_err(|why| io::Error::new(
            io::ErrorKind::Other,
            format!("error reading data archive within {}: {}", self.path.display(), why)
        ))
    }

    /// Unpacks the inner data archive to the given path.
    pub fn data_extract<P: AsRef<Path>>(&self, path: P) -> io::Result<()> {
        self.extract(path, self.data.0, self.data.1)
    }

    fn open_archive<F, T>(&self, id: u8, codec: Codec, mut func: F) -> io::Result<T>
        where F: FnMut(&mut dyn io::Read) -> T,
    {
        let mut archive = ar::Archive::new(File::open(self.path)?);
        let inner_tar_archive = archive.jump_to_entry(id as usize)?;
        let mut reader: Box<dyn io::Read> = match codec {
            Codec::Zstd => Box::new(zstd::Decoder::new(inner_tar_archive)?),
            Codec::Xz => Box::new(XzDecoder::new(inner_tar_archive)),
            Codec::Gz => Box::new(GzDecoder::new(inner_tar_archive)?),
        };

        Ok(func(reader.as_mut()))
    }

    fn iter_entries<F: FnMut(&mut tar::Entry<&mut dyn io::Read>) -> io::Result<()>>(&self, mut action: F, id: u8, codec: Codec) -> io::Result<()> {
        self.open_archive(id, codec, |reader| {
            for entry in tar::Archive::new(reader).entries()? {
                let mut entry = entry?;
                if entry.header().entry_type().is_dir() {
                    continue
                }

                action(&mut entry)?;
            }

            Ok(())
        })?
    }

    fn inner_data<F: FnMut(&mut tar::Entry<&mut dyn io::Read>) -> io::Result<()>>(&self, action: F) -> io::Result<()> {
        self.iter_entries(action, self.data.0, self.data.1)
    }

    fn inner_control<F: FnMut(&mut tar::Entry<&mut dyn io::Read>) -> io::Result<()>>(&self, action: F) -> io::Result<()> {
        self.iter_entries(action, self.control.0, self.control.1)
    }

    fn extract<P: AsRef<Path>>(&self, path: P, id: u8, codec: Codec) -> io::Result<()> {
        let path = path.as_ref();
        if !path.exists() {
            fs::create_dir_all(path)?;
        }

        self.open_archive(id, codec, |reader| tar::Archive::new(reader).unpack(path))?
    }

    fn inner_control_map(&self) -> io::Result<BTreeMap<String, String>> {
        let (id, codec) = (self.control.0, self.control.1);
        self.open_archive(id, codec, |reader| {
            let mut control_data = BTreeMap::new();

            for entry in tar::Archive::new(reader).entries()? {
                let mut entry = entry?;
                let path = entry.path()?.to_path_buf();

                if path == Path::new("./control") || path == Path::new("control") {
                    let mut description_unset = true;
                    let mut lines = BufReader::new(&mut entry).lines().peekable();
                    while let Some(line) = lines.next() {
                        let line = line?;
                        if let Some(pos) = line.find(':') {
                            let (key, value) = line.split_at(pos);
                            let mut value: String = value[1..].trim().to_owned();

                            if description_unset && key == "Description" {
                                description_unset = false;
                                loop {
                                    match lines.peek() {
                                        Some(next_line) => {
                                            match *next_line {
                                                Ok(ref next_line) => {
                                                    if next_line.starts_with(' ') {
                                                        value.push('\n');
                                                        value.push_str(next_line);
                                                    } else {
                                                        break
                                                    }
                                                }
                                                Err(_) => break
                                            }
                                        }
                                        None => break
                                    }

                                    let _ = lines.next();
                                }
                            }

                            control_data.insert(key.to_owned(), value);
                        }
                    }
                }
            }

            Ok(control_data)
        })?
    }
}

#[derive(Copy, Clone, Debug)]
enum Codec {
    Xz,
    Gz,
    Zstd
}
