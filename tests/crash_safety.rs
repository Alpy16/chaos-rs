use chaos_disk::{
    AlignedBlock, BlockDevice, BlockDeviceError, FaultPolicy, FaultTrigger, TriggerCondition,
    run_crash_test,
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
                condition: TriggerCondition::OnOperationCount(1),
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
