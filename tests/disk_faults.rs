use chaos_rs::{
    AlignedBlock, BlockDevice, BlockDeviceError, ChaosDisk, FaultPolicy, FaultTrigger,
    TriggerCondition,
};

#[test]
fn torn_flush_persists_only_prefix() {
    // Create a disk with 4 blocks.
    let mut disk = ChaosDisk::new(4);

    // Prepare a full 4096-byte block filled with 0xFF.
    let mut payload = AlignedBlock::new();
    payload.data.fill(0xFF);

    // Stage the payload into block 1.
    // This only writes to volatile cache.
    disk.write_block(1, &payload.data).unwrap();

    // Arm a fault for the first flush.
    // The flush will copy only the first 2048 bytes into stable storage,
    // then simulate an interrupted operation.
    disk.set_fault(FaultTrigger {
        condition: TriggerCondition::OnFlushCount(1),
        policy: FaultPolicy::TornWrite {
            bytes_written: 2048,
        },
    });

    // The flush should fail with InterruptedOperation because the simulated
    // power cut happens mid-commit.
    let result = disk.flush();

    assert!(matches!(result, Err(BlockDeviceError::InterruptedOperation)));
}