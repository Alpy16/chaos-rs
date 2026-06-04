use chaos_rs::{BlockDevice, ChaosDisk, WalManager};

#[test]
fn checkpoint_replays_entries_and_clears_log_region() {
    // Create a disk with 16 blocks.
    //
    // block 0 = target database block
    // blocks 8..12 = WAL region
    let disk = ChaosDisk::new(16);

    // Create a WAL manager with 4 WAL blocks starting at block 8.
    let mut wal = WalManager::new(disk, 8, 4);

    // Prepare a small payload.
    let payload = b"checkpointed";

    // Append one durable WAL entry targeting block 0.
    wal.append_log_entry(0, payload).unwrap();

    // Checkpoint should:
    // 1. scan valid WAL entries
    // 2. replay them to target blocks
    // 3. flush target blocks
    // 4. clear the WAL region
    // 5. reset current_log_index to 0
    wal.checkpoint().unwrap();

    // Read the target database block.
    let mut target_read = chaos_rs::AlignedBlock::new();
    wal.device.read_block(0, &mut target_read.data).unwrap();

    // The checkpointed payload should have been applied to the target block.
    assert_eq!(&target_read.data[0..payload.len()], payload);

    // Read the first WAL block.
    let mut wal_read = chaos_rs::AlignedBlock::new();
    wal.device.read_block(8, &mut wal_read.data).unwrap();

    // The checkpoint should have cleared the WAL region back to zeroes.
    assert!(wal_read.data.iter().all(|&byte| byte == 0));

    // The WAL write head should be reset to the beginning of the ring.
    assert_eq!(wal.current_log_index, 0);
}
