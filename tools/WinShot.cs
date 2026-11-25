// Window capture / click helper for docs/screenshots. PowerShell type (Add-Type -Path), no dependencies.
// A window is captured with PrintWindow (works even if it is covered); layered flyouts often come back empty, so the
// caller can ask for the screen region instead, and Shot() falls back to it on its own when the bitmap has no ink.
using System;
using System.Collections.Generic;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;
using System.Text;

public static class WinShot
{
    public delegate bool EnumProc(IntPtr h, IntPtr l);

    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc cb, IntPtr l);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] static extern bool IsWindow(IntPtr h);
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
    [DllImport("user32.dll")] static extern IntPtr GetWindow(IntPtr h, uint cmd);
    [DllImport("user32.dll")] static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] static extern bool BringWindowToTop(IntPtr h);
    [DllImport("user32.dll")] static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] static extern bool GetCursorPos(out POINT p);
    [DllImport("user32.dll")] static extern void mouse_event(uint f, int dx, int dy, uint d, IntPtr e);

    public struct RECT { public int Left, Top, Right, Bottom; }
    public struct POINT { public int X, Y; }

    // a collected delegate makes the native callback crash the process: keep one static instance
    static List<IntPtr> found = new List<IntPtr>();
    static readonly EnumProc keep = Keep;
    static bool Keep(IntPtr h, IntPtr l) { found.Add(h); return true; }

    public static List<IntPtr> All() { found = new List<IntPtr>(); EnumWindows(keep, IntPtr.Zero); return found; }
    public static string ClassOf(IntPtr h) { var s = new StringBuilder(128); GetClassNameW(h, s, 128); return s.ToString(); }
    public static string TitleOf(IntPtr h) { var s = new StringBuilder(256); GetWindowTextW(h, s, 256); return s.ToString(); }
    public static uint PidOf(IntPtr h) { uint p; GetWindowThreadProcessId(h, out p); return p; }
    public static bool Visible(IntPtr h) { return IsWindowVisible(h); }
    public static bool Alive(IntPtr h) { return IsWindow(h); }
    public static bool IsOwned(IntPtr h) { return GetWindow(h, 4) != IntPtr.Zero; }
    public static int[] RectOf(IntPtr h)
    {
        RECT r; if (!GetWindowRect(h, out r)) return null;
        return new[] { r.Left, r.Top, r.Right - r.Left, r.Bottom - r.Top };
    }

    public static IntPtr Find(string cls, string titlePart, uint pid)
    {
        foreach (var h in All())
        {
            if (!Visible(h)) continue;
            if (cls.Length > 0 && ClassOf(h) != cls) continue;
            if (pid != 0 && PidOf(h) != pid) continue;
            if (titlePart.Length > 0 && TitleOf(h).IndexOf(titlePart, StringComparison.OrdinalIgnoreCase) < 0) continue;
            return h;
        }
        return IntPtr.Zero;
    }

    /// PrintWindow, then the screen region when the result has no ink (or when `screen` is set). `pad` adds a margin
    /// around the window, which is where its shadow lands.
    public static bool Shot(IntPtr h, string path, bool screen, int pad)
    {
        int[] r = RectOf(h);
        if (r == null || r[2] < 2 || r[3] < 2) return false;
        int x = r[0] - pad, y = r[1] - pad, w = r[2] + pad * 2, ht = r[3] + pad * 2;
        using (var bmp = new Bitmap(w, ht, PixelFormat.Format32bppArgb))
        {
            bool ok = false;
            if (!screen)
            {
                using (var g = Graphics.FromImage(bmp))
                {
                    IntPtr dc = g.GetHdc();
                    ok = PrintWindow(h, dc, 2); // PW_RENDERFULLCONTENT
                    g.ReleaseHdc(dc);
                }
                ok = ok && HasInk(bmp);
            }
            if (!ok)
            {
                SetForegroundWindow(h);
                BringWindowToTop(h);
                System.Threading.Thread.Sleep(160);
                using (var g = Graphics.FromImage(bmp)) g.CopyFromScreen(x, y, 0, 0, new Size(w, ht));
            }
            bmp.Save(path, ImageFormat.Png);
        }
        return true;
    }

    static bool HasInk(Bitmap b)
    {
        int sx = Math.Max(1, b.Width / 32), sy = Math.Max(1, b.Height / 32);
        for (int y = 0; y < b.Height; y += sy)
            for (int x = 0; x < b.Width; x += sx)
            {
                Color c = b.GetPixel(x, y);
                if (c.A > 8 && (c.R + c.G + c.B) > 12) return true;
            }
        return false;
    }

    public static int[] Cursor() { POINT p; GetCursorPos(out p); return new[] { p.X, p.Y }; }
    public static void Move(int x, int y) { SetCursorPos(x, y); }

    [DllImport("user32.dll")] static extern void keybd_event(byte vk, byte scan, uint flags, IntPtr extra);
    /// A real key to the focused window (the flyouts take Escape to close).
    public static void Key(byte vk) { keybd_event(vk, 0, 0, IntPtr.Zero); keybd_event(vk, 0, 2, IntPtr.Zero); }

    /// The largest visible, unowned window of `pid`: an app's frame, not its tool windows.
    public static IntPtr MainOf(uint pid)
    {
        IntPtr best = IntPtr.Zero; long area = 0;
        foreach (var h in All())
        {
            if (!Visible(h) || PidOf(h) != pid || IsOwned(h)) continue;
            int[] r = RectOf(h); if (r == null) continue;
            if (ClassOf(h) == "IME") continue;
            long a = (long)r[2] * r[3];
            if (a > area) { area = a; best = h; }
        }
        return best;
    }

    /// A real click at the cursor: the bar and its flyouts read mouse messages, not sent keys.
    public static void Click(int x, int y, bool right)
    {
        SetCursorPos(x, y);
        System.Threading.Thread.Sleep(50);
        if (right)
        {
            mouse_event(0x0008, 0, 0, 0, IntPtr.Zero);
            mouse_event(0x0010, 0, 0, 0, IntPtr.Zero);
        }
        else
        {
            mouse_event(0x0002, 0, 0, 0, IntPtr.Zero);
            mouse_event(0x0004, 0, 0, 0, IntPtr.Zero);
        }
    }
}
