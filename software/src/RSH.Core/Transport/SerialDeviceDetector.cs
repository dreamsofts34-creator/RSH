// RSH wired USB-serial transport — device auto-detect (Phase G, D26 /
// OPEN-20). Har candidate port kholo, HELO bhejo, HELLO reply ka intezar
// karo — pehla port jis se valid HELLO (92 B, §2.2) aaye, wahi RSH board
// hai. Ghalat port (doosra USB-serial device, khaali port) khamoshi se
// chhor diya jata hai — koi exception bahar nahi aati, sirf null milta hai.
using RSH.Core.Protocol;

namespace RSH.Core.Transport;

/// <summary>Auto-detect ka nateeja: port + us ka HELLO reply (nonce/key/session ke liye).</summary>
public sealed record DetectedSerialDevice(string PortName, HelloReply Hello);

/// <summary>
/// USB-serial RSH board dhoondo: candidate ports par HELO probe karo.
/// <paramref name="openLink"/> sirf tests ke liye (fake port); null ho
/// to asal SystemSerialPortAdapter banta hai.
/// </summary>
public static class SerialDeviceDetector
{
    /// <summary>Ek port par HELO probe ka timeout (board foran jawab deta hai — 500 ms kaafi hai).</summary>
    public static readonly TimeSpan ProbeTimeout = TimeSpan.FromMilliseconds(500);

    /// <summary>
    /// Candidate ports ko tarteeb se probe karo. Pehla valid HELLO milte
    /// hi (portName, hello) return karo; koi board na mile to null.
    /// Har port probe ke baad band hota hai — caller ko khula port nahi
    /// milta (caller apna SerialLink khud banata hai).
    /// </summary>
    public static async Task<DetectedSerialDevice?> DetectAsync(
        IEnumerable<string> portNames,
        Func<string, SerialLink>? openLink = null,
        TimeSpan? perPortTimeout = null,
        CancellationToken cancellationToken = default)
    {
        ArgumentNullException.ThrowIfNull(portNames);
        TimeSpan timeout = perPortTimeout ?? ProbeTimeout;
        foreach (string portName in portNames)
        {
            if (cancellationToken.IsCancellationRequested) return null;
            SerialLink? link = null;
            try
            {
                link = openLink is not null
                    ? openLink(portName)
                    : new SerialLink(new SystemSerialPortAdapter(portName));
                link.Start();
                // Pehla HELO naya session hai (naya nonce — board ka
                // session shuru hota hai; dobara probe par resume hota).
                HelloReply? hello = await link.SendHelloAsync(newSession: true, timeout, cancellationToken)
                    .ConfigureAwait(false);
                if (hello is not null)
                    return new DetectedSerialDevice(portName, hello.Value);
            }
            catch (OperationCanceledException) { return null; }
            catch (Exception)
            {
                // Port khula nahi / jawab nahi — agla port try karo.
            }
            finally
            {
                try { link?.Dispose(); } catch { /* probe ke doran khamoshi */ }
            }
        }
        return null;
    }

    /// <summary>System ke serial ports (Windows: COMn; Linux: /dev/ttyUSB*, /dev/ttyACM*, /dev/ttyS*).</summary>
    public static string[] GetSystemPortNames() => SystemSerialPortAdapter.GetPortNames();

    /// <summary>System ports par auto-detect chalao (aam istemal).</summary>
    public static Task<DetectedSerialDevice?> DetectOnSystemPortsAsync(
        TimeSpan? perPortTimeout = null,
        CancellationToken cancellationToken = default) =>
        DetectAsync(GetSystemPortNames(), null, perPortTimeout, cancellationToken);
}
