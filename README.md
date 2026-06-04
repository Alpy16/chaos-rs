# chaos-rs

![CI](https://github.com/Alpy16/chaos-rs/actions/workflows/rust.yml/badge.svg)

A zero-dependency, deterministic, page-aligned virtual block-device simulator and circular Write-Ahead Log (WAL) framework built in Rust for crash-consistency and fault-injection testing.

`chaos-rs` provides a user-space environment for testing storage engines, WAL implementations, and persistence layers under realistic hardware failure conditions. It simulates volatile write caches, durable media, explicit flush boundaries, and deterministic storage faults such as torn writes, lost writes, bit flips, and sector corruption.

---

## About

Modern storage software is often tested against idealized I/O behavior. Real hardware is not ideal.

`chaos-rs` exists to provide a lightweight environment where infrastructure engineers can deterministically reproduce storage failures and verify recovery logic without relying on operating-system-level fault injection, external databases, or heavyweight testing frameworks.

The project is intentionally focused on a narrow goal:

> Simulate hardware failure and validate crash-recovery correctness.

It is **not** a production database, a virtual filesystem, or a benchmarking framework.

---

## Features

### Block Device Simulation

- Dual-layer storage model:
  - Volatile controller cache
  - Stable persistent media
- Explicit durability boundaries via `flush()`
- Simulated crash and reboot operations
- Strict 4096-byte page-aligned I/O
- Configurable device resizing
- Frozen-device failure states

### Deterministic Fault Injection

Supported fault policies:

| Fault | Description |
|---------|-------------|
| `TornWrite` | Commits only a prefix of a block and interrupts the operation |
| `LostWrite` | Silently drops a write while reporting success |
| `BitFlip` | Flips specific bits at a target byte offset |
| `CorruptBlock` | Overwrites a block with deterministic garbage data |

Supported trigger conditions:

- Write count
- Flush count
- Block ID
- Flush count + block ID

### Circular Write-Ahead Log

- Fixed-size circular log region
- 32-byte WAL frame header
- CRC-32 integrity validation
- Monotonic sequence IDs
- Crash-boundary detection
- Chronological replay
- Log checkpointing
- Recovery reporting

### Zero Dependencies

`chaos-rs` is built entirely on the Rust standard library.

No external crates are required.

---

## Architecture

```text
src/
├── device.rs
├── disk.rs
├── wal.rs
└── lib.rs
```

### device.rs

Defines hardware-facing abstractions:

```rust
pub trait BlockDevice
pub trait AdminControls
pub enum BlockDeviceError
```

### disk.rs

Implements:

```rust
ChaosDisk
AlignedBlock
FaultPolicy
FaultTrigger
TriggerCondition
```

Responsibilities:

- Stable media simulation
- Volatile cache simulation
- Alignment enforcement
- Fault scheduling
- Crash/reboot behavior

### wal.rs

Implements:

```rust
WalManager
LogHeader
RecoveryReport
WalError
```

Responsibilities:

- WAL frame creation
- CRC validation
- Recovery replay
- Checkpointing
- Circular log management

---

## Quickstart

### Requirements

- Rust stable
- Cargo

### Installation

Local dependency:

```toml
[dependencies]
chaos_rs = { path = "../chaos-rs" }
```

GitHub dependency:

```toml
[dependencies]
chaos_rs = { git = "https://github.com/Alpy16/chaos-rs" }
```

---

## Core APIs

### Block Device

```rust
pub trait BlockDevice {
    fn read_block(
        &mut self,
        block_id: u64,
        buffer: &mut [u8],
    ) -> Result<(), BlockDeviceError>;

    fn write_block(
        &mut self,
        block_id: u64,
        data: &[u8],
    ) -> Result<(), BlockDeviceError>;

    fn flush(&mut self) -> Result<(), BlockDeviceError>;
}
```

### Administrative Controls

```rust
pub trait AdminControls {
    fn crash(&mut self) -> Result<(), BlockDeviceError>;

    fn reboot(&mut self) -> Result<(), BlockDeviceError>;

    fn is_frozen(&self) -> Result<bool, BlockDeviceError>;

    fn resize(&mut self, new_size: u64)
        -> Result<(), BlockDeviceError>;
}
```

### WAL Recovery

```rust
pub struct RecoveryReport {
    pub last_sequence: u64,
    pub corruption_detected: bool,
}
```

```rust
pub fn recover(
    &mut self,
) -> Result<RecoveryReport, WalError>;
```

Recovery:

1. Scans the WAL region
2. Validates CRC-32 checksums
3. Detects corruption boundaries
4. Replays valid entries in sequence order
5. Synchronizes WAL state
6. Returns a recovery report

---

## Example

### Deterministic Torn Write

```rust
use chaos_rs::{
    AlignedBlock,
    BlockDevice,
    FaultPolicy,
    FaultTrigger,
    TriggerCondition,
    run_crash_test,
};

let mut payload = AlignedBlock::new();
payload.data.fill(0xFF);

let (_workload, _recovery) = run_crash_test(
    10,
    |disk| {
        disk.write_block(2, &payload.data)?;

        disk.set_fault(FaultTrigger {
            condition: TriggerCondition::OnFlushCount(1),
            policy: FaultPolicy::TornWrite {
                bytes_written: 2048,
            },
        });

        disk.flush()?;
        Ok(())
    },
    |disk| {
        let mut read_buffer = AlignedBlock::new();

        disk.read_block(2, &mut read_buffer.data)
            .unwrap();

        assert_eq!(read_buffer.data[0], 0xFF);
        assert_eq!(read_buffer.data[2047], 0xFF);
        assert_eq!(read_buffer.data[2048], 0);
    },
);
```

---

## Testing

Run all tests:

```bash
cargo test
```

Run formatting checks:

```bash
cargo fmt --check
```

Run linting:

```bash
cargo clippy -- -D warnings
```

Current coverage includes:

- Block device correctness
- Flush durability
- Crash/reboot semantics
- Alignment validation
- Device resizing
- Lost writes
- Torn writes
- Torn flushes
- Bit flips
- Corrupt blocks
- Deterministic trigger scheduling
- WAL frame validation
- WAL recovery
- Corruption boundaries
- Circular log wraparound
- WAL checkpointing

---

## Future Work

- Property-based crash testing
- Trace recording and replay
- Latency simulation
- Async I/O fault scheduling
- Python bindings
- Storage-engine integration examples

---

## License

MIT