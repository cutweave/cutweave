#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Run a command on a separate Win32 desktop, so its windows never show on, or take the focus
of, the desktop someone is working on (issue #211).

    python scripts/hidden-desktop.py -- <command> [args...]

The user-flow suite launches the real app about 65 times per run; every window comes to the
front and takes the keyboard. On the working desktop that sends a person's typing and clicks
into test windows (and their input breaks flows). `just e2e-flows` therefore starts the suite
through this script.

* A Win32 desktop (`CreateDesktop`) is not a Task View "virtual desktop": it has its own
  windows, focus and input queue. The command and every process it starts live on it; the GPU,
  the preview and exports work as usual. The desktop goes away when its last process exits.
* The command's output is written to a temp file and copied to this script's own output while
  it runs, so logs and gates read it as before. The exit code is the command's.
* Everything the command starts runs in a job object that is killed when this script exits, also
  when it is itself killed or interrupted: no app or engine is ever left behind on a desktop
  nobody can see. (Processes that ask to break away from the job, as Chromium may, are allowed
  to; they exit with the app that started them.)
* `CUTWEAVE_E2E_VISIBLE=1` runs the command on the working desktop instead (to watch a flow).
  Not on Windows, the command simply runs: other systems use their own headless displays.
* The real-cursor steps (`just e2e-real-input`) do not go through this: they move the real
  mouse on the real desktop by design.
"""

import ctypes
import os
import subprocess
import sys
import tempfile
import time

DESKTOP = "cutweave-e2e"


def run_visible(command: list[str]) -> int:
    return subprocess.call(command, shell=(os.name == "nt"))


def run_hidden(command: list[str]) -> int:
    import msvcrt
    from ctypes import wintypes

    class StartupInfo(ctypes.Structure):
        _fields_ = [
            ("cb", wintypes.DWORD),
            ("lpReserved", wintypes.LPWSTR),
            ("lpDesktop", wintypes.LPWSTR),
            ("lpTitle", wintypes.LPWSTR),
            ("dwX", wintypes.DWORD),
            ("dwY", wintypes.DWORD),
            ("dwXSize", wintypes.DWORD),
            ("dwYSize", wintypes.DWORD),
            ("dwXCountChars", wintypes.DWORD),
            ("dwYCountChars", wintypes.DWORD),
            ("dwFillAttribute", wintypes.DWORD),
            ("dwFlags", wintypes.DWORD),
            ("wShowWindow", wintypes.WORD),
            ("cbReserved2", wintypes.WORD),
            ("lpReserved2", ctypes.c_void_p),
            ("hStdInput", wintypes.HANDLE),
            ("hStdOutput", wintypes.HANDLE),
            ("hStdError", wintypes.HANDLE),
        ]

    class ProcessInformation(ctypes.Structure):
        _fields_ = [
            ("hProcess", wintypes.HANDLE),
            ("hThread", wintypes.HANDLE),
            ("dwProcessId", wintypes.DWORD),
            ("dwThreadId", wintypes.DWORD),
        ]

    class IoCounters(ctypes.Structure):
        _fields_ = [
            (name, ctypes.c_ulonglong)
            for name in (
                "ReadOperationCount",
                "WriteOperationCount",
                "OtherOperationCount",
                "ReadTransferCount",
                "WriteTransferCount",
                "OtherTransferCount",
            )
        ]

    class BasicLimits(ctypes.Structure):
        _fields_ = [
            ("PerProcessUserTimeLimit", ctypes.c_int64),
            ("PerJobUserTimeLimit", ctypes.c_int64),
            ("LimitFlags", wintypes.DWORD),
            ("MinimumWorkingSetSize", ctypes.c_size_t),
            ("MaximumWorkingSetSize", ctypes.c_size_t),
            ("ActiveProcessLimit", wintypes.DWORD),
            ("Affinity", ctypes.c_size_t),
            ("PriorityClass", wintypes.DWORD),
            ("SchedulingClass", wintypes.DWORD),
        ]

    class ExtendedLimits(ctypes.Structure):
        _fields_ = [
            ("BasicLimitInformation", BasicLimits),
            ("IoInfo", IoCounters),
            ("ProcessMemoryLimit", ctypes.c_size_t),
            ("JobMemoryLimit", ctypes.c_size_t),
            ("PeakProcessMemoryUsed", ctypes.c_size_t),
            ("PeakJobMemoryUsed", ctypes.c_size_t),
        ]

    generic_all = 0x10000000
    startf_usestdhandles = 0x00000100
    create_unicode_environment = 0x00000400
    create_suspended = 0x00000004
    still_active = 259
    job_object_extended_limit_information = 9
    job_object_limit_kill_on_job_close = 0x00002000
    job_object_limit_breakaway_ok = 0x00000800

    user32 = ctypes.WinDLL("user32", use_last_error=True)
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    user32.CreateDesktopW.restype = wintypes.HANDLE
    user32.CreateDesktopW.argtypes = [
        wintypes.LPCWSTR,
        wintypes.LPCWSTR,
        ctypes.c_void_p,
        wintypes.DWORD,
        wintypes.DWORD,
        ctypes.c_void_p,
    ]
    user32.CloseDesktop.argtypes = [wintypes.HANDLE]
    kernel32.CreateProcessW.argtypes = [
        wintypes.LPCWSTR,
        wintypes.LPWSTR,
        ctypes.c_void_p,
        ctypes.c_void_p,
        wintypes.BOOL,
        wintypes.DWORD,
        ctypes.c_void_p,
        wintypes.LPCWSTR,
        ctypes.POINTER(StartupInfo),
        ctypes.POINTER(ProcessInformation),
    ]
    kernel32.GetExitCodeProcess.argtypes = [wintypes.HANDLE, ctypes.POINTER(wintypes.DWORD)]
    kernel32.CloseHandle.argtypes = [wintypes.HANDLE]
    kernel32.CreateJobObjectW.restype = wintypes.HANDLE
    kernel32.CreateJobObjectW.argtypes = [ctypes.c_void_p, wintypes.LPCWSTR]
    kernel32.SetInformationJobObject.argtypes = [
        wintypes.HANDLE,
        ctypes.c_int,
        ctypes.c_void_p,
        wintypes.DWORD,
    ]
    kernel32.AssignProcessToJobObject.argtypes = [wintypes.HANDLE, wintypes.HANDLE]
    kernel32.ResumeThread.argtypes = [wintypes.HANDLE]
    kernel32.TerminateProcess.argtypes = [wintypes.HANDLE, wintypes.UINT]

    # Every process the command starts is killed when this handle closes, which the system also
    # does when this script dies.
    job = kernel32.CreateJobObjectW(None, None)
    if job:
        limits = ExtendedLimits()
        limits.BasicLimitInformation.LimitFlags = (
            job_object_limit_kill_on_job_close | job_object_limit_breakaway_ok
        )
        if not kernel32.SetInformationJobObject(
            job,
            job_object_extended_limit_information,
            ctypes.byref(limits),
            ctypes.sizeof(limits),
        ):
            kernel32.CloseHandle(job)
            job = None
    if not job:
        print(
            f"hidden-desktop: no job object ({ctypes.get_last_error()}); "
            "running on the working desktop",
            file=sys.stderr,
        )
        return run_visible(command)

    desktop = user32.CreateDesktopW(DESKTOP, None, None, 0, generic_all, None)
    if not desktop:
        print(
            f"hidden-desktop: CreateDesktop failed ({ctypes.get_last_error()}); "
            "running on the working desktop",
            file=sys.stderr,
        )
        return run_visible(command)

    # The command's output goes to a file this script copies out: handles of a console or of a
    # pipe owned by another tool do not always survive the hop to another desktop.
    fd, log_path = tempfile.mkstemp(prefix="cutweave-flows-", suffix=".log")
    os.set_inheritable(fd, True)
    handle = msvcrt.get_osfhandle(fd)

    startup = StartupInfo()
    startup.cb = ctypes.sizeof(startup)
    startup.lpDesktop = f"WinSta0\\{DESKTOP}"
    startup.dwFlags = startf_usestdhandles
    startup.hStdInput = None
    startup.hStdOutput = handle
    startup.hStdError = handle
    info = ProcessInformation()
    # Through cmd so that `.cmd` shims (pnpm, npx) and PATH lookups work as in a shell.
    line = ctypes.create_unicode_buffer(
        f'cmd.exe /d /s /c "{subprocess.list2cmdline(command)}"'
    )
    started = kernel32.CreateProcessW(
        None,
        line,
        None,
        None,
        True,
        create_unicode_environment | create_suspended,
        None,
        None,
        ctypes.byref(startup),
        ctypes.byref(info),
    )
    os.close(fd)
    if not started:
        print(f"hidden-desktop: CreateProcess failed ({ctypes.get_last_error()})", file=sys.stderr)
        kernel32.CloseHandle(job)
        user32.CloseDesktop(desktop)
        os.unlink(log_path)
        return 1
    if not kernel32.AssignProcessToJobObject(job, info.hProcess):
        print(
            f"hidden-desktop: AssignProcessToJobObject failed ({ctypes.get_last_error()})",
            file=sys.stderr,
        )
        kernel32.TerminateProcess(info.hProcess, 1)
        kernel32.CloseHandle(info.hThread)
        kernel32.CloseHandle(info.hProcess)
        kernel32.CloseHandle(job)
        user32.CloseDesktop(desktop)
        os.unlink(log_path)
        return 1
    kernel32.ResumeThread(info.hThread)

    code = wintypes.DWORD(still_active)
    out = sys.stdout.buffer
    try:
        with open(log_path, "rb") as log:
            while True:
                chunk = log.read(65536)
                if chunk:
                    out.write(chunk)
                    out.flush()
                    continue
                kernel32.GetExitCodeProcess(info.hProcess, ctypes.byref(code))
                if code.value != still_active:
                    rest = log.read()
                    if rest:
                        out.write(rest)
                        out.flush()
                    break
                time.sleep(0.2)
    finally:
        # Closing the job kills whatever the command left running (an app a flow did not quit,
        # or everything, when this script is interrupted).
        kernel32.CloseHandle(job)
        kernel32.CloseHandle(info.hThread)
        kernel32.CloseHandle(info.hProcess)
        user32.CloseDesktop(desktop)
    try:
        os.unlink(log_path)
    except OSError:
        pass
    return int(code.value)


def main() -> int:
    args = sys.argv[1:]
    if not args or args[0] != "--" or len(args) < 2:
        print(__doc__, file=sys.stderr)
        return 2
    command = args[1:]
    if os.name != "nt" or os.environ.get("CUTWEAVE_E2E_VISIBLE") == "1":
        return run_visible(command)
    return run_hidden(command)


if __name__ == "__main__":
    sys.exit(main())
