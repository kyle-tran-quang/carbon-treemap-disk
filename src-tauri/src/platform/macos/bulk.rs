//! Directory reader built on `getattrlistbulk(2)`.
//!
//! One syscall returns names, object types, device, inode, link count and
//! allocated size for many entries, replacing the `readdir` + per-entry
//! `lstat` pair used by the portable reader.
//!
//! Each returned record is: `u32 length`, an `attribute_set_t` of the
//! attributes actually returned, then those attributes packed in the order
//! the kernel documents: `ERROR` first, then the common group by ascending
//! bit, then the file group.
//! Fields can be unaligned, so every read goes through `read_unaligned`.

use std::cell::RefCell;
use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::mem::size_of;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStringExt;
use std::path::Path;

use libc::{
    ATTR_BIT_MAP_COUNT, ATTR_CMN_DEVID, ATTR_CMN_FILEID, ATTR_CMN_NAME, ATTR_CMN_OBJTYPE,
    ATTR_CMN_RETURNED_ATTRS, ATTR_FILE_ALLOCSIZE, ATTR_FILE_LINKCOUNT, FSOPT_PACK_INVAL_ATTRS,
    attrgroup_t, attrlist, attrreference_t,
};

/// Not exported by the `libc` crate. Value from `<sys/attr.h>`.
const ATTR_CMN_ERROR: attrgroup_t = 0x2000_0000;
/// `fsobj_type_t` values from `<sys/vnode.h>`.
const VDIR: u32 = 2;

const BUFFER_SIZE: usize = 256 * 1024;

thread_local! {
    static BUFFER: RefCell<Vec<u8>> = RefCell::new(vec![0u8; BUFFER_SIZE]);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BulkKind {
    Dir,
    /// Anything that is not a directory (regular file, symlink, device, ...).
    Other,
}

#[derive(Debug)]
pub struct BulkEntry {
    pub name: OsString,
    pub kind: BulkKind,
    pub dev: u32,
    pub inode: u64,
    pub nlink: u32,
    pub allocated: u64,
}

/// A per-entry failure reported by the kernel (for example EPERM).
#[derive(Debug)]
pub struct BulkEntryError {
    pub name: OsString,
    pub error: io::Error,
}

#[derive(Debug, Default)]
pub struct BulkListing {
    pub entries: Vec<BulkEntry>,
    pub errors: Vec<BulkEntryError>,
}

pub fn read_dir_bulk(path: &Path) -> io::Result<BulkListing> {
    let dir = File::open(path)?;
    BUFFER.with(|buffer| {
        let mut buffer = buffer.borrow_mut();
        let mut listing = BulkListing::default();
        let mut request = attrlist {
            bitmapcount: ATTR_BIT_MAP_COUNT,
            reserved: 0,
            commonattr: ATTR_CMN_RETURNED_ATTRS
                | ATTR_CMN_NAME
                | ATTR_CMN_DEVID
                | ATTR_CMN_OBJTYPE
                | ATTR_CMN_FILEID
                | ATTR_CMN_ERROR,
            volattr: 0,
            dirattr: 0,
            fileattr: ATTR_FILE_LINKCOUNT | ATTR_FILE_ALLOCSIZE,
            forkattr: 0,
        };
        loop {
            // SAFETY: `request` and `buffer` are valid for the call, and the
            // size passed matches the buffer length.
            let count = unsafe {
                libc::getattrlistbulk(
                    dir.as_raw_fd(),
                    (&raw mut request).cast(),
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                    u64::from(FSOPT_PACK_INVAL_ATTRS),
                )
            };
            match count {
                0 => return Ok(listing),
                n if n < 0 => return Err(io::Error::last_os_error()),
                n => parse_records(&buffer, n as usize, &mut listing)?,
            }
        }
    })
}

/// Bounds-checked cursor over one record.
struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn take<T: Copy>(&mut self) -> io::Result<T> {
        let end = self.pos + size_of::<T>();
        if end > self.bytes.len() {
            return Err(malformed());
        }
        // SAFETY: bounds checked above; `T` is a plain integer or POD struct.
        let value = unsafe {
            self.bytes
                .as_ptr()
                .add(self.pos)
                .cast::<T>()
                .read_unaligned()
        };
        self.pos = end;
        Ok(value)
    }
}

fn malformed() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "malformed getattrlistbulk record",
    )
}

fn parse_records(buffer: &[u8], count: usize, out: &mut BulkListing) -> io::Result<()> {
    let mut offset = 0usize;
    for _ in 0..count {
        let header = buffer.get(offset..offset + 4).ok_or_else(malformed)?;
        let length = u32::from_ne_bytes(header.try_into().expect("4 bytes")) as usize;
        let record = buffer.get(offset..offset + length).ok_or_else(malformed)?;
        parse_record(record, out)?;
        offset += length;
    }
    Ok(())
}

fn parse_record(record: &[u8], out: &mut BulkListing) -> io::Result<()> {
    let mut reader = Reader {
        bytes: record,
        pos: 4,
    };
    let returned: libc::attribute_set_t = reader.take()?;
    let common = returned.commonattr;
    let files = returned.fileattr;

    let error: u32 = if common & ATTR_CMN_ERROR != 0 {
        reader.take()?
    } else {
        0
    };

    let mut name = OsString::new();
    if common & ATTR_CMN_NAME != 0 {
        let reference_pos = reader.pos;
        let reference: attrreference_t = reader.take()?;
        let start = reference_pos
            .checked_add_signed(reference.attr_dataoffset as isize)
            .ok_or_else(malformed)?;
        let raw = record
            .get(start..start + reference.attr_length as usize)
            .ok_or_else(malformed)?;
        // The stored length includes the trailing NUL.
        let raw = raw.strip_suffix(&[0]).unwrap_or(raw);
        name = OsString::from_vec(raw.to_vec());
    }
    let dev: u32 = if common & ATTR_CMN_DEVID != 0 {
        reader.take()?
    } else {
        0
    };
    let objtype: u32 = if common & ATTR_CMN_OBJTYPE != 0 {
        reader.take()?
    } else {
        0
    };
    let inode: u64 = if common & ATTR_CMN_FILEID != 0 {
        reader.take()?
    } else {
        0
    };
    if error != 0 {
        out.errors.push(BulkEntryError {
            name,
            error: io::Error::from_raw_os_error(error as i32),
        });
        return Ok(());
    }
    let nlink: u32 = if files & ATTR_FILE_LINKCOUNT != 0 {
        reader.take()?
    } else {
        1
    };
    let allocated: i64 = if files & ATTR_FILE_ALLOCSIZE != 0 {
        reader.take()?
    } else {
        0
    };
    out.entries.push(BulkEntry {
        name,
        kind: if objtype == VDIR {
            BulkKind::Dir
        } else {
            BulkKind::Other
        },
        dev,
        inode,
        nlink,
        allocated: allocated.max(0) as u64,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ctd-bulk-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn lists_names_kinds_and_sizes() {
        let dir = temp_dir("list");
        fs::File::create(dir.join("file.bin"))
            .unwrap()
            .write_all(&[1u8; 8192])
            .unwrap();
        fs::create_dir(dir.join("sub")).unwrap();
        std::os::unix::fs::symlink(&dir, dir.join("link")).unwrap();

        let listing = read_dir_bulk(&dir).unwrap();
        let find = |n: &str| listing.entries.iter().find(|e| e.name == n).unwrap();
        assert_eq!(listing.entries.len(), 3);
        assert_eq!(find("sub").kind, BulkKind::Dir);
        assert_eq!(find("link").kind, BulkKind::Other);
        let file = find("file.bin");
        assert_eq!(file.kind, BulkKind::Other);
        assert!(file.allocated >= 8192);
        assert_eq!(file.nlink, 1);
        assert!(listing.errors.is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn large_directory_spans_multiple_syscalls() {
        let dir = temp_dir("many");
        for i in 0..5000 {
            fs::File::create(dir.join(format!("a-long-file-name-{i:06}.txt"))).unwrap();
        }
        let listing = read_dir_bulk(&dir).unwrap();
        assert_eq!(listing.entries.len(), 5000);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn missing_directory_is_an_error() {
        let err = read_dir_bulk(Path::new("/no/such/dir/ctd")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn non_utf8_names_round_trip() {
        use std::os::unix::ffi::OsStrExt;
        let dir = temp_dir("bytes");
        let name = std::ffi::OsStr::from_bytes(b"caf\xc3\xa9-\xff");
        // APFS rejects invalid UTF-8; skip when the filesystem does too.
        if fs::File::create(dir.join(name)).is_ok() {
            let listing = read_dir_bulk(&dir).unwrap();
            assert_eq!(listing.entries[0].name, name);
        }
        fs::remove_dir_all(dir).unwrap();
    }
}
