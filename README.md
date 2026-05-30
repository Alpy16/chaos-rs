# chaos_disk

A deterministic, hardware-level chaos engineering framework and storage device simulator built in Rust.

`chaos_disk` provides a page-aligned virtual disk sandbox designed to intercept low-level I/O operations and inject structural media failures—such as torn writes, lost writes, bit-rot, and sector corruption—at precise operational boundaries. It allows infrastructure engineers to build automated, reliable crash-consistency tests for databases, filesystems, and write-ahead logs without flaky or non-reproducible test runs.

---

## Architecture

The simulator maintains a dual-layer memory architecture in user space to mirror bare-metal storage controllers:

* **Volatile Cache (`volatile_cache`):** Simulates onboarding drive RAM/write buffers. Ephemeral data stays here and is lost instantly during an ungraceful shutdown.
* **Stable Storage (`stable_storage`):** Simulates persistent, non-volatile media cells (NAND Flash or Magnetic Platter). Data here survives power severance.
* **Dirty Page Ledger (`ledger`):** A tracking bitset identifying modified cache blocks that have not yet been synchronized via a barrier call.

---

## Features

* **Strict Page Alignment:** Enforces 4096-byte boundary verification on raw buffers to accurately mimic Direct I/O (`O_DIRECT`) and DMA constraints.
* **Deterministic Scheduling:** Faults trip exactly on targeted write counts, flush counts, or specific block indices, eliminating random flakiness in CI/CD pipelines.
* **Administrative Lifecycle Control:** Exposes manual hooks to `.crash()`, `.reboot()`, and `.resize()` the underlying partition media outside of the production application runtime.
* **Automated Orchestration Harness:** Provides a generic `run_crash_test` framework that wraps workload execution, hardware power-severance, reboot recovery, and post-mortem state verification into a single atomic test cycle.

---

## Core Abstractions

```rust
pub trait BlockDevice {
    fn read_block(&mut self, block_id: u64, buffer: &mut [u8]) -> Result<(), BlockDeviceError>;
    fn write_block(&mut self, block_id: u64, data: &[u8]) -> Result<(), BlockDeviceError>;
    fn flush(&mut self) -> Result<(), BlockDeviceError>;
}

pub trait AdminControls {
    fn crash(&mut self) -> Result<(), BlockDeviceError>;
    fn reboot(&mut self) -> Result<(), BlockDeviceError>;
    fn is_frozen(&self) -> Result<bool, BlockDeviceError>;
    fn resize(&mut self, new_size: u64) -> Result<(), BlockDeviceError>;
}

```

---

## Usage Example

The following integration snippet sets up a deterministic power loss mid-operation during a storage sync routine:

```rust
use chaos_disk::{AlignedBlock, BlockDevice, BlockDeviceError, FaultPolicy, FaultTrigger, TriggerCondition, run_crash_test};

#[test]
fn test_deterministic_torn_write() {
    let mut payload = AlignedBlock::new();
    payload.data.fill(0xFF);

    let (workload_result, crash_result) = run_crash_test(
        10,
        |disk| {
            // 1. Stage full payload cleanly into volatile controller memory
            disk.write_block(2, &payload)?;

            // 2. Schedule a power cut precisely halfway through the first flush iteration
            disk.set_fault(FaultTrigger {
                condition: TriggerCondition::OnOperationCount(1),
                policy: FaultPolicy::TornWrite { bytes_written: 2048 },
            });

            // 3. This barrier call will truncate data transfer and return InterruptedOperation
            disk.flush()?;
            Ok(())
        },
        |disk| {
            // 4. Post-Reboot Verification Phase
            let mut read_buffer = AlignedBlock::new();
            disk.read_block(2, &mut read_buffer).unwrap();

            // First 2KB successfully hit persistent platter cells before power cut
            assert_eq!(read_buffer.data[0], 0xFF);
            assert_eq!(read_buffer.data[2047], 0xFF);

            // Remaining 2KB never committed and stayed zeroed
            assert_eq!(read_buffer.data[2048], 0);
            assert_eq!(read_buffer.data[4095], 0);
        },
    );

    assert!(matches!(workload_result, Err(BlockDeviceError::InterruptedOperation)));
    assert!(crash_result.is_ok());
}

```

---

## Testing

Run the integration and verification test suite using cargo:

```bash
cargo test

```