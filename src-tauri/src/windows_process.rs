//! Windows core processes belong to a private job from process creation onward.
//! Closing its non-inheritable handle (including on application termination)
//! kills the core and its descendants. No suspended, unassigned child can leak.
use crate::lifecycle::{Purpose, PROCESS_LIFECYCLE};
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, Read};
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::os::windows::process::ExitStatusExt;
use std::process::{Command, ExitStatus, Output};
use windows_sys::Win32::Foundation::{
    DuplicateHandle, SetHandleInformation, DUPLICATE_SAME_ACCESS, HANDLE, HANDLE_FLAG_INHERIT,
    WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::System::JobObjects::{
    CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
    TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, GetCurrentProcess, GetExitCodeProcess,
    InitializeProcThreadAttributeList, UpdateProcThreadAttribute, WaitForSingleObject,
    CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, EXTENDED_STARTUPINFO_PRESENT,
    LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
    PROC_THREAD_ATTRIBUTE_JOB_LIST, STARTF_USESTDHANDLES, STARTUPINFOEXW,
};

pub(super) struct Child {
    // Keep the job first so drop closes it before releasing process/pipe handles.
    job: OwnedHandle,
    process: OwnedHandle,
    pid: u32,
    pub stdout: Option<File>,
    pub stderr: Option<File>,
}

impl Child {
    pub fn id(&self) -> u32 {
        self.pid
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.wait_timeout(0)
    }

    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        self.wait_timeout(2_000)?
            .ok_or_else(|| io::Error::other("process wait timed out"))
    }

    fn wait_timeout(&self, timeout: u32) -> io::Result<Option<ExitStatus>> {
        // SAFETY: this object owns a live process handle for the duration of the call.
        match unsafe { WaitForSingleObject(self.process.as_raw_handle(), timeout) } {
            WAIT_OBJECT_0 => {
                let mut code = 0;
                // SAFETY: code is a writable DWORD and process is a valid handle.
                if unsafe { GetExitCodeProcess(self.process.as_raw_handle(), &mut code) } == 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(Some(ExitStatus::from_raw(code)))
            }
            WAIT_TIMEOUT => Ok(None),
            _ => Err(io::Error::last_os_error()),
        }
    }

    pub fn kill(&mut self) -> io::Result<()> {
        // Stop the whole process tree, including children retaining log handles.
        // SAFETY: job is an owned job handle created by create_job.
        if unsafe { TerminateJobObject(self.job.as_raw_handle(), 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

/// Spawn a native core executable using arguments/environment/cwd from Command.
/// Stdin is always NUL; output is either two pipes or the supplied validation log.
/// Command's stdio/creation flags are deliberately not used by this private API.
#[cfg(test)]
pub(super) fn spawn(command: &Command, log: Option<&File>) -> io::Result<Child> {
    spawn_for(command, log, Purpose::Runtime)
}

pub(super) fn spawn_for(
    command: &Command,
    log: Option<&File>,
    purpose: Purpose,
) -> io::Result<Child> {
    let job = create_job()?;
    let stdin = inheritable_duplicate(&File::open("NUL")?)?;
    let (stdout, stdout_writer) = output_handle(log)?;
    let (stderr, stderr_writer) = output_handle(log)?;
    let inherited = [
        stdin.as_raw_handle(),
        stdout_writer.as_raw_handle(),
        stderr_writer.as_raw_handle(),
    ];
    let jobs = [job.as_raw_handle()];
    // Both borrowed arrays and all their handles outlive the attribute list.
    let mut attributes = AttributeList::new(&jobs, &inherited)?;
    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = inherited[0];
    startup.StartupInfo.hStdOutput = inherited[1];
    startup.StartupInfo.hStdError = inherited[2];
    startup.lpAttributeList = attributes.as_mut_ptr();

    // All runtime callers resolve an actual executable. Canonicalization avoids
    // CreateProcess search-path ambiguities and supports spaces/Unicode in paths.
    let executable = std::fs::canonicalize(command.get_program())?;
    let application = wide_nul(executable.as_os_str())?;
    let mut command_line = quoted_argument(executable.as_os_str())?;
    for argument in command.get_args() {
        command_line.push(b' ' as u16);
        command_line.extend(quoted_argument(argument)?);
    }
    command_line.push(0);
    let environment = environment_block(command)?;
    let cwd = command
        .get_current_dir()
        .map(|value| wide_nul(value.as_os_str()))
        .transpose()?;
    let mut info = PROCESS_INFORMATION::default();
    // SAFETY: all strings are NUL-terminated, the command line is writable, and
    // STARTUPINFOEX/attribute buffers remain valid through CreateProcessW. The
    // handle list explicitly includes only stdio; the job is never inherited.
    // JOB_LIST binds before the first instruction, unlike post-spawn assignment.
    let admission = crate::lifecycle::admission(purpose)?;
    if unsafe {
        CreateProcessW(
            application.as_ptr(),
            command_line.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1,
            CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT,
            environment
                .as_ref()
                .map_or(std::ptr::null(), |value| value.as_ptr().cast()),
            cwd.as_ref()
                .map_or(std::ptr::null(), |value| value.as_ptr()),
            &startup.StartupInfo,
            &mut info,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    drop(admission);
    crate::app_log::record(
        0,
        crate::app_log::Area::Runtime,
        &format!(
            "core_process purpose={} action=spawn pid={}",
            purpose.label(),
            info.dwProcessId
        ),
    );
    // SAFETY: successful CreateProcessW returns new process and thread handles.
    let process = unsafe { OwnedHandle::from_raw_handle(info.hProcess) };
    let _thread = unsafe { OwnedHandle::from_raw_handle(info.hThread) };
    Ok(Child {
        job,
        process,
        pid: info.dwProcessId,
        stdout,
        stderr,
    })
}

/// Bounded version probe; it receives the same job protection as the core and
/// configuration validator. Pipes are drained concurrently to avoid deadlocks.
pub(super) fn output(command: &Command) -> io::Result<Output> {
    let mut child = spawn_for(command, None, Purpose::Version)?;
    let read = |pipe: Option<File>| {
        std::thread::spawn(move || -> io::Result<Vec<u8>> {
            let mut bytes = Vec::new();
            if let Some(pipe) = pipe {
                pipe.take(32 * 1024).read_to_end(&mut bytes)?;
            }
            Ok(bytes)
        })
    };
    let stdout = read(child.stdout.take());
    let stderr = read(child.stderr.take());
    let started = std::time::Instant::now();
    let status = loop {
        if !PROCESS_LIFECYCLE.admitting() || started.elapsed() >= std::time::Duration::from_secs(5)
        {
            break None;
        }
        if let Some(status) = child.wait_timeout(20)? {
            break Some(status);
        }
    };
    // Terminate descendants too before joining readers: they may retain a pipe
    // after the probed executable has already exited.
    child.kill()?;
    child.wait()?;
    let stdout = stdout
        .join()
        .map_err(|_| io::Error::other("stdout reader failed"))??;
    let stderr = stderr
        .join()
        .map_err(|_| io::Error::other("stderr reader failed"))??;
    match status {
        Some(status) => Ok(Output {
            status,
            stdout,
            stderr,
        }),
        None => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "Mihomo version probe timed out",
        )),
    }
}

fn create_job() -> io::Result<OwnedHandle> {
    // Null security attributes make this handle non-inheritable.
    // SAFETY: nullable inputs are accepted; there is no borrowed name buffer.
    let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if raw.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful CreateJobObjectW transfers ownership of a new handle.
    let job = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    // SAFETY: limits matches the requested information class and length.
    if unsafe {
        SetInformationJobObject(
            job.as_raw_handle(),
            JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(job)
}

fn inheritable_duplicate(handle: &impl AsRawHandle) -> io::Result<OwnedHandle> {
    let mut duplicate = std::ptr::null_mut();
    // SAFETY: the input handle remains live; duplicate receives a new owned handle.
    if unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            handle.as_raw_handle(),
            GetCurrentProcess(),
            &mut duplicate,
            0,
            1,
            DUPLICATE_SAME_ACCESS,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: DuplicateHandle succeeded and returned an owned handle.
    Ok(unsafe { OwnedHandle::from_raw_handle(duplicate) })
}

fn output_handle(log: Option<&File>) -> io::Result<(Option<File>, OwnedHandle)> {
    if let Some(log) = log {
        return Ok((None, inheritable_duplicate(log)?));
    }
    let mut reader = std::ptr::null_mut();
    let mut writer = std::ptr::null_mut();
    // SAFETY: writable out parameters; null attributes produce non-inheritable handles.
    if unsafe { CreatePipe(&mut reader, &mut writer, std::ptr::null(), 0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful CreatePipe returns two new owned handles.
    let reader = unsafe { OwnedHandle::from_raw_handle(reader) };
    let writer = unsafe { OwnedHandle::from_raw_handle(writer) };
    // SAFETY: writer is a live pipe handle. Reader stays non-inheritable.
    if unsafe {
        SetHandleInformation(
            writer.as_raw_handle(),
            HANDLE_FLAG_INHERIT,
            HANDLE_FLAG_INHERIT,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok((Some(File::from(reader)), writer))
}

struct AttributeList<'a> {
    // usize alignment is sufficient for the native attribute list's pointers.
    storage: Vec<usize>,
    _handles: std::marker::PhantomData<&'a [HANDLE]>,
}

impl<'a> AttributeList<'a> {
    fn new(jobs: &'a [HANDLE], handles: &'a [HANDLE]) -> io::Result<Self> {
        let mut bytes = 0;
        // SAFETY: first sizing call intentionally passes a null buffer.
        unsafe {
            InitializeProcThreadAttributeList(std::ptr::null_mut(), 2, 0, &mut bytes);
        }
        if bytes == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut storage = vec![0usize; bytes.div_ceil(size_of::<usize>())];
        // SAFETY: storage is aligned and covers the size returned by Windows.
        if unsafe {
            InitializeProcThreadAttributeList(storage.as_mut_ptr().cast(), 2, 0, &mut bytes)
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let mut list = Self {
            storage,
            _handles: std::marker::PhantomData,
        };
        for (attribute, values) in [
            (PROC_THREAD_ATTRIBUTE_JOB_LIST, jobs),
            (PROC_THREAD_ATTRIBUTE_HANDLE_LIST, handles),
        ] {
            // SAFETY: values lives for 'a, and the list cannot outlive either slice.
            if unsafe {
                UpdateProcThreadAttribute(
                    list.as_mut_ptr(),
                    0,
                    attribute as usize,
                    values.as_ptr().cast(),
                    std::mem::size_of_val(values),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(list)
    }

    fn as_mut_ptr(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.storage.as_mut_ptr().cast()
    }
}

impl Drop for AttributeList<'_> {
    fn drop(&mut self) {
        // SAFETY: successful construction initialized this list; storage is still live.
        unsafe {
            DeleteProcThreadAttributeList(self.as_mut_ptr());
        }
    }
}

fn wide_nul(value: &OsStr) -> io::Result<Vec<u16>> {
    let mut wide: Vec<u16> = value.encode_wide().collect();
    if wide.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "NUL in process argument",
        ));
    }
    wide.push(0);
    Ok(wide)
}

// Quote every argument according to the native Windows argv rules, including
// empty arguments, embedded quotes and backslashes before a quote or the end.
fn quoted_argument(value: &OsStr) -> io::Result<Vec<u16>> {
    let wide = wide_nul(value)?;
    let mut quoted = vec![b'"' as u16];
    let mut slashes = 0;
    for &unit in &wide[..wide.len() - 1] {
        if unit == b'\\' as u16 {
            slashes += 1;
            continue;
        }
        quoted.extend(std::iter::repeat_n(
            b'\\' as u16,
            if unit == b'"' as u16 {
                slashes * 2 + 1
            } else {
                slashes
            },
        ));
        slashes = 0;
        quoted.push(unit);
    }
    quoted.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2));
    quoted.push(b'"' as u16);
    Ok(quoted)
}

fn environment_block(command: &Command) -> io::Result<Option<Vec<u16>>> {
    if command.get_envs().len() == 0 {
        return Ok(None);
    }
    // Runtime commands never use env_clear. Support explicit test/child overrides
    // while preserving Windows' case-insensitive environment keys and sort order.
    let key = |name: &OsStr| name.to_string_lossy().to_uppercase();
    let mut variables: BTreeMap<String, (OsString, OsString)> = std::env::vars_os()
        .map(|(name, value)| (key(&name), (name, value)))
        .collect();
    for (name, value) in command.get_envs() {
        match value {
            Some(value) => {
                variables.insert(key(name), (name.to_os_string(), value.to_os_string()));
            }
            None => {
                variables.remove(&key(name));
            }
        }
    }
    let mut block = Vec::new();
    for (_, (name, value)) in variables {
        let mut entry = name;
        entry.push("=");
        entry.push(value);
        block.extend(wide_nul(&entry)?);
    }
    if block.is_empty() {
        block.push(0);
    }
    block.push(0);
    Ok(Some(block))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::windows::process::CommandExt;
    use std::time::{Duration, Instant};
    use windows_sys::Win32::System::JobObjects::IsProcessInJob;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    };

    const FIXTURE_ENV: &str = "MIHOMO_WINDOWS_JOB_FIXTURE";
    const QUOTING_CASES: &[&str] = &[
        "",
        "two words",
        "a\"b",
        "tail\\",
        "before\\\"quote",
        "C:\\中文 目录\\",
    ];

    fn fixture_command(mode: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().expect("test executable"));
        // libtest names exclude the crate prefix returned by module_path!().
        let module = module_path!()
            .split_once("::")
            .expect("nested test module")
            .1;
        let fixture = format!("{module}::process_fixture");
        command.args(["--exact", &fixture, "--nocapture"]);
        command.env(FIXTURE_ENV, mode);
        command
    }

    #[test]
    fn process_fixture() {
        match std::env::var(FIXTURE_ENV).as_deref() {
            Ok("gate") => {
                let lifecycle = &crate::lifecycle::PROCESS_LIFECYCLE;
                assert!(lifecycle.prepare());
                for purpose in [Purpose::Runtime, Purpose::Version, Purpose::Validation] {
                    assert!(spawn_for(&fixture_command("sleep"), None, purpose).is_err());
                }
                assert!(lifecycle.cancel());
                drop(spawn_for(&fixture_command("sleep"), None, Purpose::Runtime).unwrap());
                lifecycle.close();
                for purpose in [Purpose::Runtime, Purpose::Version, Purpose::Validation] {
                    assert!(spawn_for(&fixture_command("sleep"), None, purpose).is_err());
                }
                println!("admission-fixture-ok");
            }
            Ok("sleep") => loop {
                std::thread::sleep(Duration::from_secs(1));
            },
            Ok("argv") => {
                let args: Vec<_> = std::env::args().collect();
                for (index, expected) in QUOTING_CASES.iter().enumerate() {
                    assert_eq!(&args[5 + index], expected);
                }
                let mut in_job = 0;
                // SAFETY: current process is valid; output is a writable BOOL.
                assert_ne!(
                    unsafe {
                        IsProcessInJob(GetCurrentProcess(), std::ptr::null_mut(), &mut in_job)
                    },
                    0
                );
                assert_ne!(in_job, 0, "fixture starts inside a job");
                println!("argv-and-job-ok");
            }
            Ok("owner") => {
                let directory = std::path::PathBuf::from(
                    std::env::var_os("MIHOMO_WINDOWS_JOB_DIRECTORY").expect("fixture directory"),
                );
                let child = spawn(&fixture_command("sleep"), None).expect("managed descendant");
                std::fs::write(directory.join("pid.tmp"), child.id().to_string())
                    .expect("publish child pid");
                std::fs::rename(directory.join("pid.tmp"), directory.join("pid"))
                    .expect("atomically publish child pid");
                let started = Instant::now();
                while !directory.join("exit").exists() {
                    assert!(
                        started.elapsed() < Duration::from_secs(15),
                        "owner fixture timed out"
                    );
                    std::thread::sleep(Duration::from_millis(10));
                }
                // Deliberately bypass Rust destructors: Windows must close the
                // job handle itself, as it does when the app crashes/is killed.
                std::process::exit(0);
            }
            _ => {}
        }
    }

    #[test]
    fn actual_windows_spawn_rejects_all_roles_during_session_end() {
        let result = output(&fixture_command("gate")).unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(String::from_utf8_lossy(&result.stdout).contains("admission-fixture-ok"));
    }

    #[test]
    fn native_spawn_preserves_argv_and_starts_inside_job() {
        let mut command = fixture_command("argv");
        command.arg("--");
        for value in QUOTING_CASES {
            command.arg(value);
        }
        let output = output(&command).expect("run argument fixture");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("argv-and-job-ok"));
    }

    #[test]
    fn dropping_job_kills_a_running_child_without_admin_rights() {
        let mut child = spawn(&fixture_command("sleep"), None).expect("managed fixture");
        let mut in_job = 0;
        // SAFETY: both handles are owned by child and output is a writable BOOL.
        assert_ne!(
            unsafe {
                IsProcessInJob(
                    child.process.as_raw_handle(),
                    child.job.as_raw_handle(),
                    &mut in_job,
                )
            },
            0
        );
        assert_ne!(in_job, 0, "child belongs to its private job");
        assert!(child.try_wait().expect("running child").is_none());
        let process = child.process.try_clone().expect("observer handle");
        drop(child);
        // SAFETY: process is an owned observer handle kept after Child was dropped.
        assert_eq!(
            unsafe { WaitForSingleObject(process.as_raw_handle(), 5_000) },
            WAIT_OBJECT_0
        );
    }

    struct OwnerGuard(std::process::Child);
    impl Drop for OwnerGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[test]
    fn owner_exit_without_destructors_kills_its_managed_core() {
        let directory = std::env::temp_dir().join(format!(
            "mihomo-job-fixture-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        std::fs::create_dir(&directory).expect("fixture directory");
        let mut command = fixture_command("owner");
        command
            .env("MIHOMO_WINDOWS_JOB_DIRECTORY", &directory)
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        // The outer test process is intentionally unmanaged to reproduce the
        // application owner's lifetime independently of the job under test.
        let mut owner = OwnerGuard(command.spawn().expect("owner fixture"));
        let started = Instant::now();
        let pid_path = directory.join("pid");
        while !pid_path.exists() {
            assert!(
                owner.0.try_wait().expect("owner state").is_none(),
                "owner exited before publishing its child"
            );
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "owner did not publish child pid"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let pid = std::fs::read_to_string(&pid_path)
            .expect("child pid")
            .parse::<u32>()
            .expect("numeric pid");
        // SAFETY: read-only process query and synchronization access; no elevation required.
        let raw = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                0,
                pid,
            )
        };
        assert!(!raw.is_null(), "{}", io::Error::last_os_error());
        // SAFETY: OpenProcess succeeded and transferred a new owned handle.
        let process = unsafe { OwnedHandle::from_raw_handle(raw) };
        File::create(directory.join("exit"))
            .expect("owner exit signal")
            .write_all(b"exit")
            .unwrap();
        // SAFETY: observer handle remains valid even after the target exits.
        assert_eq!(
            unsafe { WaitForSingleObject(process.as_raw_handle(), 5_000) },
            WAIT_OBJECT_0,
            "OS handle cleanup must stop core even when Rust destructors never run"
        );
        assert!(owner.0.wait().expect("owner exited").success());
        std::fs::remove_file(pid_path).expect("remove fixture pid");
        std::fs::remove_file(directory.join("exit")).expect("remove fixture signal");
        std::fs::remove_dir(directory).expect("remove empty fixture directory");
    }

    #[test]
    fn validation_output_uses_the_supplied_file() {
        let path = std::env::temp_dir().join(format!("mihomo-job-log-{}.txt", std::process::id()));
        let mut log = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .expect("log fixture");
        let mut command = fixture_command("argv");
        command.arg("--");
        for value in QUOTING_CASES {
            command.arg(value);
        }
        let child = spawn(&command, Some(&log)).expect("child with file output");
        assert!(child.stdout.is_none() && child.stderr.is_none());
        assert!(child
            .wait_timeout(5_000)
            .expect("wait")
            .expect("child exits")
            .success());
        drop(child);
        use std::io::{Seek, SeekFrom};
        log.seek(SeekFrom::Start(0)).unwrap();
        let mut text = String::new();
        log.read_to_string(&mut text).unwrap();
        assert!(text.contains("argv-and-job-ok"));
        drop(log);
        std::fs::remove_file(path).expect("log handles released");
    }
}
