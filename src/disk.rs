use crate::device::{AdminControls, BlockDevice, BlockDeviceError};
use std::ops::{Deref, DerefMut};

/// A 4096-byte memory block forced into page-alignment.
/// This is required to simulate Direct I/O and DMA-compatible memory buffers.
#[derive(Clone, Copy)]
#[repr(align(4096))]
pub struct AlignedBlock {
    pub data: [u8; 4096],
}

impl AlignedBlock {
    /// Creates a new, zeroed-out block.
    pub fn new() -> Self {
        AlignedBlock { data: [0; 4096] }
    }
}

impl Deref for AlignedBlock {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl DerefMut for AlignedBlock {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.data
    }
}

/// Defines when a scheduled fault should "trip" and execute its policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerCondition {
    /// Trigger the fault when `write_count` hits this exact number.
    OnWriteCount(u64),

    /// Trigger the fault when `flush_count` hits this exact number.
    /// Note: This will trigger for EVERY dirty block during that specific flush call.
    OnFlushCount(u64),

    /// Trigger the fault whenever a specific `block_id` is targeted.
    OnBlockId(u64),

    /// Trigger the fault for a specific block during a specific flush cycle.
    OnFlushBlock { flush_count: u64, block_id: u64 },
}

#[derive(Debug, Clone, Copy)]
enum OpContext {
    Write,
    Flush,
}

/// The specific type of hardware failure to simulate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultPolicy {
    /// Act completely normal.
    None,

    /// Simulates power failure during a sector commit.
    /// Results in a block that contains a mix of old and new data.
    TornWrite { bytes_written: usize },

    /// Simulates a silent storage controller failure.
    /// Returns `Ok(())` to the caller, but completely drops the data.
    LostWrite,

    /// Simulates bit-rot or magnetic interference by flipping bits
    /// at a specific offset.
    BitFlip { byte_offset: usize, bit_mask: u8 },

    /// Simulates a 'bad sector' that returns garbage data instead of the real payload.
    CorruptBlock { garbage_value: u8 },
}

/// A complete plan for a chaos event: A Condition + A Failure Policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FaultTrigger {
    pub condition: TriggerCondition,
    pub policy: FaultPolicy,
}

/// The primary simulator for a physical disk.
pub struct ChaosDisk {
    /// Simulated Non-Volatile Flash Cells / Platter Space.
    stable_storage: Vec<u8>,

    /// Simulated Volatile Drive Controller RAM / Write Cache.
    volatile_cache: Vec<u8>,

    /// Tracks which blocks are in the cache but not yet committed to stable storage.
    ledger: Vec<bool>,

    /// The Hardware Power Status / Circuit Breaker.
    is_frozen: bool,

    /// Fixed structural block geometry size (rigidly locked to 4096 bytes).
    block_size: usize,

    /// Total physical block sector capacity allocated to this device.
    capacity_blocks: usize,

    /// Global monotonic counter tracking every single `write_block` call.
    write_count: u64,

    /// Global monotonic counter tracking every single `flush` call.
    flush_count: u64,

    /// The active fault injection plan.
    scheduled_fault: Option<FaultTrigger>,
}

impl ChaosDisk {
    /// Allocates and initializes a brand new, pristine virtual storage drive.
    pub fn new(capacity_blocks: usize) -> Self {
        let block_size = 4096;
        let total_bytes = capacity_blocks * block_size;

        ChaosDisk {
            stable_storage: vec![0; total_bytes],
            volatile_cache: vec![0; total_bytes],
            ledger: vec![false; capacity_blocks],
            is_frozen: false,
            block_size,
            capacity_blocks,
            write_count: 0,
            flush_count: 0,
            scheduled_fault: None,
        }
    }

    /// Arms the device with a specific failure scenario.
    pub fn set_fault(&mut self, fault: FaultTrigger) {
        self.scheduled_fault = Some(fault);
    }

    /// Disarms all chaos triggers.
    pub fn clear_fault(&mut self) {
        self.scheduled_fault = None;
    }

    /// Maps a 0-indexed block ID to its byte start/end positions in the internal vectors.
    pub fn calculate_offset(&self, block_id: u64) -> (usize, usize) {
        assert!(
            (block_id as usize) < self.capacity_blocks,
            "Block ID out of physical bounds"
        );
        let offset = (block_id as usize) * self.block_size;
        (offset, offset + self.block_size)
    }

    /// Ensures the provided slice is aligned to 4KB in memory, simulating DMA constraints.
    pub fn check_alignment(&self, buffer: &[u8]) -> Result<(), BlockDeviceError> {
        let memory_address = buffer.as_ptr() as usize;
        if memory_address % self.block_size != 0 {
            return Err(BlockDeviceError::AlignmentMismatch);
        }
        Ok(())
    }

    /// Internal engine that checks if the current op count or block ID matches the armed fault.
    fn should_trigger_fault(
        &self,
        context: OpContext,
        current_op_count: u64,
        current_block_id: u64,
    ) -> Option<FaultPolicy> {
        let trigger = self.scheduled_fault?;
        let is_match = match trigger.condition {
            TriggerCondition::OnWriteCount(target) => {
                matches!(context, OpContext::Write) && target == current_op_count
            }
            TriggerCondition::OnFlushCount(target) => {
                matches!(context, OpContext::Flush) && target == current_op_count
            }
            TriggerCondition::OnBlockId(target) => target == current_block_id,
            TriggerCondition::OnFlushBlock {
                flush_count,
                block_id,
            } => {
                matches!(context, OpContext::Flush)
                    && flush_count == current_op_count
                    && block_id == current_block_id
            }
        };

        if is_match { Some(trigger.policy) } else { None }
    }

    /// Standard commit of a single block to non-volatile media.
    fn commit_block_to_stable(&mut self, block_id: usize, start: usize, end: usize) {
        self.stable_storage[start..end].copy_from_slice(&self.volatile_cache[start..end]);
        self.ledger[block_id] = false;
    }
}

impl BlockDevice for ChaosDisk {
    /// Reads from the volatile cache. This mimics reading from a drive's internal RAM buffer.
    fn read_block(&mut self, block_id: u64, buffer: &mut [u8]) -> Result<(), BlockDeviceError> {
        if self.is_frozen {
            return Err(BlockDeviceError::FrozenDevice);
        }
        if buffer.len() != self.block_size {
            return Err(BlockDeviceError::BufferSizeError);
        }
        if block_id as usize >= self.capacity_blocks {
            return Err(BlockDeviceError::DiskspaceExceeded);
        }
        self.check_alignment(buffer)?;

        let (start, end) = self.calculate_offset(block_id);
        buffer.copy_from_slice(&self.volatile_cache[start..end]);

        Ok(())
    }

    /// Writes data into the volatile cache and marks the block as "dirty" in the ledger.
    fn write_block(&mut self, block_id: u64, data: &[u8]) -> Result<(), BlockDeviceError> {
        if self.is_frozen {
            return Err(BlockDeviceError::FrozenDevice);
        }
        if data.len() != self.block_size {
            return Err(BlockDeviceError::BufferSizeError);
        }
        if block_id as usize >= self.capacity_blocks {
            return Err(BlockDeviceError::DiskspaceExceeded);
        }
        self.check_alignment(data)?;

        self.write_count += 1;

        let (start, end) = self.calculate_offset(block_id);

        if let Some(policy) =
            self.should_trigger_fault(OpContext::Write, self.write_count, block_id)
        {
            match policy {
                FaultPolicy::None => {
                    // Normal operation even if a trigger matched (None policy)
                    self.volatile_cache[start..end].copy_from_slice(data);
                    self.ledger[block_id as usize] = true;
                }
                FaultPolicy::LostWrite => {
                    // Silently drop write: simulate success without dirtying ledger or modifying RAM
                    // Note: Returns Ok(()) by falling through to the end of the function.
                }
                FaultPolicy::TornWrite { bytes_written } => {
                    let clamped_bytes = bytes_written.min(self.block_size);
                    self.volatile_cache[start..start + clamped_bytes]
                        .copy_from_slice(&data[..clamped_bytes]);
                    self.ledger[block_id as usize] = true;
                    // Sever power immediately
                    self.is_frozen = true;
                    return Err(BlockDeviceError::InterruptedOperation);
                }
                FaultPolicy::BitFlip {
                    byte_offset,
                    bit_mask,
                } => {
                    self.volatile_cache[start..end].copy_from_slice(data);
                    if byte_offset < self.block_size {
                        self.volatile_cache[start + byte_offset] ^= bit_mask;
                    }
                    self.ledger[block_id as usize] = true;
                }
                FaultPolicy::CorruptBlock { garbage_value } => {
                    self.volatile_cache[start..end].fill(garbage_value);
                    self.ledger[block_id as usize] = true;
                }
            }
        } else {
            self.volatile_cache[start..end].copy_from_slice(data);
            self.ledger[block_id as usize] = true;
        }

        Ok(())
    }

    /// Commits all 'dirty' blocks from Volatile RAM to Stable Storage.
    fn flush(&mut self) -> Result<(), BlockDeviceError> {
        if self.is_frozen {
            return Err(BlockDeviceError::FrozenDevice);
        }

        self.flush_count += 1;

        // We track if a fault has already been triggered in this specific flush call
        // to prevent repetitive corruption if using OnOperationCount.
        let mut fault_triggered_in_this_op = false;

        for block_id in 0..self.capacity_blocks {
            if self.ledger[block_id] {
                let (start, end) = self.calculate_offset(block_id as u64);

                if let Some(policy) =
                    self.should_trigger_fault(OpContext::Flush, self.flush_count, block_id as u64)
                {
                    match policy {
                        FaultPolicy::None => self.commit_block_to_stable(block_id, start, end),
                        FaultPolicy::LostWrite => {
                            // Clear dirty flag and revert cache to stable storage to simulate
                            // the controller discarding the buffer without actually writing to media.
                            self.ledger[block_id] = false;
                            self.volatile_cache[start..end]
                                .copy_from_slice(&self.stable_storage[start..end]);
                        }
                        FaultPolicy::TornWrite { bytes_written } => {
                            let clamped_bytes = bytes_written.min(self.block_size);
                            self.stable_storage[start..start + clamped_bytes].copy_from_slice(
                                &self.volatile_cache[start..start + clamped_bytes],
                            );
                            // Crash midway through the loop
                            self.is_frozen = true;
                            return Err(BlockDeviceError::InterruptedOperation);
                        }
                        FaultPolicy::BitFlip {
                            byte_offset,
                            bit_mask,
                        } if !fault_triggered_in_this_op => {
                            self.commit_block_to_stable(block_id, start, end);
                            if byte_offset < self.block_size {
                                self.stable_storage[start + byte_offset] ^= bit_mask;
                            }

                            // Mark fault as consumed for this specific flush invocation
                            fault_triggered_in_this_op = true;
                        }
                        FaultPolicy::BitFlip { .. } => {
                            self.commit_block_to_stable(block_id, start, end);
                        }
                        FaultPolicy::CorruptBlock { garbage_value } => {
                            self.stable_storage[start..end].fill(garbage_value);
                            self.ledger[block_id] = false;
                        }
                    }
                } else {
                    self.commit_block_to_stable(block_id, start, end);
                }
            }
        }

        Ok(())
    }
}

impl AdminControls for ChaosDisk {
    fn crash(&mut self) -> Result<(), BlockDeviceError> {
        self.is_frozen = true;

        let total_bytes = self.capacity_blocks * self.block_size;
        self.volatile_cache = vec![0; total_bytes];

        for flag in self.ledger.iter_mut() {
            *flag = false;
        }

        Ok(())
    }

    fn reboot(&mut self) -> Result<(), BlockDeviceError> {
        // Power returns, controller re-initializes volatile cache from stable storage
        self.is_frozen = false;
        self.volatile_cache.copy_from_slice(&self.stable_storage);
        Ok(())
    }

    fn is_frozen(&self) -> Result<bool, BlockDeviceError> {
        Ok(self.is_frozen)
    }

    fn resize(&mut self, new_size: u64) -> Result<(), BlockDeviceError> {
        // Simulates dynamic resizing of a virtual disk partition
        let new_capacity = new_size as usize;
        let new_byte_count = new_capacity * self.block_size;

        self.stable_storage.resize(new_byte_count, 0);
        self.volatile_cache.resize(new_byte_count, 0);
        self.ledger.resize(new_capacity, false);
        self.capacity_blocks = new_capacity;

        Ok(())
    }
}
