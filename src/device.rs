/// Low-level hardware error states that simulate physical disk controller failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockDeviceError {
    /// Triggered when memory is not aligned to 4096-byte boundaries.
    /// This mimics `O_DIRECT` requirements where hardware DMA engines cannot
    /// cross page boundaries.
    AlignmentMismatch,

    /// Attempted to access a block index outside the physical bounds of the disk.
    DiskspaceExceeded,

    /// The device is in a 'Frozen' state (simulating power loss).
    /// All I/O is rejected until an administrative reboot occurs.
    FrozenDevice,

    /// A critical failure where an operation (like a Flush) was cut short.
    /// This is used to signal that a "Torn Write" may have occurred on the media.
    InterruptedOperation,

    /// The provided buffer size does not match the device's fixed sector size.
    BufferSizeError,
}

impl std::fmt::Display for BlockDeviceError {
    /// Provides human-readable descriptions of hardware-level failures.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BlockDeviceError::AlignmentMismatch => {
                write!(
                    f,
                    "Hardware Alignment Violation: Memory buffer address must be page-aligned to block geometry parameters."
                )
            }
            BlockDeviceError::DiskspaceExceeded => {
                write!(
                    f,
                    "Boundary Error: Requested logical block address exceeds total physical sector capacity."
                )
            }
            BlockDeviceError::FrozenDevice => {
                write!(
                    f,
                    "Hardware Fault: Device is unresponsive. Drive controller has lost power or experienced critical media failure."
                )
            }
            BlockDeviceError::InterruptedOperation => {
                write!(
                    f,
                    "I/O Fault: Write operation was truncated or torn due to an ungraceful system crash."
                )
            }
            BlockDeviceError::BufferSizeError => {
                write!(
                    f,
                    "Buffer Size Error: The provided buffer is not the correct size for the requested operation."
                )
            }
        }
    }
}

impl std::error::Error for BlockDeviceError {}

/// The standard hardware interface for I/O operations.
///
/// This trait mimics the behavior of a raw block device, requiring fixed-size
/// transfers and explicit synchronization (flushing) to ensure durability.
pub trait BlockDevice {
    /// Reads a single 4KB sector from the volatile cache into the provided buffer.
    ///
    /// Returns `AlignmentMismatch` if the buffer is not page-aligned.
    fn read_block(&mut self, block_id: u64, buffer: &mut [u8]) -> Result<(), BlockDeviceError>;

    /// Writes a 4KB sector into the device's volatile write cache.
    ///
    /// NOTE: Data is NOT persistent until `flush()` is called. A crash at this
    /// stage results in total data loss for this block.
    fn write_block(&mut self, block_id: u64, data: &[u8]) -> Result<(), BlockDeviceError>;

    /// The synchronization barrier (equivalent to `fsync`).
    ///
    /// Physically moves data from the volatile RAM cache to non-volatile stable storage.
    /// This is where most "Torn Writes" or "Lost Writes" occur in real systems.
    fn flush(&mut self) -> Result<(), BlockDeviceError>;
}

/// Management interface used by tests to simulate physical environment changes.
///
/// These methods allow a test harness to "pull the plug" or reboot the device,
/// actions that standard software cannot perform on itself.
pub trait AdminControls {
    /// Mimics a power outage: wipes volatile RAM and freezes the controller.
    fn crash(&mut self) -> Result<(), BlockDeviceError>;

    /// Mimics a system restart: restores volatile RAM from the stable storage
    /// (the platter/NAND) and thaws the controller for new I/O.
    fn reboot(&mut self) -> Result<(), BlockDeviceError>;

    /// Checks if the device is currently in a 'Frozen' (unpowered) state.
    fn is_frozen(&self) -> Result<bool, BlockDeviceError>;

    /// Changes the physical capacity of the disk.
    fn resize(&mut self, new_size: u64) -> Result<(), BlockDeviceError>;
}
