// SPDX-License-Identifier: MPL-2.0

use core::ops::Range;
use runtime::{Error, FrameAllocation, Handle};

pub struct Archive {
    frames: Handle<FrameAllocation>,
    length: usize,
}

pub struct File<'a> {
    archive: &'a Archive,
    range: Range<usize>,
}

impl Archive {
    pub fn new(frames: Handle<FrameAllocation>, length: usize) -> Result<Self, Error> {
        let capacity = frames.frames()?.checked_mul(4096).ok_or(Error::Protocol)?;
        if length == 0 || length as u64 > capacity {
            return Err(Error::Protocol);
        }
        Ok(Self { frames, length })
    }

    // REFERENCE: https://www.kernel.org/doc/html/latest/driver-api/early-userspace/buffer-format.html
    pub fn executable(&self, path: &[u8]) -> Result<File<'_>, Error> {
        let mut offset = 0usize;
        let mut found = None;
        loop {
            let mut header = [0; 110];
            self.read(offset, &mut header)?;
            if &header[..6] != b"070701" || hex(&header[102..110])? != 0 {
                return Err(Error::Protocol);
            }
            let size = hex(&header[54..62])?;
            let name_size = hex(&header[94..102])?;
            let name_start = offset.checked_add(header.len()).ok_or(Error::Protocol)?;
            let name_end = name_start
                .checked_add(name_size)
                .filter(|&end| end <= self.length)
                .ok_or(Error::Protocol)?;
            if name_size == 0 {
                return Err(Error::Protocol);
            }
            let data_start = name_end
                .checked_next_multiple_of(4)
                .ok_or(Error::Protocol)?;
            let data_end = data_start
                .checked_add(size)
                .filter(|&end| end <= self.length)
                .ok_or(Error::Protocol)?;
            let mut terminator = [0];
            self.read(name_end - 1, &mut terminator)?;
            if terminator != [0] {
                return Err(Error::Protocol);
            }
            let mut name = name_start..name_end - 1;
            if self.matches(name.clone(), b"TRAILER!!!")? {
                if size != 0 {
                    return Err(Error::Protocol);
                }
                return found.ok_or(Error::Protocol);
            }
            if name.len() >= 2 {
                let mut prefix = [0; 2];
                self.read(name.start, &mut prefix)?;
                if prefix == *b"./" {
                    name.start += 2;
                }
            }
            if self.matches(name, path)? {
                let mode = hex(&header[14..22])?;
                if mode & 0o170000 != 0o100000
                    || mode & 0o111 == 0
                    || hex(&header[38..46])? != 1
                    || found.is_some()
                {
                    return Err(Error::Protocol);
                }
                found = Some(File {
                    archive: self,
                    range: data_start..data_end,
                });
            }
            offset = data_end
                .checked_next_multiple_of(4)
                .ok_or(Error::Protocol)?;
        }
    }

    fn read(&self, offset: usize, output: &mut [u8]) -> Result<(), Error> {
        if offset
            .checked_add(output.len())
            .is_none_or(|end| end > self.length)
        {
            return Err(Error::Protocol);
        }
        self.frames.read(offset, output)
    }

    fn matches(&self, name: Range<usize>, expected: &[u8]) -> Result<bool, Error> {
        if name.len() != expected.len() {
            return Ok(false);
        }
        let mut buffer = [0; 32];
        for (index, chunk) in expected.chunks(buffer.len()).enumerate() {
            self.read(
                name.start + index * buffer.len(),
                &mut buffer[..chunk.len()],
            )?;
            if &buffer[..chunk.len()] != chunk {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

impl File<'_> {
    pub fn len(&self) -> usize {
        self.range.len()
    }

    pub fn read(&self, offset: usize, output: &mut [u8]) -> Result<(), Error> {
        if offset
            .checked_add(output.len())
            .is_none_or(|end| end > self.len())
        {
            return Err(Error::Protocol);
        }
        self.archive.read(self.range.start + offset, output)
    }
}

fn hex(bytes: &[u8]) -> Result<usize, Error> {
    bytes.iter().try_fold(0usize, |value, &digit| {
        let digit = match digit {
            b'0'..=b'9' => digit - b'0',
            b'a'..=b'f' => digit - b'a' + 10,
            b'A'..=b'F' => digit - b'A' + 10,
            _ => return Err(Error::Protocol),
        };
        value
            .checked_mul(16)
            .and_then(|value| value.checked_add(digit as usize))
            .ok_or(Error::Protocol)
    })
}
