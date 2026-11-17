//! The disposable dashboard exchanges size-prefixed messages over private,
//! inherited pipes. Windows and macOS use the same frame and different JSON.
use std::io::{self, Read, Write};

#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(windows)]
pub mod windows;

pub const MAX_FRAME: usize = 4 * 1024 * 1024;

pub struct Endpoint {
    input: std::sync::Mutex<std::fs::File>,
    output: std::sync::Mutex<std::fs::File>,
}
impl Endpoint {
    /// Take the two private pipe handles passed by the Windows monitor.
    /// # Safety
    /// Both handle values must be owned by the caller, with no other closer.
    #[cfg(windows)]
    pub unsafe fn inherited(input: usize, output: usize) -> io::Result<Self> {
        use std::{fs::File, os::windows::io::FromRawHandle};
        use windows_sys::Win32::{
            Foundation::{HANDLE, HANDLE_FLAG_INHERIT, SetHandleInformation},
            Storage::FileSystem::{FILE_TYPE_PIPE, GetFileType},
        };
        if input == output {
            return Err(crate::files::invalid("Duplicate dashboard pipe"));
        }
        for handle in [input, output].map(|h| h as HANDLE) {
            if unsafe { GetFileType(handle) } != FILE_TYPE_PIPE {
                return Err(crate::files::invalid("Invalid dashboard pipe"));
            }
            if unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) } == 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(Self {
            input: std::sync::Mutex::new(unsafe { File::from_raw_handle(input as HANDLE) }),
            output: std::sync::Mutex::new(unsafe { File::from_raw_handle(output as HANDLE) }),
        })
    }
    /// Keep the standard descriptors available to Cocoa while owning close-on-exec duplicates.
    #[cfg(target_os = "macos")]
    pub fn standard() -> io::Result<Self> {
        use std::os::fd::AsFd;
        let output = std::io::stdout().as_fd().try_clone_to_owned()?;
        macos::suppress_sigpipe(&output)?;
        Ok(Self {
            input: std::sync::Mutex::new(std::io::stdin().as_fd().try_clone_to_owned()?.into()),
            output: std::sync::Mutex::new(output.into()),
        })
    }
    pub fn receive(&self) -> io::Result<Option<Vec<u8>>> {
        read_frame(&mut *self.input.lock().unwrap_or_else(|e| e.into_inner()))
    }
    pub fn send(&self, body: &[u8]) -> io::Result<()> {
        write_frame(
            &mut *self.output.lock().unwrap_or_else(|e| e.into_inner()),
            body,
        )
    }
}

fn size(header: [u8; 4]) -> io::Result<usize> {
    let n = u32::from_be_bytes(header) as usize;
    if !(1..=MAX_FRAME).contains(&n) {
        return Err(crate::files::invalid("Invalid dashboard frame size"));
    }
    Ok(n)
}

#[derive(Clone, Default)]
pub struct Decoder {
    buffer: Vec<u8>,
    expected: Option<usize>,
}
impl Decoder {
    pub fn append(&mut self, mut input: &[u8], mut deliver: impl FnMut(&[u8])) -> io::Result<()> {
        while !input.is_empty() {
            let total = self.expected.unwrap_or(4);
            let take = (total - self.buffer.len()).min(input.len());
            self.buffer.extend_from_slice(&input[..take]);
            input = &input[take..];
            if self.buffer.len() == total {
                if self.expected.is_some() {
                    deliver(&self.buffer);
                    zeroize::Zeroize::zeroize(&mut self.buffer);
                    self.buffer = Vec::new();
                    self.expected = None;
                } else {
                    let header = self.buffer.as_slice().try_into().expect("four-byte header");
                    self.buffer.clear();
                    self.expected = Some(size(header)?);
                }
            }
        }
        Ok(())
    }
    pub fn finish(&self) -> io::Result<()> {
        if self.expected.is_some() || !self.buffer.is_empty() {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        Ok(())
    }
}
impl Drop for Decoder {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.buffer);
    }
}
pub fn read_frame(reader: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
    let mut header = [0; 4];
    // EOF is a normal shutdown only on a frame boundary.
    loop {
        match reader.read(&mut header[..1]) {
            Ok(0) => return Ok(None),
            Ok(_) => break,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    reader.read_exact(&mut header[1..])?;
    let mut data = vec![0; size(header)?];
    reader.read_exact(&mut data)?;
    Ok(Some(data))
}

pub fn write_frame(writer: &mut impl Write, data: &[u8]) -> io::Result<()> {
    if data.is_empty() || data.len() > MAX_FRAME {
        return Err(crate::files::invalid("Invalid dashboard frame size"));
    }
    writer.write_all(&(data.len() as u32).to_be_bytes())?;
    writer.write_all(data)?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    struct Fragments(Cursor<Vec<u8>>);
    impl Read for Fragments {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            let n = bytes.len().min(2);
            self.0.read(&mut bytes[..n])
        }
    }
    #[test]
    fn fragmented_frames_and_eof_are_unambiguous() {
        let mut wire = Vec::new();
        for value in ["한글😀", "a", "repeat"] {
            write_frame(&mut wire, value.as_bytes()).unwrap();
        }
        let mut fragmented = Fragments(Cursor::new(wire.clone()));
        for value in ["한글😀", "a", "repeat"] {
            assert_eq!(
                read_frame(&mut fragmented).unwrap().unwrap(),
                value.as_bytes()
            );
        }
        assert!(read_frame(&mut fragmented).unwrap().is_none());
        for malformed in [
            vec![0],
            vec![0, 0, 0, 4, 42],
            vec![0, 0, 0, 0],
            vec![0xff; 4],
        ] {
            assert!(read_frame(&mut Cursor::new(malformed)).is_err());
        }
        assert!(write_frame(&mut Vec::new(), &[]).is_err());
        assert!(write_frame(&mut Vec::new(), &vec![0; MAX_FRAME + 1]).is_err());
        for n in [1, 3, 4, 7, wire.len()] {
            let mut decoder = Decoder::default();
            let mut frames = Vec::new();
            for chunk in wire.chunks(n) {
                decoder
                    .append(chunk, |frame| frames.push(frame.to_vec()))
                    .unwrap();
            }
            decoder.finish().unwrap();
            assert_eq!(
                frames,
                ["한글😀", "a", "repeat"].map(|v| v.as_bytes().to_vec())
            );
        }
        let mut half = Decoder::default();
        half.append(&[0, 0, 0, 2, 65], |_| panic!()).unwrap();
        let mut copy = half.clone();
        half.append(b"B", |b| assert_eq!(b, b"AB")).unwrap();
        assert!(copy.finish().is_err());
        copy.append(b"C", |b| assert_eq!(b, b"AC")).unwrap();
    }
}
