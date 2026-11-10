"""List every top-level window whose title matches YOLO, with its owning process image.

Written after a false reading: FindWindowW("YOLO Trainer") returned a 136x39 window at
(0,0) instead of the real frame, so a geometry check must see all candidates — and which
process owns them — rather than trusting the first title match.

No subprocess calls: tasklist writes GBK on this machine and breaks under UTF-8 decoding,
so the owner image name is resolved through the Win32 API instead.

Run:  python spikes/qa/list_windows.py [substring]
"""

import ctypes
import sys
from ctypes import wintypes

user32 = ctypes.WinDLL("user32", use_last_error=True)
kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
EnumProc = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
PROCESS_QUERY_LIMITED_INFORMATION = 0x1000


def image_name(pid: int) -> str:
    handle = kernel32.OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, False, pid)
    if not handle:
        return "?"
    try:
        size = wintypes.DWORD(1024)
        buf = ctypes.create_unicode_buffer(size.value)
        if kernel32.QueryFullProcessImageNameW(handle, 0, buf, ctypes.byref(size)):
            return buf.value.rsplit("\\", 1)[-1]
        return "?"
    finally:
        kernel32.CloseHandle(handle)


def main() -> None:
    needle = (sys.argv[1] if len(sys.argv) > 1 else "YOLO").lower()

    def callback(hwnd, _):
        length = user32.GetWindowTextLengthW(hwnd)
        if length:
            title = ctypes.create_unicode_buffer(length + 1)
            user32.GetWindowTextW(hwnd, title, length + 1)
            if needle in title.value.lower():
                pid = wintypes.DWORD()
                user32.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
                cls = ctypes.create_unicode_buffer(256)
                user32.GetClassNameW(hwnd, cls, 256)
                rect = wintypes.RECT()
                user32.GetWindowRect(hwnd, ctypes.byref(rect))
                print(
                    f"hwnd={hwnd} pid={pid.value} ({image_name(pid.value)}) "
                    f"visible={bool(user32.IsWindowVisible(hwnd))} class={cls.value!r} "
                    f"title={title.value!r} "
                    f"pos=({rect.left},{rect.top}) size={rect.right - rect.left}x{rect.bottom - rect.top}"
                )
        return True

    user32.EnumWindows(EnumProc(callback), 0)


if __name__ == "__main__":
    main()
