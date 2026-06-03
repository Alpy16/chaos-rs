pub mod device;
pub mod disk;
pub mod wal;

pub use device::{AdminControls, BlockDevice, BlockDeviceError};
pub use disk::{AlignedBlock, ChaosDisk, FaultPolicy, FaultTrigger, TriggerCondition};
pub use wal::{LogHeader, WalManager, WalError, RecoveryReport};

/// A high-level orchestration harness for crash-consistency testing.
///
/// It executes a workload, performs a simulated hardware crash (wiping volatile state),
/// reboots the device (restoring from stable storage), and then runs a verification closure.
/// Returns a tuple of (Workload Result, Crash/Reboot Result).
pub fn run_crash_test<W, V>(
    capacity_blocks: usize,
    workload: W,
    verify: V,
) -> (Result<(), BlockDeviceError>, Result<(), BlockDeviceError>)
where
    W: FnOnce(&mut ChaosDisk) -> Result<(), BlockDeviceError>,
    V: FnOnce(&mut ChaosDisk),
{
    let mut disk = ChaosDisk::new(capacity_blocks);

    // Capture the result of the workload phase
    let workload_result = workload(&mut disk);

    // Simulate the crash/reboot cycle regardless of workload success
    let crash_result = disk.crash().and_then(|_| disk.reboot());

    verify(&mut disk);

    (workload_result, crash_result)
}
