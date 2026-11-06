param(
    [Parameter(Mandatory=$true)][ValidateSet("Capture","Move","Click","Key","Type","Scroll","Close")][string]$Action,
    [Parameter(Mandatory=$true)][int]$WindowProcessId,
    [Parameter(Mandatory=$true)][string]$Output,
    [int]$X=0, [int]$Y=0, [int]$Width=1600, [int]$Height=1000,
    [string]$Text="", [int]$Delta=-240
)
$ErrorActionPreference="Stop"
$root=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot "..\.."))
$outputPath=[IO.Path]::GetFullPath($Output)
if(-not $outputPath.StartsWith($root+[IO.Path]::DirectorySeparatorChar,[StringComparison]::OrdinalIgnoreCase)) { throw "Evidence must stay inside this checkout." }
$process=Get-Process -Id $WindowProcessId
if(-not $process.Path.StartsWith($root+[IO.Path]::DirectorySeparatorChar,[StringComparison]::OrdinalIgnoreCase) -or $process.ProcessName -ne "vfx-launcher") { throw "The target is not this checkout's VFX Launcher." }
if($process.SessionId -ne [Diagnostics.Process]::GetCurrentProcess().SessionId) { throw "Run the helper in the application's interactive session." }
Add-Type -AssemblyName System.Windows.Forms,System.Drawing
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class VfxDesktop {
  [StructLayout(LayoutKind.Sequential)] public struct Rect { public int Left,Top,Right,Bottom; }
  [StructLayout(LayoutKind.Sequential)] public struct Point { public int X,Y; }
  [StructLayout(LayoutKind.Sequential)] public struct Mouse { public int X,Y; public uint Data,Flags,Time; public UIntPtr Extra; }
  [StructLayout(LayoutKind.Sequential)] public struct Keyboard { public ushort Key,Scan; public uint Flags,Time; public UIntPtr Extra; }
  [StructLayout(LayoutKind.Explicit)] public struct InputUnion { [FieldOffset(0)] public Mouse Mouse; [FieldOffset(0)] public Keyboard Keyboard; }
  [StructLayout(LayoutKind.Sequential)] public struct Input { public uint Type; public InputUnion Union; }
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr window,out Rect rect);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr window);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr window,out uint process);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr window,IntPtr after,int x,int y,int width,int height,uint flags);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x,int y);
  [DllImport("user32.dll")] public static extern bool GetCursorPos(out Point point);
  [DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(Point point);
  [DllImport("user32.dll")] public static extern void mouse_event(uint flags,uint x,uint y,uint data,UIntPtr extra);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr window,uint message,IntPtr w,IntPtr l);
  [DllImport("user32.dll",SetLastError=true)] public static extern uint SendInput(uint count,ref Input input,int size);
  public static void Type(string text) {
    foreach(char value in text) {
      Input input=new Input(); input.Type=1; input.Union.Keyboard.Scan=value; input.Union.Keyboard.Flags=4;
      if(SendInput(1,ref input,Marshal.SizeOf(typeof(Input)))!=1) throw new InvalidOperationException("Unicode key press failed");
      input.Union.Keyboard.Flags=6;
      if(SendInput(1,ref input,Marshal.SizeOf(typeof(Input)))!=1) throw new InvalidOperationException("Unicode key release failed");
    }
  }
}
'@
[VfxDesktop]::SetProcessDPIAware() | Out-Null
$process.Refresh()
$window=$process.MainWindowHandle
if($window -eq [IntPtr]::Zero) { throw "The application has no visible window." }
[VfxDesktop]::SetForegroundWindow($window) | Out-Null
Start-Sleep -Milliseconds 200
if($Action -eq "Move") {
    if($Width -lt 1080 -or $Height -lt 700) { throw "Use a supported desktop window size." }
    if(-not [VfxDesktop]::SetWindowPos($window,[IntPtr]::Zero,$X,$Y,$Width,$Height,0x0040)) { throw "Window movement failed." }
}
$foreground=[VfxDesktop]::GetForegroundWindow()
[uint32]$foregroundProcess=0
[VfxDesktop]::GetWindowThreadProcessId($foreground,[ref]$foregroundProcess) | Out-Null
if($foregroundProcess -ne $WindowProcessId) { throw "Another application has focus; refusing input." }
switch($Action) {
    { $_ -eq "Click" -or $_ -eq "Scroll" } {
        $point=New-Object VfxDesktop+Point
        $point.X=$X; $point.Y=$Y
        [uint32]$pointProcess=0
        [VfxDesktop]::GetWindowThreadProcessId([VfxDesktop]::WindowFromPoint($point),[ref]$pointProcess) | Out-Null
        if($pointProcess -ne $WindowProcessId) { throw "The click is outside the target application." }
        if(-not [VfxDesktop]::SetCursorPos($X,$Y)) { throw "Pointer movement failed." }
        $landed=New-Object VfxDesktop+Point
        if(-not [VfxDesktop]::GetCursorPos([ref]$landed) -or $landed.X -ne $X -or $landed.Y -ne $Y) { throw "Pointer did not land at the requested pixel." }
        if($Action -eq "Scroll") {
            $wheel=[BitConverter]::ToUInt32([BitConverter]::GetBytes($Delta),0)
            [VfxDesktop]::mouse_event(0x0800,0,0,$wheel,[UIntPtr]::Zero)
        } else {
            [VfxDesktop]::mouse_event(2,0,0,0,[UIntPtr]::Zero)
            [VfxDesktop]::mouse_event(4,0,0,0,[UIntPtr]::Zero)
        }
    }
    "Key" { [System.Windows.Forms.SendKeys]::SendWait($Text) }
    "Type" { [VfxDesktop]::Type($Text) }
    "Close" { if(-not [VfxDesktop]::PostMessage($window,0x0010,[IntPtr]::Zero,[IntPtr]::Zero)) { throw "Close request failed." } }
}
if($Action -eq "Close") {
    New-Item -ItemType Directory -Force (Split-Path $outputPath) | Out-Null
    @{ action=$Action; process=$WindowProcessId; closeRequested=$true } | ConvertTo-Json -Compress | Set-Content "$outputPath.json" -Encoding UTF8
    return
}
$rect=New-Object VfxDesktop+Rect
if(-not [VfxDesktop]::GetWindowRect($window,[ref]$rect)) { throw "The window bounds could not be read." }
New-Item -ItemType Directory -Force (Split-Path $outputPath) | Out-Null
if($Action -eq "Capture") {
    $bitmap=New-Object Drawing.Bitmap ($rect.Right-$rect.Left),($rect.Bottom-$rect.Top)
    $graphics=[Drawing.Graphics]::FromImage($bitmap)
    try {
        $graphics.CopyFromScreen($rect.Left,$rect.Top,0,0,$bitmap.Size)
        $bitmap.Save($outputPath,[Drawing.Imaging.ImageFormat]::Png)
    } finally { $graphics.Dispose(); $bitmap.Dispose() }
}
@{ action=$Action; process=$WindowProcessId; x=$rect.Left; y=$rect.Top; width=($rect.Right-$rect.Left); height=($rect.Bottom-$rect.Top); screenshot=$(if($Action -eq "Capture"){$outputPath}else{$null}) } | ConvertTo-Json -Compress | Set-Content "$outputPath.json" -Encoding UTF8
