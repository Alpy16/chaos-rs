# chaos-rs

A deterministic, page-aligned virtual disk simulator and circular Write-Ahead Log (WAL) framework built in Rust for crash-consistency and fault-injection testing.

`chaos-rs` provides a user-space environment to simulate low-level block storage devices and WAL engines. It allows infrastructure engineers to inject media faults (torn writes, lost writes, bit-rot, and sector corruption) at precise, deterministic operational boundaries.

---

## Core Components

The codebase consists of three modules:
1. **`device`**: Defines standard hardware interface traits (`BlockDevice`, `AdminControls`) and error states (`BlockDeviceError`).
2. **`disk`**: Implements `ChaosDisk`, a dual-layer virtual storage drive simulating volatile write cache and stable media platter storage, with page-aligned (`AlignedBlock`) operations.
3. **`wal`**: Implements `WalManager`, a circular Write-Ahead Log engine with integrity checksum validation, crash recovery replay, and log checkpoints.

---

## Features

### Storage Simulation & Fault Injection
- **Volatile & Stable Storage:** Simulates drive controller RAM cache and persistent stable media separately. Ephemeral cache data is lost on simulated crashes.
- **Strict Page Alignment:** Enforces 4096-byte DMA boundary checks on raw buffers.
- **Deterministic Chaos Injection:** Schedule faults to trigger on exact write counts, flush counts, or block IDs.
- **Fault Policies:**
  - `TornWrite`: Simulates midway power failure by committing only a prefix of a block.
  - `LostWrite`: Simulates silent controller drops (success returned, but data not written).
  - `BitFlip`: Simulates magnetic/electrical bit-rot at specific offsets.
  - `CorruptBlock`: Fills blocks with garbage values.

### Circular Write-Ahead Log (WAL)
- **Compact Log Header:** Pre-allocates a 32-byte header containing magic bytes (`A016`), checksum, monotonic sequence ID, and target block ID.
- **Additive Checksum:** Integrates checksum calculation over WAL frames to validate integrity during recovery.
- **Log Recovery:** Scans log partitions, verifies checksums to identify crash points, sorts entries by sequence ID, and chronologically replays updates to main storage.
- **Log Checkpointing:** Replays pending updates from the WAL partition to their home database slots and wipes log headers to reclaim circular space.

---

## Core Interfaces

### Block Device API
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

### WAL Manager API
```rust
pub struct WalManager {
    pub device: ChaosDisk,
    pub next_sequence_id: u64,
    pub log_start_block: u64,
    pub current_log_block: u64,
    pub max_log_blocks: u64,
}

impl WalManager {
    pub fn new(device: ChaosDisk, log_start_block: u64, max_log_blocks: u64) -> Self;
    pub fn append_log_entry(&mut self, target_block_id: u64, payload: &[u8]) -> Result<(), BlockDeviceError>;
    pub fn recover(&mut self) -> Result<(), BlockDeviceError>;
    pub fn checkpoint(&mut self) -> Result<(), BlockDeviceError>;
}
```

---

## Usage Examples

### Deterministic Torn Write Verification
```rust
use chaos_rs::{AlignedBlock, BlockDevice, BlockDeviceError, FaultPolicy, FaultTrigger, TriggerCondition, run_crash_test};

#[test]
fn test_deterministic_torn_write() {
    let mut payload = AlignedBlock::new();
    payload.data.fill(0xFF);

    let (workload_result, crash_result) = run_crash_test(
        10,
        |disk| {
            disk.write_block(2, &payload)?;
            disk.set_fault(FaultTrigger {
                condition: TriggerCondition::OnFlushCount(1),
                policy: FaultPolicy::TornWrite { bytes_written: 2048 },
            });
            disk.flush()?;
            Ok(())
        },
        |disk| {
            let mut read_buffer = AlignedBlock::new();
            disk.read_block(2, &mut read_buffer).unwrap();
            assert_eq!(read_buffer.data[0], 0xFF);
            assert_eq!(read_buffer.data[2047], 0xFF);
            assert_eq!(read_buffer.data[2048], 0);
        },
    );
}
```

---

## Testing

Run the verification suite:
```bash
cargo test
```