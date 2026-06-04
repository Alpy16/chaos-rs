use chaos_rs::{
    AdminControls, AlignedBlock, BlockDevice, BlockDeviceError, ChaosDisk, FaultPolicy,
    FaultTrigger, TriggerCondition,
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

    let result = disk.flush();
    assert!(matches!(
        result,
        Err(BlockDeviceError::InterruptedOperation)
    ));

    // After reboot, stable storage should contain the prefix but not the rest.
    // We call crash() first to ensure the volatile ledger and cache are
    // explicitly wiped, mimicking a full power cycle.
    disk.crash().unwrap();
    disk.reboot().unwrap();

    let mut read_buffer = AlignedBlock::new();
    disk.read_block(1, &mut read_buffer.data).unwrap();

    // First 2048 bytes are persisted.
    assert_eq!(read_buffer.data[0], 0xFF);
    assert_eq!(read_buffer.data[2047], 0xFF);
    // Remaining 2048 bytes are still zeroed (old data).
    assert_eq!(read_buffer.data[2048], 0);
    assert_eq!(read_buffer.data[4095], 0);
}

#[test]
fn lost_write_returns_ok_but_does_not_modify_cache() {
    let mut disk = ChaosDisk::new(4);
    let mut write_buffer = AlignedBlock::new();
    let mut read_buffer = AlignedBlock::new();

    disk.set_fault(FaultTrigger {
        condition: TriggerCondition::OnWriteCount(1),
        policy: FaultPolicy::LostWrite,
    });

    write_buffer.data[0..5].copy_from_slice(b"hello");
    let _ = disk.write_block(0, &write_buffer.data);

    let result = disk.flush();
    assert!(result.is_ok());

    // In our model, LostWrite on a write_block means the data never
    // even entered the volatile cache. Therefore, a read (which
    // pulls from cache) returns the old data (zeros).
    disk.read_block(0, &mut read_buffer.data).unwrap();
    assert_eq!(read_buffer.data, [0u8; 4096]);
}

#[test]
fn bit_flip_changes_expected_byte_on_write() {
    let mut disk = ChaosDisk::new(4);
    let mut write_buffer = AlignedBlock::new();
    let mut read_buffer = AlignedBlock::new();

    disk.set_fault(FaultTrigger {
        condition: TriggerCondition::OnWriteCount(1),
        policy: FaultPolicy::BitFlip {
            byte_offset: 100,
            bit_mask: 0b0000_0001, // This tells the disk which bit to XOR
        },
    });

    write_buffer.data[100] = 0b0000_0001;
    disk.write_block(0, &write_buffer.data).unwrap();
    disk.flush().unwrap();
    disk.read_block(0, &mut read_buffer.data).unwrap();
    assert_eq!(read_buffer.data[100], 0b0000_0000);
}

#[test]
fn corrupt_block_fills_block_with_garbage_on_write() {
    let mut disk = ChaosDisk::new(4);
    let mut write_buffer = AlignedBlock::new();
    let mut read_buffer = AlignedBlock::new();

    disk.set_fault(FaultTrigger {
        condition: TriggerCondition::OnWriteCount(1),
        policy: FaultPolicy::CorruptBlock {
            garbage_value: 0xEE,
        },
    });

    write_buffer.data.fill(0xFF);
    disk.write_block(0, &write_buffer.data).unwrap();

    disk.read_block(0, &mut read_buffer.data).unwrap();
    assert!(read_buffer.data.iter().all(|&b| b == 0xEE));
}

#[test]
fn torn_write_interrupts_and_freezes_device() {
    let mut disk = ChaosDisk::new(4);
    let mut write_buffer = AlignedBlock::new();

    disk.set_fault(FaultTrigger {
        condition: TriggerCondition::OnWriteCount(1),
        policy: FaultPolicy::TornWrite {
            bytes_written: 1024,
        },
    });

    let result = disk.write_block(0, &write_buffer.data);
    assert_eq!(result, Err(BlockDeviceError::InterruptedOperation));

    // Subsequent I/O should fail until reboot
    let mut read_buffer = AlignedBlock::new();
    let read_result = disk.read_block(0, &mut read_buffer.data);
    assert_eq!(read_result, Err(BlockDeviceError::FrozenDevice));
}

#[test]
fn torn_write_loses_unflushed_partial_cache_after_reboot() {
    let mut disk = ChaosDisk::new(4);
    let mut write_buffer = AlignedBlock::new();
    write_buffer.data.fill(0xAA);

    disk.set_fault(FaultTrigger {
        condition: TriggerCondition::OnWriteCount(1),
        policy: FaultPolicy::TornWrite {
            bytes_written: 1024,
        },
    });

    let _ = disk.write_block(0, &write_buffer.data);

    // Reboot clears volatile cache. Since the write was torn and never flushed,
    // stable storage should remain pristine (all zeroes).
    disk.reboot().unwrap();

    let mut read_buffer = AlignedBlock::new();
    disk.read_block(0, &mut read_buffer.data).unwrap();
    assert!(read_buffer.data.iter().all(|&b| b == 0));
}

#[test]
fn lost_flush_returns_ok_but_does_not_persist_dirty_block() {
    let mut disk = ChaosDisk::new(4);
    let mut write_buffer = AlignedBlock::new();
    write_buffer.data.fill(0xBB);

    disk.write_block(0, &write_buffer.data).unwrap();

    disk.set_fault(FaultTrigger {
        condition: TriggerCondition::OnFlushCount(1),
        policy: FaultPolicy::LostWrite,
    });

    let result = disk.flush();
    assert!(result.is_ok());

    // Data is lost after power cycle
    disk.crash().unwrap();
    disk.reboot().unwrap();

    let mut read_buffer = AlignedBlock::new();
    disk.read_block(0, &mut read_buffer.data).unwrap();
    assert!(read_buffer.data.iter().all(|&b| b == 0));
}

#[test]
fn bit_flip_on_flush_corrupts_stable_storage() {
    let mut disk = ChaosDisk::new(4);
    let mut write_buffer = AlignedBlock::new();
    write_buffer.data.fill(0xFF);

    disk.write_block(0, &write_buffer.data).unwrap();

    disk.set_fault(FaultTrigger {
        condition: TriggerCondition::OnFlushCount(1),
        policy: FaultPolicy::BitFlip {
            byte_offset: 50,
            bit_mask: 0b1111_1111,
        },
    });

    disk.flush().unwrap();
    disk.crash().unwrap();
    disk.reboot().unwrap();

    let mut read_buffer = AlignedBlock::new();
    disk.read_block(0, &mut read_buffer.data).unwrap();
    assert_eq!(read_buffer.data[50], 0x00); // 0xFF XOR 0xFF
    assert_eq!(read_buffer.data[49], 0xFF);
}

#[test]
fn corrupt_block_on_flush_persists_garbage() {
    let mut disk = ChaosDisk::new(4);
    let mut write_buffer = AlignedBlock::new();
    write_buffer.data.fill(0xFF);

    disk.write_block(0, &write_buffer.data).unwrap();

    disk.set_fault(FaultTrigger {
        condition: TriggerCondition::OnFlushCount(1),
        policy: FaultPolicy::CorruptBlock {
            garbage_value: 0x42,
        },
    });

    disk.flush().unwrap();
    disk.crash().unwrap();
    disk.reboot().unwrap();

    let mut read_buffer = AlignedBlock::new();
    disk.read_block(0, &mut read_buffer.data).unwrap();
    assert!(read_buffer.data.iter().all(|&b| b == 0x42));
}

#[test]
fn on_write_count_triggers_only_target_write_number() {
    let mut disk = ChaosDisk::new(4);
    let mut write_buffer = AlignedBlock::new();
    let mut read_buffer = AlignedBlock::new();

    disk.set_fault(FaultTrigger {
        condition: TriggerCondition::OnWriteCount(2),
        policy: FaultPolicy::LostWrite,
    });

    write_buffer.data.fill(0x11);
    disk.write_block(0, &write_buffer.data).unwrap(); // Count 1: Normal

    write_buffer.data.fill(0x22);
    disk.write_block(1, &write_buffer.data).unwrap(); // Count 2: Faulted

    // read_block pulls from the volatile cache. We verify that
    // Write #1 hit the cache, but Write #2 was dropped by the fault.
    disk.read_block(0, &mut read_buffer.data).unwrap();
    assert_eq!(read_buffer.data[0], 0x11);

    disk.read_block(1, &mut read_buffer.data).unwrap();
    assert_eq!(read_buffer.data[0], 0x00); // Write was dropped
}

#[test]
fn on_flush_count_triggers_only_target_flush_number() {
    let mut disk = ChaosDisk::new(4);
    let mut write_buffer = AlignedBlock::new();

    disk.set_fault(FaultTrigger {
        condition: TriggerCondition::OnFlushCount(2),
        policy: FaultPolicy::LostWrite,
    });

    write_buffer.data.fill(0x11);
    disk.write_block(0, &write_buffer.data).unwrap();
    disk.flush().unwrap(); // Flush 1: Normal

    write_buffer.data.fill(0x22);
    disk.write_block(1, &write_buffer.data).unwrap();
    disk.flush().unwrap(); // Flush 2: Faulted

    disk.crash().unwrap();
    disk.reboot().unwrap();

    let mut read_buffer = AlignedBlock::new();
    disk.read_block(0, &mut read_buffer.data).unwrap();
    assert_eq!(read_buffer.data[0], 0x11);

    disk.read_block(1, &mut read_buffer.data).unwrap();
    assert_eq!(read_buffer.data[0], 0x00); // Flush was dropped
}

#[test]
fn on_block_id_triggers_only_target_block() {
    let mut disk = ChaosDisk::new(4);
    let mut write_buffer = AlignedBlock::new();
    let mut read_buffer = AlignedBlock::new();

    disk.set_fault(FaultTrigger {
        condition: TriggerCondition::OnBlockId(2),
        policy: FaultPolicy::LostWrite,
    });

    write_buffer.data.fill(0xAA);
    disk.write_block(0, &write_buffer.data).unwrap(); // Block 0: Normal
    disk.write_block(2, &write_buffer.data).unwrap(); // Block 2: Faulted

    disk.read_block(0, &mut read_buffer.data).unwrap();
    assert_eq!(read_buffer.data[0], 0xAA);

    disk.read_block(2, &mut read_buffer.data).unwrap();
    assert_eq!(read_buffer.data[0], 0x00);
}

#[test]
fn on_flush_block_targets_specific_block_during_flush() {
    let mut disk = ChaosDisk::new(4);
    let mut write_buffer = AlignedBlock::new();

    write_buffer.data.fill(0x11);
    disk.write_block(1, &write_buffer.data).unwrap();
    write_buffer.data.fill(0x22);
    disk.write_block(2, &write_buffer.data).unwrap();

    disk.set_fault(FaultTrigger {
        condition: TriggerCondition::OnFlushBlock {
            flush_count: 1,
            block_id: 2,
        },
        policy: FaultPolicy::LostWrite,
    });

    disk.flush().unwrap();
    disk.crash().unwrap();
    disk.reboot().unwrap();

    let mut read_buffer = AlignedBlock::new();
    disk.read_block(1, &mut read_buffer.data).unwrap();
    assert_eq!(read_buffer.data[0], 0x11); // Persistent

    disk.read_block(2, &mut read_buffer.data).unwrap();
    assert_eq!(read_buffer.data[0], 0x00); // Lost during targeted flush
}

#[test]
fn clear_fault_disables_scheduled_fault() {
    let mut disk = ChaosDisk::new(4);
    let mut write_buffer = AlignedBlock::new();
    let mut read_buffer = AlignedBlock::new();

    disk.set_fault(FaultTrigger {
        condition: TriggerCondition::OnWriteCount(1),
        policy: FaultPolicy::LostWrite,
    });
    disk.clear_fault();

    write_buffer.data.fill(0xCC);
    disk.write_block(0, &write_buffer.data).unwrap();

    disk.read_block(0, &mut read_buffer.data).unwrap();
    assert_eq!(read_buffer.data[0], 0xCC);
}

#[test]
fn fault_policy_none_behaves_like_normal_operation() {
    let mut disk = ChaosDisk::new(4);
    let mut write_buffer = AlignedBlock::new();
    let mut read_buffer = AlignedBlock::new();

    disk.set_fault(FaultTrigger {
        condition: TriggerCondition::OnWriteCount(1),
        policy: FaultPolicy::None,
    });

    write_buffer.data.fill(0xDD);
    disk.write_block(0, &write_buffer.data).unwrap();

    disk.read_block(0, &mut read_buffer.data).unwrap();
    assert_eq!(read_buffer.data[0], 0xDD);
}

#[test]
fn fault_is_single_use_on_counts() {
    let mut disk = ChaosDisk::new(4);
    let mut write_buffer = AlignedBlock::new();
    let mut read_buffer = AlignedBlock::new();

    // Target exactly the first write
    disk.set_fault(FaultTrigger {
        condition: TriggerCondition::OnWriteCount(1),
        policy: FaultPolicy::LostWrite,
    });

    write_buffer.data.fill(0x11);
    disk.write_block(0, &write_buffer.data).unwrap(); // Triggered: Data lost

    write_buffer.data.fill(0x22);
    disk.write_block(0, &write_buffer.data).unwrap(); // Not Triggered: Should succeed

    disk.read_block(0, &mut read_buffer.data).unwrap();
    assert_eq!(
        read_buffer.data[0], 0x22,
        "Second write should have succeeded as fault is single-use for that count"
    );
}

#[test]
fn replacing_fault_overwrites_previous_fault() {
    let mut disk = ChaosDisk::new(4);
    let mut write_buffer = AlignedBlock::new();
    let mut read_buffer = AlignedBlock::new();

    // First, set a fault to drop the write
    disk.set_fault(FaultTrigger {
        condition: TriggerCondition::OnWriteCount(1),
        policy: FaultPolicy::LostWrite,
    });

    // Then immediately overwrite it with a 'None' policy or a different condition
    disk.set_fault(FaultTrigger {
        condition: TriggerCondition::OnWriteCount(1),
        policy: FaultPolicy::None,
    });

    write_buffer.data.fill(0xAA);
    disk.write_block(0, &write_buffer.data).unwrap();

    disk.read_block(0, &mut read_buffer.data).unwrap();
    assert_eq!(
        read_buffer.data[0], 0xAA,
        "The second fault policy should have superseded the first"
    );
}
