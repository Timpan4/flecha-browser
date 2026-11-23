use std::ffi::{CStr, CString, c_char};
use std::fs::{self, File};
use std::io::{self, Read as _, Seek as _, SeekFrom};
use std::os::unix::ffi::OsStrExt as _;
use std::path::Path;
use std::ptr::{self, NonNull};
use std::slice;

use anyhow::{Context as _, Result, bail, ensure};
use crc32c::Crc32cReader;
use cruzbit_leveldb_sys as ffi;

pub(super) struct Database(NonNull<ffi::leveldb_t>);

impl Database {
    pub(super) fn open(path: &Path) -> Result<Self> {
        validate_copy(path).context("validate copied extension LevelDB")?;
        let path = CString::new(path.as_os_str().as_bytes()).context("encode LevelDB path")?;
        // SAFETY: Options and path live through the synchronous call. LevelDB copies its
        // options and the returned database handle is owned by this wrapper.
        unsafe {
            let options =
                NonNull::new(ffi::leveldb_options_create()).context("allocate LevelDB options")?;
            ffi::leveldb_options_set_paranoid_checks(options.as_ptr(), 1);
            let db = with_status(|error| ffi::leveldb_open(options.as_ptr(), path.as_ptr(), error));
            ffi::leveldb_options_destroy(options.as_ptr());
            let db = db.context("open copied extension LevelDB")?;
            Ok(Self(NonNull::new(db).context("missing LevelDB handle")?))
        }
    }

    pub(super) fn scan(&self, mut visit: impl FnMut(&[u8], &[u8]) -> Result<()>) -> Result<()> {
        // SAFETY: The iterator owns its read options and borrows the open database for this scan.
        let iterator = unsafe {
            let options = NonNull::new(ffi::leveldb_readoptions_create())
                .context("allocate LevelDB read options")?;
            ffi::leveldb_readoptions_set_verify_checksums(options.as_ptr(), 1);
            ffi::leveldb_readoptions_set_fill_cache(options.as_ptr(), 0);
            let iterator = ffi::leveldb_create_iterator(self.0.as_ptr(), options.as_ptr());
            ffi::leveldb_readoptions_destroy(options.as_ptr());
            IteratorHandle(NonNull::new(iterator).context("allocate LevelDB iterator")?)
        };
        let iterator_ptr = iterator.0.as_ptr();
        // SAFETY: The iterator is valid until IteratorHandle drops. Key/value borrows end
        // before advancing. The C API exposes their exact byte lengths without C-string rules.
        unsafe {
            ffi::leveldb_iter_seek_to_first(iterator_ptr);
            while ffi::leveldb_iter_valid(iterator_ptr) != 0 {
                let mut key_len = 0;
                let key = ffi::leveldb_iter_key(iterator_ptr, &raw mut key_len);
                let mut value_len = 0;
                let value = ffi::leveldb_iter_value(iterator_ptr, &raw mut value_len);
                let key = if key_len == 0 {
                    &[]
                } else {
                    slice::from_raw_parts(key.cast(), key_len)
                };
                let value = if value_len == 0 {
                    &[]
                } else {
                    slice::from_raw_parts(value.cast(), value_len)
                };
                visit(key, value)?;
                ffi::leveldb_iter_next(iterator_ptr);
            }
            // An exhausted iterator can hold a sticky read error. Check before publishing.
            with_status(|error| ffi::leveldb_iter_get_error(iterator_ptr, error.cast()))
                .context("scan copied extension LevelDB")
        }
    }
}

impl Drop for Database {
    fn drop(&mut self) {
        // SAFETY: This wrapper owns the open handle and scans cannot outlive it.
        unsafe { ffi::leveldb_close(self.0.as_ptr()) };
    }
}

struct IteratorHandle(NonNull<ffi::leveldb_iterator_t>);

impl Drop for IteratorHandle {
    fn drop(&mut self) {
        // SAFETY: The handle came from leveldb_create_iterator and is owned here.
        unsafe { ffi::leveldb_iter_destroy(self.0.as_ptr()) };
    }
}

fn with_status<T>(call: impl FnOnce(*mut *mut c_char) -> T) -> Result<T> {
    let mut error = ptr::null_mut();
    let value = call(&raw mut error);
    if error.is_null() {
        return Ok(value);
    }
    // SAFETY: LevelDB returns a malloc-allocated, NUL-terminated error string.
    unsafe {
        let message = CStr::from_ptr(error).to_string_lossy().into_owned();
        ffi::leveldb_free(error.cast());
        bail!("{message}");
    }
}

fn validate_copy(path: &Path) -> Result<()> {
    let current = fs::read_to_string(path.join("CURRENT"))?;
    let manifest = current.trim_end_matches('\n');
    validate_log(File::open(path.join(manifest))?).context("manifest journal")?;
    for entry in fs::read_dir(path)? {
        let file = entry?.path();
        match file.extension().and_then(|extension| extension.to_str()) {
            Some("log") => validate_log(File::open(file)?).context("write-ahead log")?,
            Some("ldb" | "sst") => {
                validate_table(File::open(file)?).context("table footer and metadata")?
            }
            _ => {}
        }
    }
    Ok(())
}

// https://github.com/google/leveldb/blob/1.22/doc/log_format.md
const LOG_BLOCK_SIZE: usize = 32 * 1024;
const LOG_HEADER_SIZE: usize = 7;

fn validate_log(mut file: File) -> Result<()> {
    // Native recovery treats a torn final record as EOF. This input is a closed profile.
    let mut fragmented = false;
    let mut block = Vec::with_capacity(LOG_BLOCK_SIZE);
    loop {
        block.clear();
        let size = file
            .by_ref()
            .take(LOG_BLOCK_SIZE as u64)
            .read_to_end(&mut block)?;
        let mut records = block.as_slice();
        while let Some((header, body)) = records.split_at_checked(LOG_HEADER_SIZE) {
            let [a, b, c, d, low, high, kind] = <[u8; LOG_HEADER_SIZE]>::try_from(header)?;
            if header == [0; LOG_HEADER_SIZE] {
                break;
            }
            let length = usize::from(u16::from_le_bytes([low, high]));
            let (data, rest) = body
                .split_at_checked(length)
                .context("truncated record payload")?;
            let checksum = crc32c::crc32c_append(crc32c::crc32c(&[kind]), data);
            ensure!(
                masked_crc(checksum) == u32::from_le_bytes([a, b, c, d]),
                "record checksum mismatch"
            );
            fragmented = match (fragmented, kind) {
                (false, 1) | (true, 4) => false,
                (false, 2) | (true, 3) => true,
                _ => bail!("invalid record fragment sequence"),
            };
            records = rest;
        }
        ensure!(
            records.iter().all(|&byte| byte == 0),
            "nonzero journal trailer or truncated record header"
        );
        if size < LOG_BLOCK_SIZE {
            ensure!(!fragmented, "truncated fragmented record");
            return Ok(());
        }
    }
}

// https://github.com/google/leveldb/blob/1.22/doc/table_format.md
const TABLE_FOOTER_SIZE: usize = 48;
const TABLE_MAGIC: u64 = 0xdb4775248b80fb57;
const TABLE_BLOCK_TRAILER_SIZE: u64 = 5;

fn validate_table(mut file: File) -> Result<()> {
    let footer_offset = file.seek(SeekFrom::End(-(TABLE_FOOTER_SIZE as i64)))?;
    let mut footer = [0; TABLE_FOOTER_SIZE];
    file.read_exact(&mut footer)?;
    let (mut handles, magic) = footer.split_at(TABLE_FOOTER_SIZE - size_of::<u64>());
    ensure!(magic == TABLE_MAGIC.to_le_bytes(), "invalid table magic");
    // Footer handles have no checksum. Validate their bounds before native LevelDB allocates
    // a block buffer. LevelDB writes the metaindex and index back to back before the footer.
    let metadata_offset = read_varint(&mut handles)?;
    let metadata_size = read_varint(&mut handles)?;
    let index_offset = read_varint(&mut handles)?;
    let index_size = read_varint(&mut handles)?;
    ensure!(
        block_end(metadata_offset, metadata_size) == Some(index_offset)
            && block_end(index_offset, index_size) == Some(footer_offset),
        "invalid table block bounds"
    );
    verify_metadata_block(&mut file, metadata_offset, metadata_size)?;
    verify_metadata_block(&mut file, index_offset, index_size)?;
    Ok(())
}

fn read_varint(bytes: &mut &[u8]) -> Result<u64> {
    let mut value = 0;
    for shift in (0..u64::BITS).step_by(7) {
        let (&byte, rest) = bytes.split_first().context("truncated block handle")?;
        *bytes = rest;
        let digit = u64::from(byte & 0x7f);
        ensure!(digit <= (u64::MAX >> shift), "block handle overflow");
        value |= digit << shift;
        if byte < 0x80 {
            return Ok(value);
        }
    }
    bail!("unterminated block handle")
}

fn block_end(offset: u64, size: u64) -> Option<u64> {
    offset
        .checked_add(size)?
        .checked_add(TABLE_BLOCK_TRAILER_SIZE)
}

fn verify_metadata_block(file: &mut File, offset: u64, size: u64) -> Result<()> {
    file.seek(SeekFrom::Start(offset))?;
    let mut reader = Crc32cReader::new((&mut *file).take(size + 1));
    ensure!(
        io::copy(&mut reader, &mut io::sink())? == size + 1,
        "truncated table block"
    );
    let checksum = reader.crc32c();
    let mut stored = [0; 4];
    file.read_exact(&mut stored)?;
    ensure!(
        masked_crc(checksum) == u32::from_le_bytes(stored),
        "block checksum mismatch"
    );
    Ok(())
}

fn masked_crc(checksum: u32) -> u32 {
    // LevelDB's CRC mask is part of the on-disk format (util/crc32c.h).
    checksum.rotate_right(15).wrapping_add(0xa282_ead8)
}
