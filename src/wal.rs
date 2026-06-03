use crate::device::{BlockDevice, BlockDeviceError};
use crate::disk::{AlignedBlock, ChaosDisk};

#[repr(C)]
pub struct LogHeader {
    pub magic: [u8; 4],       // "A016"
    pub checksum: u32,        // CRC32 or similar of header + payload
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
    /// The current block we are writing to
    pub current_log_block: u64,
    /// Total number of blocks allocated for the circular log
    pub max_log_blocks: u64,
}

impl WalManager {
    pub fn new(device: ChaosDisk, log_start_block: u64, max_log_blocks: u64) -> Self {
        WalManager {
            device,
            next_sequence_id: 1,
            log_start_block,
            current_log_block: 0,
            max_log_blocks,
        }
    }

    pub fn serialize_frame(
        &self,
        sequence_id: u64,
        target_block_id: u64,
        payload: &[u8],
    ) -> AlignedBlock {
        assert!(
            payload.len() <= 4096 - 32,
            "Payload size {} exceeds maximum available space in a 4KB block",
            payload.len()
        );
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

        frame
    }

    /// Computes a simple additive checksum over the 4KB block to validate integrity.
    fn compute_block_checksum(&self, data: &[u8; 4096]) -> u32 {
        data.chunks_exact(4)
            .map(|chunk| u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .fold(0u32, |acc, val| acc.wrapping_add(val))
    }

    pub fn append_log_entry(
        &mut self,
        target_block_id: u64,
        payload: &[u8],
    ) -> Result<(), BlockDeviceError> {
        let physical_block_id = self.log_start_block + self.current_log_block;
        let frame = self.serialize_frame(self.next_sequence_id, target_block_id, payload);

        self.device.write_block(physical_block_id, &frame.data)?;
        // Critical: Flush the log to stable storage so it survives a crash
        self.device.flush()?;

        self.next_sequence_id += 1;
        self.current_log_block = (self.current_log_block + 1) % self.max_log_blocks;

        Ok(())
    }

    /// Scans the WAL partition, validates integrity, and replays entries in chronological order.
    pub fn recover(&mut self) -> Result<(), BlockDeviceError> {
        let mut valid_entries = Vec::new();
        let mut scratch_pad = AlignedBlock::new();

        println!("Executing low-level system recovery protocol...");

        // 1. Linear scan across the designated log partition zone
        for relative_idx in 0..self.max_log_blocks {
            let physical_block_id = self.log_start_block + relative_idx;

            // Read raw block into RAM workspace
            if let Err(e) = self
                .device
                .read_block(physical_block_id, &mut scratch_pad.data)
            {
                println!(
                    "Hardware read failure at block {}: {}. Stopping scan.",
                    physical_block_id, e
                );
                break;
            }

            // Extract Magic Signature
            if &scratch_pad.data[0..4] != b"A016" {
                continue; // Skip uninitialized or non-log space
            }

            // Extract Stored Checksum
            let stored_checksum = u32::from_be_bytes(scratch_pad.data[4..8].try_into().unwrap());

            // Reset checksum field to zero inline for verification (matching serialization logic)
            scratch_pad.data[4..8].copy_from_slice(&[0u8; 4]);

            // Execute Integrity Check
            let computed_checksum = self.compute_block_checksum(&scratch_pad.data);
            if stored_checksum != computed_checksum {
                println!(
                    "Checksum Mismatch at physical sector {}! Discarding torn frame.",
                    physical_block_id
                );
                // In an append-only timeline, a checksum failure marks the crash point.
                // We must break here because subsequent blocks contain stale data
                // from a previous wrap-around of the circular log.
                break;
            }

            // Unpack metadata
            let seq_id = u64::from_be_bytes(scratch_pad.data[8..16].try_into().unwrap());
            let target_block_id = u64::from_be_bytes(scratch_pad.data[16..24].try_into().unwrap());
            let payload_size =
                u32::from_be_bytes(scratch_pad.data[24..28].try_into().unwrap()) as usize;

            // Store for sorting (Sequence ID, Target Block, Payload, Physical Index)
            let mut payload = vec![0u8; payload_size];
            payload.copy_from_slice(&scratch_pad.data[32..32 + payload_size]);

            valid_entries.push((seq_id, target_block_id, payload, relative_idx));
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
            self.current_log_block = (last_physical_idx + 1) % self.max_log_blocks;
        }

        println!(
            "System recovery complete. Reality restored to sequence: {}",
            highest_valid_seq
        );
        Ok(())
    }

    pub fn checkpoint(&mut self) -> Result<(), BlockDeviceError> {
        let mut valid_entries = Vec::new();
        let mut scratch_pad = AlignedBlock::new();

        // 1. Linear scan across the log partition to collect all pending updates
        for relative_idx in 0..self.max_log_blocks {
            let physical_block_id = self.log_start_block + relative_idx;
            self.device
                .read_block(physical_block_id, &mut scratch_pad.data)?;

            // Extract Magic Signature
            if &scratch_pad.data[0..4] != b"A016" {
                continue; // Skip empty space
            }

            // Extract and verify checksum (zeroing the slot inline for the hash check)
            let stored_checksum = u32::from_be_bytes(scratch_pad.data[4..8].try_into().unwrap());
            scratch_pad.data[4..8].copy_from_slice(&[0u8; 4]);

            if stored_checksum != self.compute_block_checksum(&scratch_pad.data) {
                // A checksum failure in an append-only timeline marks the crash point.
                break;
            }

            let seq_id = u64::from_be_bytes(scratch_pad.data[8..16].try_into().unwrap());
            let target_id = u64::from_be_bytes(scratch_pad.data[16..24].try_into().unwrap());
            let payload_size =
                u32::from_be_bytes(scratch_pad.data[24..28].try_into().unwrap()) as usize;

            let mut payload = vec![0u8; payload_size];
            payload.copy_from_slice(&scratch_pad.data[32..32 + payload_size]);
            valid_entries.push((seq_id, target_id, payload));
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
        self.current_log_block = 0;
        Ok(())
    }
}
