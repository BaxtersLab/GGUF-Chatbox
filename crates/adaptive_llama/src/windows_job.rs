#[cfg(windows)]
pub mod windows_job {
    use once_cell::sync::OnceCell;
    use std::io;
    use std::mem::zeroed;
    use std::ptr::null_mut;
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;

    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::JobObjects::{
        CreateJobObjectA,
        SetInformationJobObject,
        AssignProcessToJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectExtendedLimitInformation,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    static JOB_HANDLE: OnceCell<HANDLE> = OnceCell::new();

    fn create_job() -> io::Result<HANDLE> {
        unsafe {
            let h = CreateJobObjectA(null_mut(), null_mut());
            if h == 0 {
                return Err(io::Error::last_os_error());
            }

            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
            // set the kill-on-close flag
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

            let ok = SetInformationJobObject(
                h,
                JobObjectExtendedLimitInformation,
                &mut info as *mut _ as *mut _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }

            Ok(h)
        }
    }

    pub fn get_or_init_job() -> io::Result<HANDLE> {
        if let Some(&h) = JOB_HANDLE.get() {
            return Ok(h);
        }
        let h = create_job()?;
        JOB_HANDLE.set(h).map_err(|_| io::Error::new(io::ErrorKind::Other, "job already set"))?;
        Ok(h)
    }

    pub fn assign_child_to_job(child: &Child) -> io::Result<()> {
        unsafe {
            let hjob = get_or_init_job()?;
            // Child exposes a raw handle on Windows
            let raw = child.as_raw_handle();
            let ok = AssignProcessToJobObject(hjob, raw as HANDLE);
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
    }
}
