using System;
using System.ComponentModel;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Threading;

namespace QuotaBarSmoke
{
    // Run in the process which starts the secondary monitor, just like a user
    // launching QuotaBar from a focused desktop application.
    public sealed class ForegroundLauncher : IDisposable
    {
        [StructLayout(LayoutKind.Sequential)]
        private struct Point { public int X, Y; }
        [StructLayout(LayoutKind.Sequential)]
        private struct Message
        {
            public IntPtr Window;
            public uint Id;
            public UIntPtr WParam;
            public IntPtr LParam;
            public uint Time;
            public Point Point;
            public uint Private;
        }
        [StructLayout(LayoutKind.Sequential)]
        private struct GuiThread
        {
            public int Size, Flags;
            public IntPtr Active, Focus, Capture, Menu, Move, Caret;
            public int Left, Top, Right, Bottom;
        }

        [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        private static extern IntPtr CreateWindowExW(uint exStyle, string cls, string title, uint style,
            int x, int y, int w, int h, IntPtr parent, IntPtr menu, IntPtr instance, IntPtr param);
        [DllImport("user32.dll")] private static extern bool DestroyWindow(IntPtr hwnd);
        [DllImport("user32.dll")] private static extern bool SetWindowPos(IntPtr hwnd, IntPtr after, int x, int y, int w, int h, uint flags);
        [DllImport("user32.dll", SetLastError = true)] private static extern bool GetCursorPos(out Point point);
        [DllImport("user32.dll")] private static extern bool SetCursorPos(int x, int y);
        [DllImport("user32.dll")] private static extern IntPtr WindowFromPoint(Point point);
        [DllImport("user32.dll")] private static extern IntPtr SetFocus(IntPtr window);
        [DllImport("user32.dll")] private static extern void mouse_event(uint flags, uint x, uint y, uint data, UIntPtr extra);
        [DllImport("user32.dll")] private static extern IntPtr GetForegroundWindow();
        [DllImport("user32.dll")] private static extern bool GetGUIThreadInfo(uint thread, ref GuiThread info);
        [DllImport("user32.dll")] private static extern bool IsChild(IntPtr parent, IntPtr child);
        [DllImport("user32.dll")] private static extern bool PeekMessageW(out Message msg, IntPtr hwnd, uint min, uint max, uint remove);
        [DllImport("user32.dll")] private static extern bool TranslateMessage(ref Message msg);
        [DllImport("user32.dll")] private static extern IntPtr DispatchMessageW(ref Message msg);

        private readonly Point previous;
        public IntPtr Window { get; private set; }

        public ForegroundLauncher()
        {
            if (!GetCursorPos(out previous)) throw new Win32Exception(Marshal.GetLastWin32Error());
            // A static text control is transparent to WindowFromPoint. Use a
            // real button so the click reaches the foreground launcher's client.
            Window = CreateWindowExW(0x80, "BUTTON", "Launch QuotaBar", 0x10cf0000, 35, 35, 340, 130,
                IntPtr.Zero, IntPtr.Zero, IntPtr.Zero, IntPtr.Zero);
            if (Window == IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error());
        }

        private static void Pump()
        {
            while (PeekMessageW(out Message msg, IntPtr.Zero, 0, 0, 1))
            {
                TranslateMessage(ref msg);
                DispatchMessageW(ref msg);
            }
            Thread.Sleep(10);
        }

        public void TakeFocus()
        {
            var deadline = Stopwatch.StartNew();
            var point = new Point { X = 95, Y = 95 };
            do
            {
                if (!SetWindowPos(Window, new IntPtr(-1), 35, 35, 340, 130, 0x40) || !SetCursorPos(point.X, point.Y))
                    throw new InvalidOperationException("Cannot place the foreground launcher's input target.");
                Pump();
                if (WindowFromPoint(point) != Window) continue;
                mouse_event(2, 0, 0, 0, UIntPtr.Zero);
                mouse_event(4, 0, 0, 0, UIntPtr.Zero);
                Pump();
                if (GetForegroundWindow() != Window) continue;
                SetFocus(Window);
                if (!SetWindowPos(Window, new IntPtr(-2), 0, 0, 0, 0, 0x13))
                    throw new InvalidOperationException("Cannot restore launcher z-order.");
                // A new dashboard may still be finishing its first activation.
                // Start the next launch only after the click holds keyboard focus.
                var stable = Stopwatch.StartNew();
                while (ReadFocus(Window).Matches)
                {
                    if (stable.ElapsedMilliseconds >= 400) return;
                    Pump();
                }
            } while (deadline.Elapsed < TimeSpan.FromSeconds(8));
            var final = ReadFocus(Window);
            throw new TimeoutException($"Launcher {final.Window} could not hold focus. Foreground {final.Foreground}, keyboard {final.Keyboard}, hit {WindowFromPoint(point)}.");
        }

        public sealed class Focus
        {
            public long Window, Foreground, Keyboard;
            public bool Matches;
        }

        private static Focus ReadFocus(IntPtr window)
        {
            var info = new GuiThread { Size = Marshal.SizeOf<GuiThread>() };
            var foreground = GetForegroundWindow();
            bool read = GetGUIThreadInfo(0, ref info);
            return new Focus {
                Window = window.ToInt64(), Foreground = foreground.ToInt64(), Keyboard = info.Focus.ToInt64(),
                Matches = read && foreground == window && (info.Focus == window || IsChild(window, info.Focus))
            };
        }

        public static Focus WaitForFocus(IntPtr window)
        {
            var deadline = Stopwatch.StartNew();
            Focus focus;
            do
            {
                Pump();
                focus = ReadFocus(window);
                if (focus.Matches) return focus;
            } while (deadline.Elapsed < TimeSpan.FromSeconds(8));
            return focus;
        }

        public void Dispose()
        {
            DestroyWindow(Window);
            if (GetCursorPos(out Point cursor) && cursor.X == 95 && cursor.Y == 95)
                SetCursorPos(previous.X, previous.Y);
        }
    }
}
