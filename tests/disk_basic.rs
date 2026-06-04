use chaos_rs::{AdminControls, AlignedBlock, BlockDevice, BlockDeviceError, ChaosDisk};

#[test]
fn flushed_write_survives_crash_and_reboot() {
    // Create a tiny virtual disk with 4 fixed-size 4096-byte blocks.
    let mut disk = ChaosDisk::new(4);

    // Create a page-aligned 4096-byte write buffer.
    // AlignedBlock is important because ChaosDisk enforces Direct I/O-style alignment.
    let mut write_buffer = AlignedBlock::new();

    // Put recognizable data into the beginning of the block.
    // The rest of the block remains zeroed.
    write_buffer.data[0..5].copy_from_slice(b"hello");

    // Write the block into the device's volatile cache.
    // At this point, the data is NOT durable yet.
    disk.write_block(0, &write_buffer.data).unwrap();

    // Flush moves dirty volatile cache blocks into stable storage.
    // After this point, the write should survive a crash.
    disk.flush().unwrap();

    // Simulate sudden power loss.
    // This destroys volatile cache state and freezes the device.
    disk.crash().unwrap();

    // Simulate reboot.
    // This restores readable volatile state from stable storage.
    disk.reboot().unwrap();

    // Create a fresh aligned read buffer.
    let mut read_buffer = AlignedBlock::new();

    // Read block 0 after reboot.
    disk.read_block(0, &mut read_buffer.data).unwrap();

    // The flushed data should still be present.
    assert_eq!(&read_buffer.data[0..5], b"hello");
}

#[test]
fn new_disk_reads_zeroed_blocks() {
    // 1. Spin up a fresh virtual drive partition with 4 blocks
    let mut disk = ChaosDisk::new(4);

    // 2. Allocate our 4096-byte RAM workspace
    let mut read_buffer = AlignedBlock::new();

    // 3. Perform a full-page read from physical block 0 and unwrap the result.
    // By passing the Deref handle `&mut *read_buffer`, we give it the full 4096 bytes.
    disk.read_block(0, &mut *read_buffer).unwrap();

    // 4. Assert that the block is filled with absolute silence.
    // We compare our filled read buffer against a baseline of 4096 zeros.
    assert_eq!(read_buffer.data, [0u8; 4096]);
}

#[test]
fn write_then_read_returns_same_block() {
    let mut disk = ChaosDisk::new(4);
    let mut write_buffer = AlignedBlock::new();
    let mut read_buffer = AlignedBlock::new();

    write_buffer.data[0..5].copy_from_slice(b"hello");
    disk.write_block(0, &write_buffer.data).unwrap();
    disk.flush().unwrap();
    disk.read_block(0, &mut read_buffer.data).unwrap();
    assert_eq!(&read_buffer.data[0..5], b"hello");
}

#[test]
fn multiple_blocks_are_isolated() {
    let mut disk = ChaosDisk::new(4);
    let mut write_buffer = AlignedBlock::new();
    let mut read_buffer = AlignedBlock::new();

    write_buffer.data[0..5].copy_from_slice(b"hello");
    disk.write_block(1, &write_buffer.data).unwrap();
    write_buffer.data.fill(0);
    write_buffer.data[0..4].copy_from_slice(b"aaaa");
    disk.write_block(0, &write_buffer.data).unwrap();
    disk.flush().unwrap();
    disk.read_block(0, &mut read_buffer.data).unwrap();
    assert_eq!(&read_buffer.data[0..4], b"aaaa");
    disk.read_block(1, &mut read_buffer.data).unwrap();
    assert_eq!(&read_buffer.data[0..5], b"hello");
}

#[test]
fn unflushed_write_is_lost_after_crash_and_reboot() {
    let mut disk = ChaosDisk::new(4);
    let mut write_buffer = AlignedBlock::new();

    write_buffer.data[0..5].copy_from_slice(b"hello");
    disk.write_block(0, &write_buffer.data).unwrap();
    disk.crash().unwrap();
    disk.reboot().unwrap();

    let mut read_buffer = AlignedBlock::new();
    disk.read_block(0, &mut read_buffer.data).unwrap();
    assert_eq!(read_buffer.data, [0u8; 4096]);
}

#[test]
fn frozen_device_rejects_read() {
    let mut disk = ChaosDisk::new(4);

    disk.crash().unwrap();
    let mut buffer = AlignedBlock::new();

    let result = disk.read_block(0, &mut buffer.data);

    assert_eq!(result, Err(BlockDeviceError::FrozenDevice));
}

#[test]
fn frozen_device_rejects_write() {
    let mut disk = ChaosDisk::new(4);

    disk.crash().unwrap();
    let buffer = AlignedBlock::new();

    let result = disk.write_block(0, &buffer.data);

    assert_eq!(result, Err(BlockDeviceError::FrozenDevice));
}

#[test]
fn frozen_device_rejects_flush() {
    let mut disk = ChaosDisk::new(4);

    disk.crash().unwrap();

    let result = disk.flush();

    assert_eq!(result, Err(BlockDeviceError::FrozenDevice));
}

#[test]
fn reboot_restores_device_after_crash() {
    let mut disk = ChaosDisk::new(4);
    let mut write_buffer = AlignedBlock::new();
    let mut read_buffer = AlignedBlock::new();

    write_buffer.data[0..5].copy_from_slice(b"hello");
    disk.write_block(1, &write_buffer.data).unwrap();
    disk.flush().unwrap();

    disk.crash().unwrap();
    let result = disk.write_block(1, &write_buffer.data);
    assert_eq!(result, Err(BlockDeviceError::FrozenDevice));

    disk.reboot().unwrap();

    disk.read_block(1, &mut read_buffer.data).unwrap();
    assert_eq!(&read_buffer.data[0..5], b"hello");
}

#[test]
fn out_of_bounds_read_returns_diskspace_exceeded() {
    let mut disk = ChaosDisk::new(4);
    let mut read_buffer = AlignedBlock::new();

    let result = disk.read_block(6, &mut read_buffer.data);
    assert_eq!(result, Err(BlockDeviceError::DiskspaceExceeded));
}

#[test]
fn out_of_bounds_write_returns_diskspace_exceeded() {
    let mut disk = ChaosDisk::new(4);
    let mut write_buffer = AlignedBlock::new();

    let result = disk.write_block(6, &mut write_buffer.data);
    assert_eq!(result, Err(BlockDeviceError::DiskspaceExceeded));
}

#[test]
fn wrong_read_buffer_size_returns_buffer_size_error() {
    let mut disk = ChaosDisk::new(4);

    // We create a buffer that is too small (e.g., 100 bytes)
    let mut wrong_buffer = [0u8; 100];

    let result = disk.read_block(0, &mut wrong_buffer);

    assert_eq!(result, Err(BlockDeviceError::BufferSizeError));
}

#[test]
fn wrong_write_buffer_size_returns_buffer_size_error() {
    let mut disk = ChaosDisk::new(4);

    let mut wrong_buffer = [0u8; 100];

    let result = disk.write_block(0, &mut wrong_buffer);

    assert_eq!(result, Err(BlockDeviceError::BufferSizeError));
}

#[test]
fn resize_expands_disk_with_zeroed_new_blocks() {
    let mut disk = ChaosDisk::new(4);
    let mut write_buffer = AlignedBlock::new();
    let mut read_buffer = AlignedBlock::new();

    write_buffer.data[0..5].copy_from_slice(b"hello");
    disk.write_block(1, &write_buffer.data).unwrap();
    disk.flush().unwrap();
    disk.resize(50).unwrap();

    disk.read_block(1, &mut read_buffer.data).unwrap();
    assert_eq!(&read_buffer.data[0..5], b"hello");
    disk.read_block(49, &mut read_buffer.data).unwrap();
    assert_eq!(read_buffer.data, [0u8; 4096]);
}

#[test]
fn resize_shrinks_disk_and_rejects_old_blocks() {
    let mut disk = ChaosDisk::new(50);
    let mut write_buffer = AlignedBlock::new();
    let mut read_buffer = AlignedBlock::new();

    write_buffer.data[0..5].copy_from_slice(b"hello");
    disk.write_block(1, &write_buffer.data).unwrap();
    disk.flush().unwrap();
    disk.resize(4).unwrap();

    disk.read_block(1, &mut read_buffer.data).unwrap();
    assert_eq!(&read_buffer.data[0..5], b"hello");
    let result = disk.read_block(49, &mut read_buffer.data);
    assert_eq!(result, Err(BlockDeviceError::DiskspaceExceeded));
}

#[test]
fn crash_without_dirty_blocks_is_safe() {
    let mut disk = ChaosDisk::new(4);
    let mut read_buffer = AlignedBlock::new();

    disk.crash().unwrap();
    disk.reboot().unwrap();

    // Proves that a reboot on a "clean" disk doesn't corrupt the zeroed state
    disk.read_block(0, &mut read_buffer.data).unwrap();
    assert_eq!(read_buffer.data, [0u8; 4096]);
}

#[test]
fn resize_to_same_size_is_noop() {
    let mut disk = ChaosDisk::new(4);
    let mut write_buffer = AlignedBlock::new();
    let mut read_buffer = AlignedBlock::new();

    write_buffer.data[0..5].copy_from_slice(b"hello");
    disk.write_block(0, &write_buffer.data).unwrap();
    disk.flush().unwrap();

    // Resizing to current capacity should not lose data or crash
    disk.resize(4).unwrap();

    disk.read_block(0, &mut read_buffer.data).unwrap();
    assert_eq!(&read_buffer.data[0..5], b"hello");
}
