# chaos-rs

A deterministic, high-performance block-layer simulation engine built in Rust for fault-injection testing. `chaos-rs` provides an isolated, dual-layer virtual storage drive specifically architected to validate the atomic crash-safety invariants of write-ahead logs (WAL), transactional storage engines, and filesystems.

By implementing strict Direct I/O emulation, memory page-alignment verification, and an independent administrative control harness, `chaos-rs` allows test suites to simulate physical hardware anomalies—such as ungraceful power loss, torn writes, and hardware unresponsiveness—with absolute determinism.

---

## Architecture Overview

`chaos-rs` decouples the storage runtime into two distinct conceptual surfaces:

1. **The Operational Face (`BlockDevice`):** The standard hardware abstraction layer exposed to your database engine. It enforces uniform, 4096-byte block boundaries and Direct I/O memory constraints.
2. **The Administrative Face (`AdminControls`):** A segregated control interface used exclusively by testing harnesses to orchestrate macro-level physical faults (e.g., pulling the power cable, triggering cold reboots).

```
   PRODUCTION ENGINE                     CHAOS TESTING HARNESS
         │                                         │
         ▼ (BlockDevice Trait)                     ▼ (AdminControls Trait)
 ┌─────────────────────────────────────────────────────────────────┐
 │                           CHAOSDISK                             │
 │                                                                 │
 │   ┌───────────────────────┐             ┌───────────────────┐   │
 │   │ Volatile Cache (RAM)  │ ──[Flush]──>│  Stable Storage   │   │
 │   └───────────────────────┘             └───────────────────┘   │
 │               │                                   ▲             │
 │               └─────────── [Simulated Crash] ─────┘             │
 │                                (Vaporizes)                      │
 └─────────────────────────────────────────────────────────────────┘

```

### Core Components

* **Dual-Layer Memory Geometry:** Maintains an active volatile write buffer (simulated drive controller RAM) alongside an underlying non-volatile persistence array (simulated disk platter).
* **Direct I/O Guardrails:** Validates memory buffer address pointers using modulo arithmetic. Slices passed to the device must be strictly page-aligned (multiples of 4096 bytes) to match Direct Memory Access (DMA) physical constraints.
* **Dirty Page Ledger:** Seamlessly tracks unsynchronized sectors to precisely simulate the volatility of hardware write-caches prior to an explicit synchronization barrier (`flush`).

---

## Features

* **Deterministic Fault Injection:** Drive behavior is entirely reproducible, allowing edge-case storage bugs to be isolated and debugged reliably within standard CI/CD pipelines.
* **Torn Write Simulation:** Injects mid-block truncation and sector corruption during ungraceful synchronization loops to verify checksum and torn-write protection mechanisms.
* **Zero Overhead Baseline:** Built with optimized, low-level memory copies and dynamic vectors, ensuring minimal test-runtime inflation.

---

## API Specification

### The Operational Face

```rust
pub trait BlockDevice {
    /// Fetches a 4096-byte sector from the active volatile workspace.
    fn read_block(&mut self, block_id: u64, buffer: &mut [u8]) -> Result<(), BlockDeviceError>;

    /// Stages a 4096-byte chunk into the controller write cache.
    fn write_block(&mut self, block_id: u64, data: &[u8]) -> Result<(), BlockDeviceError>;

    /// The Synchronization Barrier (fsync/fdatasync).
    /// Commits dirty sectors from volatile cache to stable storage.
    fn flush(&mut self) -> Result<(), BlockDeviceError>;
}

```

### The Administrative Face

```rust
pub trait AdminControls {
    /// Instantly vaporizes the volatile cache, clears the ledger,
    /// and places the device into an unresponsive state.
    fn crash(&mut self) -> Result<(), BlockDeviceError>;

    /// Restores power to the device controller, re-initializing 
    /// state strictly from data that survived stable storage.
    fn reboot(&mut self) -> Result<(), BlockDeviceError>;

    /// Diagnostic check evaluating device power state.
    fn is_frozen(&self) -> Result<bool, BlockDeviceError>;

    /// Resizes the underlying storage arrays dynamically.
    fn resize(&mut self, new_size: u64) -> Result<(), BlockDeviceError>;
}

```

---

## Error Model

`chaos-rs` provides an explicit, hardware-centric error model (`BlockDeviceError`) to mirror physical disk-controller logic rather than high-level OS software errors:

| Error Variant | Root Cause |
| --- | --- |
| `AlignmentMismatch` | The memory buffer address in RAM is not aligned to a 4096-byte boundary, violating DMA rules. |
| `DiskspaceExceeded` | The requested logical block ID falls outside the physically provisioned sector capacity. |
| `FrozenDevice` | An I/O request was executed against an unpowered or dead drive controller following a crash. |
| `InterruptedOperation` | A write or flush sequence was truncated midway through execution by a crash event. |

---

## License

This project is licensed under the MIT License - see the LICENSE file for details.# chaos-rs
