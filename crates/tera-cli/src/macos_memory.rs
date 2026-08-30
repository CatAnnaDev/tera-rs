use anyhow::{bail, Result};
use mach2::kern_return::KERN_SUCCESS;
use mach2::message::mach_msg_type_number_t;
use mach2::port::mach_port_t;
use mach2::traps::{mach_task_self, task_for_pid};
use mach2::vm::{mach_vm_read_overwrite, mach_vm_region};
use mach2::vm_prot::VM_PROT_READ;
use mach2::vm_region::{vm_region_basic_info_data_64_t, vm_region_info_t, VM_REGION_BASIC_INFO_64};
use mach2::vm_types::{mach_vm_address_t, mach_vm_size_t};

const MAX_REGION_BYTES: u64 = 512 * 1024 * 1024;

pub struct Region {
    pub address: u64,
    pub protection: i32,
    pub bytes: Vec<u8>,
}

impl Region {
    pub fn executable(&self) -> bool {
        self.protection & mach2::vm_prot::VM_PROT_EXECUTE != 0
    }

    pub fn writable(&self) -> bool {
        self.protection & mach2::vm_prot::VM_PROT_WRITE != 0
    }

    pub fn flags(&self) -> String {
        let mut out = String::new();
        out.push(if self.protection & VM_PROT_READ != 0 { 'r' } else { '-' });
        out.push(if self.writable() { 'w' } else { '-' });
        out.push(if self.executable() { 'x' } else { '-' });
        out
    }
}

pub fn read_all(pid: i32) -> Result<Vec<(u64, Vec<u8>)>> {
    Ok(regions(pid)?
        .into_iter()
        .map(|region| (region.address, region.bytes))
        .collect())
}

pub fn regions(pid: i32) -> Result<Vec<Region>> {
    unsafe {
        let mut task: mach_port_t = 0;
        let status = task_for_pid(mach_task_self(), pid, &mut task);
        if status != KERN_SUCCESS {
            bail!(
                "task_for_pid({pid}) failed with {status}; run this command with sudo, or dump the \
                 process with `lldb -p {pid} -o \"process save-core dump.core\" -o detach -b` and \
                 scan the dump file instead"
            );
        }
        let mut regions = Vec::new();
        let mut address: mach_vm_address_t = 1;
        loop {
            let mut size: mach_vm_size_t = 0;
            let mut info: vm_region_basic_info_data_64_t = std::mem::zeroed();
            let mut count = (std::mem::size_of::<vm_region_basic_info_data_64_t>()
                / std::mem::size_of::<i32>()) as mach_msg_type_number_t;
            let mut object: mach_port_t = 0;
            let status = mach_vm_region(
                task,
                &mut address,
                &mut size,
                VM_REGION_BASIC_INFO_64,
                &mut info as *mut _ as vm_region_info_t,
                &mut count,
                &mut object,
            );
            if status != KERN_SUCCESS {
                break;
            }
            if info.protection & VM_PROT_READ != 0 && size > 0 && size <= MAX_REGION_BYTES {
                let mut buffer = vec![0u8; size as usize];
                let mut read: mach_vm_size_t = 0;
                let status = mach_vm_read_overwrite(
                    task,
                    address,
                    size,
                    buffer.as_mut_ptr() as mach_vm_address_t,
                    &mut read,
                );
                if status == KERN_SUCCESS && read > 0 {
                    buffer.truncate(read as usize);
                    regions.push(Region {
                        address,
                        protection: info.protection,
                        bytes: buffer,
                    });
                }
            }
            address = match address.checked_add(size.max(1)) {
                Some(next) => next,
                None => break,
            };
        }
        Ok(regions)
    }
}
