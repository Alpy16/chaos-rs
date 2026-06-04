use chaos_rs::{AdminControls, AlignedBlock, BlockDevice, ChaosDisk, WalError, WalManager};

#[test]
fn serialize_frame_rejects_oversized_payload() {
    let disk = ChaosDisk::new(16);
    let wal = WalManager::new(disk, 8, 4);

    // Max payload is 4096 - 32 = 4064 bytes.
    let oversized_payload = vec![0u8; 4065];
    let result = wal.serialize_frame(1, 0, &oversized_payload);

    assert!(matches!(result, Err(WalError::InvalidPayloadSize)));
}

#[test]
fn recover_ignores_empty_log_blocks() {
    let disk = ChaosDisk::new(16);
    let mut wal = WalManager::new(disk, 8, 4);

    let report = wal.recover().unwrap();
    assert_eq!(report.last_sequence, 0);
    assert!(!report.corruption_detected);
}

#[test]
fn recover_replays_valid_log_entry() {
    // Create a disk with 16 blocks.
    //
    // For this test:
    // - block 0 will be the target "database" block
    // - blocks 8..12 will act as the WAL region
    let disk = ChaosDisk::new(16);

    // Create a WAL manager over the disk.
    // WAL starts at physical block 8 and has 4 blocks of circular log space.
    let mut wal = WalManager::new(disk, 8, 4);

    // This is the logical database payload we want to protect through the WAL.
    let payload = b"record-1";

    // Append a WAL entry saying:
    // "during recovery, write this payload to target block 0."
    //
    // append_log_entry writes the WAL frame and flushes it to stable storage.
    wal.append_log_entry(0, payload).unwrap();

    // Run recovery.
    // Recovery scans the WAL region, validates CRC-32, sorts by sequence,
    // and replays valid entries to their target blocks.
    let report = wal.recover().unwrap();

    // The valid WAL frame should have sequence ID 1.
    assert_eq!(report.last_sequence, 1);

    // There should be no corruption boundary in this simple clean case.
    assert!(!report.corruption_detected);

    // Read the target database block after recovery.
    let mut read_buffer = AlignedBlock::new();
    wal.device.read_block(0, &mut read_buffer.data).unwrap();

    // The recovered payload should now exist at the target block.
    assert_eq!(&read_buffer.data[0..payload.len()], payload);
}

#[test]
fn recover_replays_multiple_entries_in_order() {
    let disk = ChaosDisk::new(16);
    let mut wal = WalManager::new(disk, 8, 4);

    // We write multiple updates to the SAME block to prove that
    // recovery replays entries in the correct chronological sequence.
    wal.append_log_entry(0, b"old-data").unwrap(); // seq 1
    wal.append_log_entry(0, b"new-data").unwrap(); // seq 2

    wal.device.crash().unwrap();
    wal.device.reboot().unwrap();

    wal.recover().unwrap();

    let mut rb = AlignedBlock::new();
    wal.device.read_block(0, &mut rb.data).unwrap();
    // The database block should contain the data from the LATEST sequence.
    assert_eq!(&rb.data[0..8], b"new-data");
}

#[test]
fn recover_updates_sequence_state() {
    let disk = ChaosDisk::new(16);
    let mut wal = WalManager::new(disk, 8, 4);

    wal.append_log_entry(0, b"a").unwrap();
    wal.append_log_entry(0, b"b").unwrap();
    wal.append_log_entry(0, b"c").unwrap();

    wal.device.crash().unwrap();
    wal.device.reboot().unwrap();

    wal.recover().unwrap();
    assert_eq!(wal.next_sequence_id, 4);
}

#[test]
fn recover_updates_log_index_state() {
    let disk = ChaosDisk::new(16);
    let mut wal = WalManager::new(disk, 8, 4);

    wal.append_log_entry(0, b"a").unwrap();
    wal.append_log_entry(0, b"b").unwrap();

    wal.device.crash().unwrap();
    wal.device.reboot().unwrap();

    wal.recover().unwrap();
    // Entries were at indices 0, 1. Next should be 2.
    assert_eq!(wal.current_log_index, 2);
}

#[test]
fn recover_stops_at_checksum_boundary_and_replays_valid_prefix() {
    let disk = ChaosDisk::new(16);
    let mut wal = WalManager::new(disk, 8, 4);

    wal.append_log_entry(0, b"valid-1").unwrap();
    wal.append_log_entry(1, b"valid-2").unwrap();

    // Manually write a corrupted block at log index 2 (physical block 10).
    let mut corrupt_block = AlignedBlock::new();
    corrupt_block.data[0..4].copy_from_slice(b"A016");
    // Junk checksum.
    corrupt_block.data[4..8].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
    wal.device.write_block(10, &corrupt_block.data).unwrap();
    wal.device.flush().unwrap();

    wal.device.crash().unwrap();
    wal.device.reboot().unwrap();

    let report = wal.recover().unwrap();

    // The valid prefix (seq 1 and 2) should replay.
    assert_eq!(report.last_sequence, 2);
    assert!(report.corruption_detected);

    let mut rb = AlignedBlock::new();
    wal.device.read_block(0, &mut rb.data).unwrap();
    assert_eq!(&rb.data[0..7], b"valid-1");
    wal.device.read_block(1, &mut rb.data).unwrap();
    assert_eq!(&rb.data[0..7], b"valid-2");
}

#[test]
fn recover_stops_at_invalid_magic_boundary() {
    let disk = ChaosDisk::new(16);
    let mut wal = WalManager::new(disk, 8, 4);

    wal.append_log_entry(0, b"valid").unwrap();

    // Write block with bad magic at log index 1 (physical block 9).
    let mut bad_magic = AlignedBlock::new();
    bad_magic.data[0..4].copy_from_slice(b"BAD!");
    wal.device.write_block(9, &bad_magic.data).unwrap();
    wal.device.flush().unwrap();

    wal.device.crash().unwrap();
    wal.device.reboot().unwrap();

    let report = wal.recover().unwrap();
    assert!(report.corruption_detected);
    assert_eq!(report.last_sequence, 1);
}

#[test]
fn recover_stops_at_invalid_payload_size_boundary() {
    let disk = ChaosDisk::new(16);
    let mut wal = WalManager::new(disk, 8, 4);

    wal.append_log_entry(0, b"valid").unwrap();

    // Create a frame that is valid EXCEPT for payload size.
    // We'll mutate the size after serialization and then re-calculate the checksum
    // to ensure it hits the payload bounds check rather than the checksum check.
    let mut frame = wal.serialize_frame(2, 1, b"dummy").unwrap();

    // Mutate payload size to be oversized.
    frame.data[24..28].copy_from_slice(&5000u32.to_be_bytes());

    // Re-calculate checksum to pass the first integrity check.
    frame.data[4..8].copy_from_slice(&[0u8; 4]);
    let mut crc = 0xFFFFFFFFu32;
    for &byte in frame.data.iter() {
        crc ^= byte as u32;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB88320;
            } else {
                crc >>= 1;
            }
        }
    }
    let checksum = !crc;
    frame.data[4..8].copy_from_slice(&checksum.to_be_bytes());

    wal.device.write_block(9, &frame.data).unwrap();
    wal.device.flush().unwrap();

    wal.device.crash().unwrap();
    wal.device.reboot().unwrap();

    let report = wal.recover().unwrap();
    assert!(report.corruption_detected);
    assert_eq!(report.last_sequence, 1);
}

#[test]
fn recover_propagates_device_errors() {
    let disk = ChaosDisk::new(16);
    let mut wal = WalManager::new(disk, 8, 4);

    wal.device.crash().unwrap();
    // Device is now frozen. All I/O should fail.

    let result = wal.recover();
    assert!(matches!(result, Err(WalError::Device(_))));
}

#[test]
fn recover_handles_circular_log_wraparound() {
    let disk = ChaosDisk::new(16);
    // 2 blocks starting at physical block 8.
    let mut wal = WalManager::new(disk, 8, 2);

    wal.append_log_entry(0, b"first").unwrap(); // seq 1, index 0
    wal.append_log_entry(1, b"second").unwrap(); // seq 2, index 1
    wal.append_log_entry(2, b"third").unwrap(); // seq 3, index 0 (wraparound)

    wal.device.crash().unwrap();
    wal.device.reboot().unwrap();

    let report = wal.recover().unwrap();
    assert_eq!(report.last_sequence, 3);

    let mut rb = AlignedBlock::new();

    // Entry 2 (seq 2) target block 1.
    wal.device.read_block(1, &mut rb.data).unwrap();
    assert_eq!(&rb.data[0..6], b"second");

    // Entry 3 (seq 3) target block 2.
    wal.device.read_block(2, &mut rb.data).unwrap();
    assert_eq!(&rb.data[0..5], b"third");

    // Entry 1 (seq 1) was overwritten in the circular log.
    // Target block 0 should remain empty/zero since it was never checkpointed.
    wal.device.read_block(0, &mut rb.data).unwrap();
    assert!(rb.data.iter().all(|&b| b == 0));
}
