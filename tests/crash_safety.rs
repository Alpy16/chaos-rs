use chaos_rs::{
    AdminControls, AlignedBlock, BlockDevice, BlockDeviceError, ChaosDisk, FaultPolicy,
    FaultTrigger, TriggerCondition, run_crash_test,
};

#[test]
fn test_flushed_data_survives() {
    // Scenario: Normal operation where data is successfully committed.
    let mut payload = AlignedBlock::new();
    payload.data[0..4].copy_from_slice(b"DATA");

    let (workload_result, crash_result) = run_crash_test(
        10,
        |disk| {
            // Write to volatile RAM, then synchronize to persistent storage
            disk.write_block(0, &payload)?;
            disk.flush()?;
            Ok(())
        },
        |disk| {
            // After crash/reboot, the data must still be there
            let mut read_buffer = AlignedBlock::new();
            disk.read_block(0, &mut read_buffer).unwrap();
            assert_eq!(&read_buffer.data[0..4], b"DATA");
        },
    );

    assert!(workload_result.is_ok());
    assert!(crash_result.is_ok());
}

#[test]
fn test_unflushed_data_is_vaporized() {
    // Scenario: Power loss occurs before data is flushed.
    let mut payload = AlignedBlock::new();
    payload.data[0..4].copy_from_slice(b"LOSS");

    let (workload_result, crash_result) = run_crash_test(
        10,
        |disk| {
            // Data is written to RAM but NEVER flushed to stable storage
            disk.write_block(1, &payload)?;
            Ok(())
        },
        |disk| {
            // After crash, volatile RAM is wiped. Data should be back to zeros.
            let mut read_buffer = AlignedBlock::new();
            disk.read_block(1, &mut read_buffer).unwrap();
            assert_eq!(&read_buffer.data[0..4], b"\0\0\0\0");
        },
    );

    assert!(workload_result.is_ok());
    assert!(crash_result.is_ok());
}

#[test]
fn test_deterministic_torn_write() {
    // Scenario: Power loss occurs exactly halfway through a sector commit.
    let mut payload = AlignedBlock::new();
    for byte in payload.data.iter_mut() {
        *byte = 0xFF;
    }

    let (workload_result, crash_result) = run_crash_test(
        10,
        |disk| {
            disk.write_block(2, &payload)?;

            disk.set_fault(FaultTrigger {
                condition: TriggerCondition::OnFlushCount(1),
                policy: FaultPolicy::TornWrite {
                    bytes_written: 2048,
                },
            });

            // This call should return InterruptedOperation and leave block 2 in a "torn" state
            disk.flush()?;
            Ok(())
        },
        |disk| {
            let mut read_buffer = AlignedBlock::new();
            disk.read_block(2, &mut read_buffer).unwrap();

            assert_eq!(read_buffer.data[0], 0xFF);
            assert_eq!(read_buffer.data[2047], 0xFF);
            assert_eq!(read_buffer.data[2048], 0);
            assert_eq!(read_buffer.data[4095], 0);
        },
    );

    assert!(matches!(
        workload_result,
        Err(BlockDeviceError::InterruptedOperation)
    ));
    assert!(crash_result.is_ok());
}

#[test]
fn test_buffer_size_error() {
    // Scenario: Application provides a buffer that doesn't match the 4KB sector size
    let mut disk = ChaosDisk::new(1);
    let small_buffer = [0u8; 1024];
    let large_buffer = [0u8; 8192];

    assert_eq!(
        disk.write_block(0, &small_buffer),
        Err(BlockDeviceError::BufferSizeError)
    );
    assert_eq!(
        disk.read_block(0, &mut [0u8; 5000]),
        Err(BlockDeviceError::BufferSizeError)
    );
}

#[test]
fn test_alignment_mismatch() {
    // Scenario: Buffer is 4096 bytes but start address is not page-aligned
    let mut disk = ChaosDisk::new(1);

    // Create a large buffer and find an offset that is definitely not a multiple of 4096
    let data = vec![0u8; 10000];
    let base_ptr = data.as_ptr() as usize;
    let aligned_ptr = (base_ptr + 4095) & !4095;
    let unaligned_offset = (aligned_ptr - base_ptr) + 1;

    let unaligned_slice = &data[unaligned_offset..unaligned_offset + 4096];

    assert_eq!(
        disk.write_block(0, unaligned_slice),
        Err(BlockDeviceError::AlignmentMismatch)
    );
}

#[test]
fn test_frozen_device_rejection() {
    // Scenario: Device is unpowered/frozen and must reject all standard I/O
    let mut disk = ChaosDisk::new(1);
    let mut block = AlignedBlock::new();

    // Trigger a crash to freeze the controller
    disk.crash().unwrap();

    assert_eq!(
        disk.read_block(0, &mut block),
        Err(BlockDeviceError::FrozenDevice)
    );
    assert_eq!(
        disk.write_block(0, &block),
        Err(BlockDeviceError::FrozenDevice)
    );
    assert_eq!(disk.flush(), Err(BlockDeviceError::FrozenDevice));
}

#[test]
fn test_lost_write_semantics() {
    // Scenario: Controller reports success but data never hits the media
    let mut payload = AlignedBlock::new();
    payload.data.fill(0xAA);

    let (workload_result, _) = run_crash_test(
        1,
        |disk| {
            disk.set_fault(FaultTrigger {
                condition: TriggerCondition::OnWriteCount(1),
                policy: FaultPolicy::LostWrite,
            });

            // This should return Ok(()) but the write is dropped
            disk.write_block(0, &payload)?;
            disk.flush()?;
            Ok(())
        },
        |disk| {
            let mut read_buffer = AlignedBlock::new();
            disk.read_block(0, &mut read_buffer).unwrap();
            // Should be empty/zeros because the write was lost
            assert_eq!(read_buffer.data, [0u8; 4096]);
        },
    );

    assert!(workload_result.is_ok());
}

#[test]
fn test_bit_flip_precision() {
    // Scenario: Silent data corruption (bit-rot) affects exactly one bit
    let mut payload = AlignedBlock::new();
    payload.data.fill(0x00);

    run_crash_test(
        1,
        |disk| {
            disk.set_fault(FaultTrigger {
                condition: TriggerCondition::OnWriteCount(1),
                policy: FaultPolicy::BitFlip {
                    byte_offset: 100,
                    bit_mask: 0b0000_0001,
                },
            });
            disk.write_block(0, &payload)?;
            disk.flush()?;
            Ok(())
        },
        |disk| {
            let mut read_buffer = AlignedBlock::new();
            disk.read_block(0, &mut read_buffer).unwrap();
            assert_eq!(read_buffer.data[100], 0b0000_0001);
            assert_eq!(read_buffer.data[101], 0x00);
        },
    );
}
