use std::fs::OpenOptions;
use std::io;
use std::path::Path;
use std::sync::Arc;

pub struct SparseFile {
    file: Arc<std::fs::File>,
}

impl SparseFile {
    /// Creates or opens a file and sets its total length.
    /// If supported by the underlying filesystem (e.g. NTFS on Windows), marks the file as sparse.
    pub fn create_preallocated(path: &Path, total_size: u64) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;

        // Set total length to pre-allocate
        if total_size > 0 {
            file.set_len(total_size)?;
        }

        // On Windows NTFS, enable sparse file flag for zero-delay instant allocation
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::System::IO::DeviceIoControl;

            const FSCTL_SET_SPARSE: u32 = 0x000900C4;
            let handle = file.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE;
            let mut bytes_returned: u32 = 0;

            unsafe {
                DeviceIoControl(
                    handle,
                    FSCTL_SET_SPARSE,
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    0,
                    &mut bytes_returned,
                    std::ptr::null_mut(),
                );
            }
        }

        Ok(Self {
            file: Arc::new(file),
        })
    }

    /// Open an existing file for resuming
    pub fn open_existing(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)?;

        Ok(Self {
            file: Arc::new(file),
        })
    }

    /// Directly write bytes at a given offset concurrently without modifying a shared seek pointer.
    pub fn write_at(&self, offset: u64, data: &[u8]) -> io::Result<usize> {
        #[cfg(windows)]
        {
            use std::os::windows::fs::FileExt;
            self.file.seek_write(data, offset)
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::FileExt;
            self.file.write_at(data, offset)
        }
    }

    pub fn sync_all(&self) -> io::Result<()> {
        self.file.sync_all()
    }

    pub fn get_arc_handle(&self) -> Arc<std::fs::File> {
        Arc::clone(&self.file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn test_sparse_file_concurrent_writes() {
        let temp_dir = std::env::temp_dir();
        let test_path = temp_dir.join("octoheft_test_file.bin");

        let total_size = 1024 * 1024; // 1 MB
        let sparse = SparseFile::create_preallocated(&test_path, total_size).unwrap();

        // Write chunk at offset 0
        sparse.write_at(0, b"HEADER_CHUNK").unwrap();
        // Write chunk at offset 500,000
        sparse.write_at(500_000, b"MIDDLE_CHUNK").unwrap();
        // Write chunk at offset 1,000,000
        sparse.write_at(1_000_000, b"TAIL_CHUNK").unwrap();

        sparse.sync_all().unwrap();

        let mut read_file = std::fs::File::open(&test_path).unwrap();
        assert_eq!(read_file.metadata().unwrap().len(), total_size);

        let mut buf_head = [0u8; 12];
        read_file.read_exact(&mut buf_head).unwrap();
        assert_eq!(&buf_head, b"HEADER_CHUNK");

        let _ = std::fs::remove_file(&test_path);
    }
}
