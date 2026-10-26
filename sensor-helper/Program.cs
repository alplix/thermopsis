// Reads CPU / board sensors through LibreHardwareMonitorLib (MPL-2.0).
// usage: thermopsis-sensors <out-file> <parent-pid> [interval-seconds]
// Writes {"key":value,...} to <out-file> (atomically) every interval until the parent process is gone. Keys: t.win.cpu.<name>, p.win.cpu.<name>, t/f.win.board.<name>.
using LibreHardwareMonitor.Hardware;
using System.Diagnostics;
using System.Globalization;
using System.Text;

if (args.Length < 2 || !int.TryParse(args[1], out int parent)) { Console.Error.WriteLine("usage: thermopsis-sensors <out-file> <parent-pid> [seconds]"); return 2; }
string outFile = args[0];
int every = args.Length > 2 && int.TryParse(args[2], out int e) && e >= 1 ? e : 5;
var pc = new Computer { IsCpuEnabled = true, IsMotherboardEnabled = true };
pc.Open();
string Clean(string s) { var t = new string(s.ToLowerInvariant().Select(c => char.IsLetterOrDigit(c) ? c : '_').ToArray()).Trim('_'); return t.StartsWith("cpu_") ? t[4..] : t; }
void Visit(IHardware h, StringBuilder sb, ref bool first)
{
    h.Update();
    foreach (var s in h.Sensors)
    {
        if (s.Value is not float v || float.IsNaN(v)) continue;
        string? kind = s.SensorType switch { SensorType.Temperature => "t", SensorType.Power => "p", SensorType.Fan => "f", _ => null };
        if (kind == null || s.Name.Contains("Distance to TjMax")) continue;
        string dev = h.HardwareType == HardwareType.Cpu ? "cpu" : "board";
        if (dev == "board" && kind == "p") continue;
        if (!first) sb.Append(',');
        first = false;
        sb.Append($"\"{kind}.win.{dev}.{Clean(s.Name)}\":{v.ToString("0.##", CultureInfo.InvariantCulture)}");
    }
    foreach (var sub in h.SubHardware) Visit(sub, sb, ref first);
}
bool ParentAlive() { try { return !Process.GetProcessById(parent).HasExited; } catch { return false; } }
while (ParentAlive())
{
    var sb = new StringBuilder("{"); bool first = true;
    foreach (var h in pc.Hardware) Visit(h, sb, ref first);
    sb.Append('}');
    File.WriteAllText(outFile + ".tmp", sb.ToString());
    File.Move(outFile + ".tmp", outFile, true);
    Thread.Sleep(every * 1000);
}
pc.Close();
return 0;
