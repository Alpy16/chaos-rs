use crate::device::{BlockDevice, BlockDeviceError};
use crate::disk::{AlignedBlock, ChaosDisk};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecoveryReport {
    pub last_sequence: u64,
    pub corruption_detected: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalError {
    CorruptFrame,
    InvalidMagic,
    ChecksumMismatch,
    InvalidPayloadSize,
    Device(BlockDeviceError),
}

impl std::fmt::Display for WalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WalError::CorruptFrame => write!(f, "WAL Error: Corrupt frame"),
            WalError::InvalidMagic => write!(f, "WAL Error: Invalid magic signature"),
            WalError::ChecksumMismatch => write!(f, "WAL Error: Checksum mismatch"),
            WalError::InvalidPayloadSize => write!(f, "WAL Error: Invalid payload size"),
            WalError::Device(e) => write!(f, "WAL Error: Device failure: {}", e),
        }
    }
}

impl std::error::Error for WalError {}

impl From<BlockDeviceError> for WalError {
    fn from(err: BlockDeviceError) -> Self {
        WalError::Device(err)
    }
}

#[repr(C)]
pub struct LogHeader {
    pub magic: [u8; 4],       // "A016"
    pub checksum: u32,        // CRC32 checksum of the entire block
    pub sequence_id: u64,     // Monotonic ID for log ordering
    pub target_block_id: u64, // The physical location this log entry updates
    pub payload_size: u32,    // Length of the data following this header
    pub _reserved: u32,       // Explicit padding to ensure 32-byte alignment
}

pub struct WalManager {
    pub device: ChaosDisk,
    pub next_sequence_id: u64,
    /// The first block ID allocated for the WAL area
    pub log_start_block: u64,
    /// The current index within the WAL ring we are writing to
    pub current_log_index: u64,
    /// Total number of blocks allocated for the circular log
    pub max_log_blocks: u64,
}

impl WalManager {
    pub fn new(device: ChaosDisk, log_start_block: u64, max_log_blocks: u64) -> Self {
        WalManager {
            device,
            next_sequence_id: 1,
            log_start_block,
            current_log_index: 0,
            max_log_blocks,
        }
    }

    pub fn serialize_frame(
        &self,
        sequence_id: u64,
        target_block_id: u64,
        payload: &[u8],
    ) -> Result<AlignedBlock, WalError> {
        if payload.len() > 4096 - 32 {
            return Err(WalError::InvalidPayloadSize);
        }
        let mut frame = AlignedBlock::new();
        let header = LogHeader {
            magic: *b"A016",
            checksum: 0, // To be calculated later
            sequence_id,
            target_block_id,
            payload_size: payload.len() as u32,
            _reserved: 0,
        };

        // Copy header fields into the frame
        frame.data[0..4].copy_from_slice(&header.magic);
        frame.data[4..8].copy_from_slice(&header.checksum.to_be_bytes());
        frame.data[8..16].copy_from_slice(&header.sequence_id.to_be_bytes());
        frame.data[16..24].copy_from_slice(&header.target_block_id.to_be_bytes());
        frame.data[24..28].copy_from_slice(&header.payload_size.to_be_bytes());

        // Payload starts immediately after the 32-byte header
        frame.data[32..32 + payload.len()].copy_from_slice(payload);

        // Compute the checksum over the entire block.
        // The bytes at 4..8 are currently 0, which correctly treats the checksum slot
        // as zero during calculation.
        let checksum = self.compute_block_checksum(&frame.data);
        frame.data[4..8].copy_from_slice(&checksum.to_be_bytes());

        Ok(frame)
    }

    /// Internal helper to validate and unpack a log frame.
    fn decode_frame(&self, data: &[u8; 4096]) -> Result<(u64, u64, Vec<u8>), WalError> {
        if &data[0..4] != b"A016" {
            return Err(WalError::InvalidMagic);
        }

        let stored_checksum = u32::from_be_bytes(data[4..8].try_into().unwrap());
        let mut validation_buffer = *data;
        validation_buffer[4..8].copy_from_slice(&[0u8; 4]);

        if stored_checksum != self.compute_block_checksum(&validation_buffer) {
            return Err(WalError::ChecksumMismatch);
        }

        let seq_id = u64::from_be_bytes(data[8..16].try_into().unwrap());
        let target_id = u64::from_be_bytes(data[16..24].try_into().unwrap());
        let payload_size = u32::from_be_bytes(data[24..28].try_into().unwrap()) as usize;

        if payload_size > 4096 - 32 {
            return Err(WalError::InvalidPayloadSize);
        }

        let mut payload = data[32..32 + payload_size].to_vec();
        Ok((seq_id, target_id, payload))
    }

    /// Computes standard CRC-32 checksum over the 4KB block to validate integrity.
    fn compute_block_checksum(&self, data: &[u8; 4096]) -> u32 {
        let mut crc = 0xFFFFFFFFu32;
        for &byte in data {
            crc ^= byte as u32;
            for _ in 0..8 {
                if crc & 1 != 0 {
                    crc = (crc >> 1) ^ 0xEDB88320;
                } else {
                    crc >>= 1;
                }
            }
        }
        !crc
    }

    pub fn append_log_entry(
        &mut self,
        target_block_id: u64,
        payload: &[u8],
    ) -> Result<(), WalError> {
        let physical_block_id = self.log_start_block + self.current_log_index;
        let frame = self.serialize_frame(self.next_sequence_id, target_block_id, payload)?;

        self.device.write_block(physical_block_id, &frame.data)?;
        // Critical: Flush the log to stable storage so it survives a crash
        self.device.flush()?;

        self.next_sequence_id += 1;
        self.current_log_index = (self.current_log_index + 1) % self.max_log_blocks;

        Ok(())
    }

    /// Scans the WAL partition, validates integrity, and replays entries in chronological order.
    pub fn recover(&mut self) -> Result<RecoveryReport, WalError> {
        let mut valid_entries = Vec::new();
        let mut scratch_pad = AlignedBlock::new();
        let mut corruption_detected = false;

        // 1. Linear scan across the designated log partition zone
        for relative_idx in 0..self.max_log_blocks {
            let physical_block_id = self.log_start_block + relative_idx;

            // Read raw block into RAM workspace
            self.device
                .read_block(physical_block_id, &mut scratch_pad.data)?;

            match self.decode_frame(&scratch_pad.data) {
                Ok((seq_id, target_id, payload)) => {
                    valid_entries.push((seq_id, target_id, payload, relative_idx));
                }
                Err(WalError::InvalidMagic) if scratch_pad.data.iter().all(|&b| b == 0) => {
                    continue; // Skip uninitialized space
                }
                Err(_) => {
                    corruption_detected = true;
                    break; // Recovery boundary reached (Torn write or bit rot)
                }
            }
        }

        // 2. Sort entries by Sequence ID to handle circular wrap-around correctly
        valid_entries.sort_by_key(|entry| entry.0);

        let mut highest_valid_seq = 0;
        let mut last_physical_idx = 0;

        // 3. Log Replay: Apply updates to stable storage in the correct order
        for (seq_id, target_id, payload, rel_idx) in valid_entries {
            let mut db_write_buffer = AlignedBlock::new();
            db_write_buffer.data[..payload.len()].copy_from_slice(&payload);

            self.device.write_block(target_id, &db_write_buffer.data)?;

            highest_valid_seq = seq_id;
            last_physical_idx = rel_idx;
        }

        // 4. Finalize durability
        self.device.flush()?;

        // 5. Sync WalManager state
        if highest_valid_seq > 0 {
            self.next_sequence_id = highest_valid_seq + 1;
            self.current_log_index = (last_physical_idx + 1) % self.max_log_blocks;
        }

        Ok(RecoveryReport {
            last_sequence: highest_valid_seq,
            corruption_detected,
        })
    }

    pub fn checkpoint(&mut self) -> Result<(), WalError> {
        let mut valid_entries = Vec::new();
        let mut scratch_pad = AlignedBlock::new();

        // 1. Linear scan across the log partition to collect all pending updates
        for relative_idx in 0..self.max_log_blocks {
            let physical_block_id = self.log_start_block + relative_idx;
            self.device
                .read_block(physical_block_id, &mut scratch_pad.data)?;

            match self.decode_frame(&scratch_pad.data) {
                Ok((seq_id, target_id, payload)) => {
                    valid_entries.push((seq_id, target_id, payload));
                }
                Err(WalError::InvalidMagic) if scratch_pad.data.iter().all(|&b| b == 0) => {
                    continue;
                }
                Err(e) => return Err(e),
            }
        }

        // 2. Sort entries chronologically to handle circular buffer wrap-around
        valid_entries.sort_by_key(|entry| entry.0);

        // 3. Replay to Main Storage: Move validated updates to their home on disk
        for (_, target_id, payload) in valid_entries {
            let mut db_buffer = AlignedBlock::new();
            db_buffer.data[..payload.len()].copy_from_slice(&payload);
            self.device.write_block(target_id, &db_buffer.data)?;
        }
        self.device.flush()?;

        // 4. Clear the Log: Wipe headers to ensure future recoveries don't replay stale data
        let empty = AlignedBlock::new();
        for i in 0..self.max_log_blocks {
            self.device
                .write_block(self.log_start_block + i, &empty.data)?;
        }
        self.device.flush()?;

        // 5. Reset the write head to the beginning of the partition
        self.current_log_index = 0;
        Ok(())
    }
}
