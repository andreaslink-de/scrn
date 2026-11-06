//! A minimal writer for uncompressed tar archives (POSIX ustar), enough
//! for the backup download: regular files with short names. PNG files are
//! compressed already, so the archive is not compressed again, and no
//! extra crate is needed.

/// Size of a header and of the blocks file data is padded to.
pub const BLOCK: u64 = 512;
/// Two zero blocks end an archive.
pub const END: [u8; 1024] = [0; 1024];

/// Longest name that fits the ustar name field without a prefix.
const MAX_NAME: usize = 100;
/// Largest size the 11 octal digits of the size field can hold (8 GiB).
const MAX_SIZE: u64 = 0o77_777_777_777;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TarError {
    #[error("file name {0:?} does not fit into a tar header")]
    Name(String),
    #[error("file {0:?} is too large for a tar archive")]
    Size(String),
}

/// The header of a regular file with mode 0644, owned by root.
pub fn header(name: &str, size: u64, mtime: i64) -> Result<[u8; 512], TarError> {
    if name.is_empty() || name.len() > MAX_NAME || !name.is_ascii() {
        return Err(TarError::Name(name.to_owned()));
    }
    if size > MAX_SIZE {
        return Err(TarError::Size(name.to_owned()));
    }
    let mut h = [0u8; 512];
    h[..name.len()].copy_from_slice(name.as_bytes());
    octal(&mut h[100..108], 0o644);
    octal(&mut h[108..116], 0);
    octal(&mut h[116..124], 0);
    octal(&mut h[124..136], size);
    octal(&mut h[136..148], mtime.max(0).cast_unsigned());
    h[156] = b'0';
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");
    // The checksum is computed with its own field filled with spaces.
    h[148..156].fill(b' ');
    let sum: u64 = h.iter().map(|b| u64::from(*b)).sum();
    octal(&mut h[148..155], sum);
    Ok(h)
}

/// Zero bytes that follow `size` bytes of file data.
pub fn padding(size: u64) -> u64 {
    (BLOCK - size % BLOCK) % BLOCK
}

/// Bytes a file takes in the archive: header, data and padding.
pub fn entry_len(size: u64) -> u64 {
    BLOCK + size + padding(size)
}

/// Writes `value` as zero-padded octal digits followed by a NUL byte.
fn octal(field: &mut [u8], value: u64) {
    let digits = field.len() - 1;
    let text = format!("{value:0digits$o}");
    field[..digits].copy_from_slice(&text.as_bytes()[text.len() - digits..]);
    field[digits] = 0;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(h: &[u8], range: std::ops::Range<usize>) -> &str {
        std::str::from_utf8(&h[range])
            .unwrap()
            .trim_end_matches('\0')
    }

    #[test]
    fn header_fields() {
        let h = header("files/1/a.png", 1234, 1_700_000_000).unwrap();
        assert_eq!(field(&h, 0..100), "files/1/a.png");
        assert_eq!(field(&h, 100..108), "0000644");
        assert_eq!(field(&h, 124..136), format!("{:011o}", 1234));
        assert_eq!(field(&h, 136..148), format!("{:011o}", 1_700_000_000));
        assert_eq!(h[156], b'0');
        assert_eq!(&h[257..265], b"ustar\x0000");
        // The checksum is the byte sum with the field counted as spaces.
        let mut copy = h;
        copy[148..156].fill(b' ');
        let sum: u64 = copy.iter().map(|b| u64::from(*b)).sum();
        assert_eq!(field(&h, 148..155), format!("{sum:06o}"));
        assert_eq!(h[155], b' ');
    }

    #[test]
    fn names_and_sizes_are_checked() {
        assert!(header(&"a".repeat(100), 0, 0).is_ok());
        assert!(header(&"a".repeat(101), 0, 0).is_err());
        assert!(header("", 0, 0).is_err());
        assert!(header("bild-ä.png", 0, 0).is_err());
        assert!(header("a", MAX_SIZE, 0).is_ok());
        assert_eq!(
            header("a", MAX_SIZE + 1, 0),
            Err(TarError::Size("a".to_owned()))
        );
    }

    #[test]
    fn padding_to_blocks() {
        assert_eq!(padding(0), 0);
        assert_eq!(padding(1), 511);
        assert_eq!(padding(512), 0);
        assert_eq!(padding(513), 511);
        assert_eq!(entry_len(0), 512);
        assert_eq!(entry_len(600), 512 + 1024);
    }
}
