# Isolierter WH_KEYBOARD_LL-Test ohne handy-keys:
# Installiert einen minimalen Hook, sendet selbst Strg+Alt+Space per keybd_event,
# pumpt 6 s Messages und zaehlt, was im Hook ankam.
$ErrorActionPreference = "Stop"
$out = "C:\Users\rrhau\projects\hook-probe-result.txt"
"log start $(Get-Date -Format T)" | Set-Content $out

Add-Type -TypeDefinition @'
using System;
using System.IO;
using System.Runtime.InteropServices;

public class HookProbe {
    [DllImport("user32.dll")] static extern IntPtr SetWindowsHookExW(int id, HookProc proc, IntPtr mod, uint tid);
    [DllImport("user32.dll")] static extern int CallNextHookEx(IntPtr hhk, int code, IntPtr wp, IntPtr lp);
    [DllImport("user32.dll")] static extern bool UnhookWindowsHookEx(IntPtr hhk);
    [DllImport("user32.dll")] static extern bool PeekMessage(out MSG msg, IntPtr hwnd, uint min, uint max, uint remove);
    [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
    [DllImport("kernel32.dll")] static extern uint GetCurrentThreadId();

    delegate int HookProc(int code, IntPtr wp, IntPtr lp);
    static HookProc _proc = Proc;
    static IntPtr _hhk;
    static string _file = @"C:\Users\rrhau\projects\hook-probe-result.txt";
    static int _count = 0;

    static void Log(string s) { try { File.AppendAllText(_file, s + "\r\n"); } catch {} }

    static int Proc(int code, IntPtr wp, IntPtr lp) {
        if (code >= 0) {
            int vk = Marshal.ReadInt32(lp);
            int flags = Marshal.ReadInt32(lp, 12);
            int msg = wp.ToInt32();
            _count++;
            Log(String.Format("hook #{0}: vk=0x{1:X} msg={2} injected={3}", _count, vk, msg, (flags & 0x10) != 0));
        }
        return CallNextHookEx(_hhk, code, wp, lp);
    }

    public static void Run() {
        _hhk = SetWindowsHookExW(13, _proc, IntPtr.Zero, 0); // 13 = WH_KEYBOARD_LL
        if (_hhk == IntPtr.Zero) { Log("HOOK INSTALL FEHLGESCHLAGEN: " + Marshal.GetLastWin32Error()); return; }
        Log("hook installiert, tid=" + GetCurrentThreadId());

        System.Threading.Thread.Sleep(500);
        // Strg+Alt+Space druecken und loslassen (selbst injiziert)
        keybd_event(0x11, 0, 0, UIntPtr.Zero);
        keybd_event(0x12, 0, 0, UIntPtr.Zero);
        keybd_event(0x20, 0, 0, UIntPtr.Zero);
        System.Threading.Thread.Sleep(600);
        keybd_event(0x20, 0, 2, UIntPtr.Zero);
        keybd_event(0x12, 0, 2, UIntPtr.Zero);
        keybd_event(0x11, 0, 2, UIntPtr.Zero);
        Log("sendinput sequenz gesendet");

        MSG m;
        var end = DateTime.UtcNow.AddSeconds(6);
        while (DateTime.UtcNow < end) {
            while (PeekMessage(out m, IntPtr.Zero, 0, 0, 1)) { System.Threading.Thread.Sleep(1); }
            System.Threading.Thread.Sleep(50);
        }
        UnhookWindowsHookEx(_hhk);
        Log("ende, events gesamt=" + _count);
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct MSG { public IntPtr hwnd; public uint message; public IntPtr wParam; public IntPtr lParam; public uint time; public int ptX; public int ptY; }
}
'@

[HookProbe]::Run()
Get-Content $out