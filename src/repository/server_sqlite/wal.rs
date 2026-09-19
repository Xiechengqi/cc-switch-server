use std::fs;
use std::io::{BufReader, Read};
use std::path::Path;

use anyhow::{ensure, Context};

/// SQLite treats an invalid WAL tail as an incomplete crash write and may
/// silently ignore it. At an authority/restore boundary every durable frame is
/// part of the repository, so validate the complete envelope and rolling
/// checksums before allowing SQLite to recover it.
pub(super) fn validate_wal_if_present(database: &Path) -> anyhow::Result<()> {
    let wal_path = database.with_file_name(format!(
        "{}-wal",
        database
            .file_name()
            .and_then(|value| value.to_str())
            .context("SQLite database filename must be UTF-8")?
    ));
    let file = match fs::File::open(&wal_path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("open SQLite WAL {}", wal_path.display()))
        }
    };
    let length = file
        .metadata()
        .with_context(|| format!("stat SQLite WAL {}", wal_path.display()))?
        .len();
    if length == 0 {
        return Ok(());
    }
    ensure!(length >= 32, "SQLite WAL is truncated before its header");
    let mut reader = BufReader::new(file);
    let mut header = [0_u8; 32];
    reader
        .read_exact(&mut header)
        .with_context(|| format!("read SQLite WAL header {}", wal_path.display()))?;
    let magic = u32::from_be_bytes(header[0..4].try_into()?);
    let checksum_little_endian = match magic {
        0x377f_0682 => true,
        0x377f_0683 => false,
        _ => anyhow::bail!("SQLite WAL has an invalid magic value"),
    };
    ensure!(
        u32::from_be_bytes(header[4..8].try_into()?) == 3_007_000,
        "SQLite WAL format version is unsupported"
    );
    let encoded_page_size = u32::from_be_bytes(header[8..12].try_into()?);
    let page_size = if encoded_page_size == 1 {
        65_536_u32
    } else {
        encoded_page_size
    };
    ensure!(
        (512..=65_536).contains(&page_size) && page_size.is_power_of_two(),
        "SQLite WAL page size is invalid"
    );
    let frame_size = 24_u64.saturating_add(u64::from(page_size));
    ensure!(
        (length - 32) % frame_size == 0,
        "SQLite WAL ends with a partial frame"
    );

    let mut checksum = wal_checksum(checksum_little_endian, [0, 0], &header[..24])?;
    let expected_header = [
        u32::from_be_bytes(header[24..28].try_into()?),
        u32::from_be_bytes(header[28..32].try_into()?),
    ];
    ensure!(
        checksum == expected_header,
        "SQLite WAL header checksum mismatch"
    );
    let salt = &header[16..24];
    let mut frame = vec![0_u8; usize::try_from(frame_size)?];
    let frame_count = (length - 32) / frame_size;
    for index in 0..frame_count {
        reader
            .read_exact(&mut frame)
            .with_context(|| format!("read SQLite WAL frame {}", index + 1))?;
        ensure!(
            frame[8..16] == salt[..],
            "SQLite WAL frame salt mismatch at frame {}",
            index + 1
        );
        checksum = wal_checksum(checksum_little_endian, checksum, &frame[..8])?;
        checksum = wal_checksum(checksum_little_endian, checksum, &frame[24..])?;
        let expected = [
            u32::from_be_bytes(frame[16..20].try_into()?),
            u32::from_be_bytes(frame[20..24].try_into()?),
        ];
        ensure!(
            checksum == expected,
            "SQLite WAL checksum mismatch at frame {}",
            index + 1
        );
    }
    Ok(())
}

fn wal_checksum(
    little_endian: bool,
    mut state: [u32; 2],
    bytes: &[u8],
) -> anyhow::Result<[u32; 2]> {
    ensure!(
        bytes.len().is_multiple_of(8),
        "SQLite WAL checksum input is misaligned"
    );
    for pair in bytes.chunks_exact(8) {
        let first = if little_endian {
            u32::from_le_bytes(pair[..4].try_into()?)
        } else {
            u32::from_be_bytes(pair[..4].try_into()?)
        };
        let second = if little_endian {
            u32::from_le_bytes(pair[4..].try_into()?)
        } else {
            u32::from_be_bytes(pair[4..].try_into()?)
        };
        state[0] = state[0].wrapping_add(first).wrapping_add(state[1]);
        state[1] = state[1].wrapping_add(second).wrapping_add(state[0]);
    }
    Ok(state)
}
