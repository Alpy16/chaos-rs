use chaos_rs::{BlockDevice, ChaosDisk, WalManager};

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
    let mut read_buffer = chaos_rs::AlignedBlock::new();
    wal.device.read_block(0, &mut read_buffer.data).unwrap();

    // The recovered payload should now exist at the target block.
    assert_eq!(&read_buffer.data[0..payload.len()], payload);
}